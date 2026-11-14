//! A scene document becomes layers.

use std::collections::HashMap;

use wepaper_json::Value;

use crate::doc::{
    Blending, Dyn, EffectDoc, EffectUse, ImageObject, MaterialDoc, MaterialPass, ModelDoc, Object,
    ObjectKind, Project, Scene, TextObject,
};
use crate::glsl::Combos;
use crate::gpu::{Blend, Mesh};
use crate::math::Mat4;
use crate::read::Files;
use crate::read::puppet::layers_from_scene;
use crate::{Engine, Fill, LoadError, Viewport};

use super::bindings::{Bindings, SceneField, Slot};
use super::particles::ParticleLayer;
use super::puppet::PuppetState;
use super::shade::{PassInput, Shade, TextureChoice, TextureRef};
use super::{BufferSpec, Effect, ImageLayer, ImageMode, Layer, LayerKind, SceneInstance, SoundLayer, Step};

impl SceneInstance {
    /// Builds a playing copy of the scene in `files` for an output of `viewport`.
    ///
    /// `values` are the user's property values; properties not in it keep the
    /// wallpaper's own. A layer that cannot be built is skipped and noted.
    pub(crate) fn build(
        engine: &mut Engine,
        files: &Files,
        project: Project,
        viewport: Viewport,
        values: &[(String, Value)],
    ) -> Result<Self, LoadError> {
        let text = files.read_text(&project.file).map_err(|e| LoadError(format!("{}: {e}", project.file)))?;
        let document = wepaper_json::parse(&text).map_err(|e| LoadError(format!("{}: {e}", project.file)))?;
        let scene = Scene::parse(&document);

        let mut bindings = Bindings::default();
        // What is built from a property's value (a particle system's size) needs the
        // values before the layers exist; what follows a property is set after.
        for property in project.properties.iter().filter(|property| !property.default.is_null()) {
            bindings.properties.insert(property.name.clone(), property.default.clone());
        }
        for (name, value) in values.iter().filter(|(_, value)| !value.is_null()) {
            bindings.properties.insert(name.clone(), value.clone());
        }
        let mut skipped = Vec::new();
        let index_of: HashMap<i64, usize> =
            scene.objects.iter().enumerate().map(|(index, object)| (object.id, index)).collect();
        let mut scene_size = scene.general.orthogonal.filter(|s| s[0] > 0.0 && s[1] > 0.0);
        if scene.general.orthogonal.is_none() {
            skipped.push("the scene uses a perspective camera, which is drawn flat".to_owned());
        }

        let general = &scene.general;
        bindings.link(&general.zoom, Slot::Scene(SceneField::Zoom));
        bindings.link(&general.clear_color, Slot::Scene(SceneField::Clear));
        bindings.link(&general.bloom, Slot::Scene(SceneField::Bloom));
        bindings.link(&general.parallax, Slot::Scene(SceneField::Parallax));
        bindings.link(&general.parallax_amount, Slot::Scene(SceneField::ParallaxAmount));
        bindings.link(&general.parallax_delay, Slot::Scene(SceneField::ParallaxDelay));
        bindings.link(&general.parallax_mouse_influence, Slot::Scene(SceneField::ParallaxMouse));
        bindings.link(&general.shake, Slot::Scene(SceneField::Shake));
        bindings.link(&general.shake_amplitude, Slot::Scene(SceneField::ShakeAmplitude));
        bindings.link(&general.shake_roughness, Slot::Scene(SceneField::ShakeRoughness));
        bindings.link(&general.shake_speed, Slot::Scene(SceneField::ShakeSpeed));
        // The bloom is built when the scene can ever bloom: now, or when a property says so.
        let bloom = (scene.general.bloom.value || scene.general.bloom.user.is_some())
            .then(|| build_bloom(engine, files, &mut bindings, &scene))
            .and_then(|bloom| bloom.inspect_err(|why| log::warn!("bloom: {why}")).ok());
        let mut layers = Vec::with_capacity(scene.objects.len());
        for (index, object) in scene.objects.iter().enumerate() {
            bindings.owner = (index, None);
            let kind = match &object.kind {
                ObjectKind::Image(image) => {
                    match build_image(
                        engine,
                        files,
                        &mut bindings,
                        index,
                        image,
                        scene_size.unwrap_or([1920.0, 1080.0]),
                    ) {
                        Ok(layer) => {
                            // A scene without a size of its own takes its first picture's.
                            scene_size.get_or_insert(layer.size);
                            LayerKind::Image(Box::new(layer))
                        }
                        Err(why) => {
                            log::warn!("layer {} ({}) is skipped: {why}", object.name, object.id);
                            skipped.push(format!("layer {}: {why}", object.name));
                            LayerKind::Group
                        }
                    }
                }
                ObjectKind::Text(text) => match build_text(engine, files, &mut bindings, text) {
                    Ok(layer) => LayerKind::Image(Box::new(layer)),
                    Err(why) => {
                        log::warn!("text layer {} ({}) is skipped: {why}", object.name, object.id);
                        skipped.push(format!("layer {}: {why}", object.name));
                        LayerKind::Group
                    }
                },
                ObjectKind::Sound(sound) => {
                    let file = sound.files.first().and_then(|file| {
                        engine
                            .share
                            .on_disk(files, file)
                            .inspect_err(|why| log::warn!("sound {file}: {why}"))
                            .ok()
                    });
                    LayerKind::Sound(SoundLayer {
                        file,
                        looping: sound.mode != "single",
                        volume: sound.volume.value.clamp(0.0, 1.0),
                        player: None,
                    })
                }
                ObjectKind::Particle(particles) => {
                    // The object's id as the seed: the same wallpaper plays the same.
                    match ParticleLayer::build(
                        engine,
                        files,
                        &mut bindings,
                        particles,
                        index,
                        object.id as u64,
                    ) {
                        Ok(layer) => LayerKind::Particles(Box::new(layer)),
                        Err(why) => {
                            log::warn!("particles {} ({}) are skipped: {why}", object.name, object.id);
                            skipped.push(format!("layer {}: {why}", object.name));
                            LayerKind::Group
                        }
                    }
                }
                ObjectKind::Unsupported(what) => {
                    skipped.push(format!("layer {}: {what} objects are not drawn", object.name));
                    LayerKind::Group
                }
                _ => LayerKind::Group,
            };
            bind_object(&mut bindings, index, object);
            layers.push(Layer {
                id: object.id,
                name: object.name.clone(),
                parent: object.parent.and_then(|id| index_of.get(&id).copied()),
                visible: object.visible.value,
                origin: object.origin.value,
                scale: object.scale.value,
                angles: object.angles.value,
                parallax_depth: object.parallax_depth.value,
                world: Mat4::IDENTITY,
                shown: true,
                kind,
            });
        }

        let mut instance = Self {
            order: (0..layers.len()).collect(),
            layers,
            bindings,
            #[cfg(feature = "script")]
            scripts: None,
            scene_size: scene_size.unwrap_or([1920.0, 1080.0]),
            clear: scene.general.clear_color.value,
            zoom: scene.general.zoom.value.max(0.01),
            viewport,
            fill: Fill::Cover,
            view_projection: Mat4::IDENTITY,
            skipped,
            project,
            bloom,
            provides: std::collections::HashSet::new(),
            videos: Vec::new(),
            parallax: super::Parallax {
                enabled: scene.general.parallax.value,
                amount: scene.general.parallax_amount.value,
                delay: scene.general.parallax_delay.value.max(0.0),
                mouse_influence: scene.general.parallax_mouse_influence.value,
            },
            eased_pointer: [0.5, 0.5],
            shake: super::Shake {
                enabled: scene.general.shake.value,
                amplitude: scene.general.shake_amplitude.value,
                roughness: scene.general.shake_roughness.value,
                speed: scene.general.shake_speed.value,
            },
            last_time: 0.0,
            wants_audio: false,
            paused: false,
            sound: (false, 0.0),
        };
        instance.hold_videos();
        let (sound, volume) = (engine.share.videos.options.audio, engine.share.videos.options.volume);
        instance.set_sound(sound, volume);
        // Every property takes effect once: the wallpaper's own value, or the user's.
        let defaults: Vec<(String, Value)> =
            instance.project.properties.iter().map(|p| (p.name.clone(), p.default.clone())).collect();
        for (name, value) in defaults.iter().chain(values) {
            if !value.is_null() {
                instance.set_property(name, value);
            }
        }
        #[cfg(feature = "script")]
        instance.attach_scripts(files);
        Ok(instance)
    }
}

impl SceneInstance {
    /// Takes a place on every video the layers show, each video once.
    fn hold_videos(&mut self) {
        let mut streams: Vec<crate::video::SharedVideo> = Vec::new();
        let mut wants_audio = false;
        let mut provides = std::collections::HashSet::new();
        let mut note = |shade: &Shade| {
            wants_audio |= shade.wants_audio;
            provides.extend(shade.samplers.iter().filter_map(|(_, _, source)| match source {
                TextureRef::Layer(id) => Some(*id),
                _ => None,
            }));
            for (_, _, source) in &shade.samplers {
                if let TextureRef::Video(video) = source
                    && !streams.iter().any(|known| std::rc::Rc::ptr_eq(known, video))
                {
                    streams.push(video.clone());
                }
            }
        };
        for layer in &self.layers {
            match &layer.kind {
                LayerKind::Image(image) => {
                    note(&image.base);
                    for step in image.effects.iter().flat_map(|e| &e.steps) {
                        if let Step::Draw { shade, .. } = step {
                            note(shade);
                        }
                    }
                }
                LayerKind::Particles(particles) => particles.parts.iter().for_each(|part| note(&part.shade)),
                _ => {}
            }
        }
        self.videos = streams.into_iter().map(crate::video::VideoHold::new).collect();
        self.wants_audio = wants_audio;
        self.provides = provides;
    }
}

/// The scene's bloom passes, from the stock shaders.
fn build_bloom(
    engine: &mut Engine,
    files: &Files,
    bindings: &mut Bindings,
    scene: &Scene,
) -> Result<super::Bloom, String> {
    let general = &scene.general;
    let number = |value: &Dyn<f32>| Dyn {
        value: Value::from(value.value),
        user: value.user.clone(),
        script: None,
        animation: None,
    };
    let constants = [
        ("bloomstrength".to_owned(), number(&general.bloom_strength)),
        ("bloomthreshold".to_owned(), number(&general.bloom_threshold)),
    ];
    let mut pass = |shader: &str, textures: &[TextureChoice], constants: &[(String, Dyn<Value>)]| {
        Shade::compile(
            &mut engine.share,
            files,
            &PassInput {
                shader,
                textures,
                combos: &Combos::new(),
                constants,
                buffers: &["_bloom".to_owned()],
                slots: &[],
            },
            bindings,
        )
    };
    Ok(super::Bloom {
        bright: pass("downsample_quarter_bloom", &[TextureChoice::Previous], &constants)?,
        across: pass("downsample_eighth_blur_v", &[TextureChoice::Previous], &[])?,
        down: pass("blur_h_bloom", &[TextureChoice::Previous], &[])?,
        combine: pass("combine", &[TextureChoice::Previous, TextureChoice::Buffer(0)], &[])?,
        enabled: general.bloom.value,
    })
}

/// Registers what drives an object's own fields.
fn bind_object(bindings: &mut Bindings, index: usize, object: &Object) {
    bindings.link(&object.visible, Slot::LayerVisible(index));
    bindings.link(&object.origin, Slot::LayerOrigin(index));
    bindings.link(&object.scale, Slot::LayerScale(index));
    bindings.link(&object.angles, Slot::LayerAngles(index));
    if let ObjectKind::Image(image) = &object.kind {
        bindings.link(&image.alpha, Slot::LayerAlpha(index));
        bindings.link(&image.color, Slot::LayerColor(index));
        for (effect_index, effect) in image.effects.iter().enumerate() {
            bindings.link(&effect.visible, Slot::EffectVisible(index, effect_index));
        }
    }
    if let ObjectKind::Text(text) = &object.kind {
        bindings.link(&text.text, Slot::LayerText(index));
        bindings.link(&text.alpha, Slot::LayerAlpha(index));
        bindings.link(&text.color, Slot::LayerColor(index));
        for (effect_index, effect) in text.effects.iter().enumerate() {
            bindings.link(&effect.visible, Slot::EffectVisible(index, effect_index));
        }
    }
}

fn read_doc(files: &Files, path: &str) -> Result<Value, String> {
    let text = files.read_text(path).map_err(|e| e.to_string())?;
    wepaper_json::parse(&text).map_err(|e| format!("{path}: {e}"))
}

pub(super) fn choices(textures: &[Option<String>]) -> Vec<TextureChoice> {
    textures.iter().map(|t| t.clone().map_or(TextureChoice::Default, TextureChoice::Named)).collect()
}

pub(super) fn constants(pass: &MaterialPass) -> Vec<(String, Dyn<Value>)> {
    pass.constants.iter().map(|(name, value)| (name.clone(), Dyn::constant(value.clone()))).collect()
}

fn build_image(
    engine: &mut Engine,
    files: &Files,
    bindings: &mut Bindings,
    index: usize,
    image: &ImageObject,
    scene_size: [f32; 2],
) -> Result<ImageLayer, String> {
    let model = ModelDoc::parse(&read_doc(files, &image.image)?);
    let material = MaterialDoc::parse(&read_doc(files, &model.material)?);
    let pass = material.passes.first().ok_or("the material has no passes")?;
    let mode = match (model.passthrough, model.fullscreen) {
        (true, true) => ImageMode::Fullscreen,
        (true, false) => ImageMode::Compose,
        _ => ImageMode::Normal,
    };

    let base = Shade::compile(
        &mut engine.share,
        files,
        &PassInput {
            shader: &pass.shader,
            textures: &choices(&pass.textures),
            combos: &pass.combos,
            constants: &constants(pass),
            buffers: &[],
            slots: &[],
        },
        bindings,
    )?;

    // The picture: its size and how much of its texture it fills.
    let picture =
        base.samplers.iter().find(|(_, unit, _)| *unit == 0).and_then(|(_, _, source)| match source {
            TextureRef::Shared(texture) => Some(texture.clone()),
            _ => None,
        });
    let (picture_size, extent) = match &picture {
        Some(texture) if texture.sprites.is_some() => {
            let frame = texture.sprites.as_ref().and_then(|s| s.frames.first().copied());
            (
                frame.map(|f| [f.width.abs().max(f.skew.1.abs()), f.height.abs().max(f.skew.0.abs())]),
                [1.0, 1.0],
            )
        }
        Some(texture) => {
            let r = texture.resolution;
            (Some([r[2], r[3]]), [(r[2] / r[0]).min(1.0), (r[3] / r[1]).min(1.0)])
        }
        None => (None, [1.0, 1.0]),
    };
    let size = if mode == ImageMode::Fullscreen {
        scene_size
    } else {
        image.size.or(model.size).or(picture_size).unwrap_or(scene_size)
    };

    // The origin sits on the picture where the alignment says: the centre, an edge, a corner.
    let center = [
        -f32::from(image.alignment.horizontal) * size[0] / 2.0,
        -f32::from(image.alignment.vertical) * size[1] / 2.0,
    ];
    let (quad, quad_unit, base_quad) = quads(engine, size, center, extent)?;
    let (effects, buffers) = build_effects(engine, files, bindings, &image.effects);
    // A puppet: the picture goes on a mesh that bones bend.
    let puppet = model.puppet.as_deref().and_then(|path| {
        let made = engine.share.puppet(files, path, extent).and_then(|shared| {
            let layers = layers_from_scene(&image.animation_layers);
            PuppetState::new(engine, files, bindings, shared, layers, pass, index)
        });
        match made {
            Ok(mut puppet) => {
                // The mesh is measured from the picture's centre; the layer's origin may sit elsewhere.
                puppet.offset = center;
                Some(puppet)
            }
            Err(why) => {
                log::warn!("the layer is drawn flat: {why}");
                None
            }
        }
    });

    Ok(ImageLayer {
        mode,
        size,
        alpha: image.alpha.value,
        color: image.color.value,
        brightness: image.brightness.value,
        blend: match pass.blending {
            Blending::Translucent => Blend::Alpha,
            Blending::Additive => Blend::Additive,
            Blending::Premultiplied => Blend::Premultiplied,
            Blending::Normal | Blending::Opaque => Blend::None,
        },
        base,
        effects,
        buffers,
        quad,
        quad_unit,
        base_quad,
        text: None,
        blend_pass: blend_pass(engine, files, bindings, image.blend_mode),
        puppet,
    })
}

/// The pass that composites a layer with blend mode `mode`; `None` for plain blending.
fn blend_pass(engine: &mut Engine, files: &Files, bindings: &mut Bindings, mode: i32) -> Option<Shade> {
    if mode == 0 {
        return None;
    }
    let combos: Combos = [("BLENDMODE".to_owned(), mode), ("TRANSFORM".to_owned(), 1)].into();
    let textures = [TextureChoice::Previous, TextureChoice::Named("_rt_FullFrameBuffer".to_owned())];
    Shade::compile(
        &mut engine.share,
        files,
        &PassInput {
            shader: "passthroughblend",
            textures: &textures,
            combos: &combos,
            constants: &[],
            buffers: &[],
            slots: &[],
        },
        bindings,
    )
    .inspect_err(|why| log::warn!("blend mode {mode}: {why}"))
    .ok()
}

/// The three quads a layer is drawn with: in layer space over the picture, in layer
/// space over a whole buffer, and in clip space over the picture.
pub(super) fn quads(
    engine: &Engine,
    size: [f32; 2],
    center: [f32; 2],
    extent: [f32; 2],
) -> Result<(Mesh, Mesh, Mesh), String> {
    // Where the picture's centre is from the layer's origin.
    let (w, h) = (size[0], size[1]);
    let [ax, ay] = center;
    let (x0, x1, y0, y1) = (ax - w / 2.0, ax + w / 2.0, ay - h / 2.0, ay + h / 2.0);
    let [u, v] = extent;
    let mesh = |corners: &[[f32; 5]; 4]| Mesh::quad(&engine.gl, corners).map_err(|e| e.to_string());
    Ok((
        mesh(&[[x0, y0, 0.0, 0.0, v], [x1, y0, 0.0, u, v], [x0, y1, 0.0, 0.0, 0.0], [x1, y1, 0.0, u, 0.0]])?,
        mesh(&[
            [x0, y0, 0.0, 0.0, 1.0],
            [x1, y0, 0.0, 1.0, 1.0],
            [x0, y1, 0.0, 0.0, 0.0],
            [x1, y1, 0.0, 1.0, 0.0],
        ])?,
        mesh(&[
            [-1.0, -1.0, 0.0, 0.0, 0.0],
            [1.0, -1.0, 0.0, u, 0.0],
            [-1.0, 1.0, 0.0, 0.0, v],
            [1.0, 1.0, 0.0, u, v],
        ])?,
    ))
}

/// A layer's effects and the buffers they declare. An effect that cannot be built
/// is kept switched off, so the chain still joins up and the layer shows without it.
fn build_effects(
    engine: &mut Engine,
    files: &Files,
    bindings: &mut Bindings,
    uses: &[EffectUse],
) -> (Vec<Effect>, Vec<BufferSpec>) {
    let mut buffers: Vec<BufferSpec> = Vec::new();
    let mut effects = Vec::new();
    for (effect_index, effect_use) in uses.iter().enumerate() {
        // What scripts and animations the effect's values carry belongs to it.
        bindings.owner.1 = Some(effect_index);
        // Scenes rarely name their effects: the effect's folder does.
        let name = if effect_use.name.is_empty() {
            effect_use.file.rsplit('/').nth(1).unwrap_or(&effect_use.file).to_owned()
        } else {
            effect_use.name.clone()
        };
        match build_effect(engine, files, bindings, effect_use, &mut buffers) {
            Ok(steps) => effects.push(Effect { name, visible: effect_use.visible.value, steps }),
            Err(why) => {
                log::warn!("effect {} is skipped: {why}", effect_use.file);
                effects.push(Effect { name, visible: false, steps: Vec::new() });
            }
        }
    }
    bindings.owner.1 = None;
    (effects, buffers)
}

/// A text layer: an image layer whose picture is rasterised text.
fn build_text(
    engine: &mut Engine,
    files: &Files,
    bindings: &mut Bindings,
    text: &TextObject,
) -> Result<ImageLayer, String> {
    use super::text::{TextState, TextStyleSpec, TextSurface};
    let font = engine
        .share
        .font(files, &text.font)
        .or_else(|| engine.share.font(files, "fonts/NotoSans-Regular.ttf"))
        .or_else(|| engine.share.font(files, "systemfont_sans"))
        .ok_or_else(|| format!("font {} is not there and neither is a fallback", text.font))?;
    let side = |name: &str, low: &str, high: &str| {
        if name == low {
            -1
        } else if name == high {
            1
        } else {
            0
        }
    };
    let style = TextStyleSpec {
        point_size: text.point_size.value,
        horizontal: side(&text.horizontal_align, "left", "right"),
        vertical: side(&text.vertical_align, "bottom", "top"),
        // Padding is room for effects; Wallpaper Engine gives a layer without them none.
        padding: if text.effects.is_empty() { 0.0 } else { text.padding },
        max_width: if text.limit_width { text.max_width.value } else { 0.0 },
    };
    // The box is as large as the text. The scene's own number for it is the same
    // to a pixel or two for the text it was saved with, and stale for any other.
    let size = TextState::fit(&font, &text.text.value, &style);
    let surface = std::rc::Rc::new(TextSurface::default());
    let base = Shade::compile(
        &mut engine.share,
        files,
        &PassInput {
            shader: "font",
            textures: &[TextureChoice::Text(surface.clone())],
            combos: &Combos::new(),
            constants: &[],
            buffers: &[],
            slots: &[],
        },
        bindings,
    )?;
    let state = TextState { text: text.text.value.clone(), font, style, size, surface, dirty: true };
    let (quad, quad_unit, base_quad) = quads(engine, size, state.offset(), [1.0, 1.0])?;
    let (effects, buffers) = build_effects(engine, files, bindings, &text.effects);
    Ok(ImageLayer {
        mode: ImageMode::Normal,
        size,
        alpha: text.alpha.value,
        color: text.color.value,
        brightness: text.brightness.value,
        blend: Blend::Alpha,
        base,
        effects,
        buffers,
        quad,
        quad_unit,
        base_quad,
        text: Some(state),
        blend_pass: blend_pass(engine, files, bindings, text.blend_mode),
        puppet: None,
    })
}

/// The slot of the layer buffer called `name`, declared on first mention.
fn buffer_slot(buffers: &mut Vec<BufferSpec>, name: &str, scale: f32) -> usize {
    if let Some(slot) = buffers.iter().position(|b| b.name == name) {
        return slot;
    }
    buffers.push(BufferSpec { name: name.to_owned(), scale });
    buffers.len() - 1
}

fn build_effect(
    engine: &mut Engine,
    files: &Files,
    bindings: &mut Bindings,
    effect_use: &crate::doc::EffectUse,
    buffers: &mut Vec<BufferSpec>,
) -> Result<Vec<Step>, String> {
    let effect = EffectDoc::parse(&read_doc(files, &effect_use.file)?);
    for fbo in &effect.fbos {
        buffer_slot(buffers, &fbo.name, fbo.scale);
    }
    let mut steps = Vec::new();
    // Overrides in the scene are per drawing pass, in order; commands do not count.
    let mut drawing = 0;
    for pass in &effect.passes {
        if pass.command.as_deref() == Some("copy") {
            let (Some(source), Some(target)) = (&pass.source, &pass.target) else { continue };
            steps.push(Step::Copy {
                source: buffer_slot(buffers, source, 1.0),
                target: buffer_slot(buffers, target, 1.0),
            });
            continue;
        }
        let material = MaterialDoc::parse(&read_doc(files, &pass.material)?);
        let base = material.passes.first().ok_or("an effect material has no passes")?;
        let overrides = effect_use.passes.get(drawing);
        drawing += 1;

        let mut textures = choices(&base.textures);
        let mut combos: Combos = base.combos.clone();
        let mut values = constants(base);
        if let Some(overrides) = overrides {
            for (slot, texture) in overrides.textures.iter().enumerate().take(crate::glsl::TEXTURE_SLOTS) {
                if let Some(name) = texture {
                    if textures.len() <= slot {
                        textures.resize(slot + 1, TextureChoice::Default);
                    }
                    textures[slot] = TextureChoice::Named(name.clone());
                }
            }
            combos.extend(overrides.combos.iter().map(|(k, v)| (k.clone(), *v)));
            for (name, value) in &overrides.constants {
                match values.iter_mut().find(|(n, _)| n == name) {
                    Some(slot) => slot.1 = value.clone(),
                    None => values.push((name.clone(), value.clone())),
                }
            }
        }
        // A bind names a texture slot; one beyond the slots there are is ignored (a
        // number from a document must never size anything).
        for bind in pass.binds.iter().filter(|bind| bind.slot < crate::glsl::TEXTURE_SLOTS) {
            if textures.len() <= bind.slot {
                textures.resize(bind.slot + 1, TextureChoice::Default);
            }
            textures[bind.slot] = match bind.name.as_str() {
                "previous" => TextureChoice::Previous,
                name if buffers.iter().any(|b| b.name == name) => {
                    TextureChoice::Buffer(buffer_slot(buffers, name, 1.0))
                }
                name => TextureChoice::Named(name.to_owned()),
            };
        }
        // An effect works on the layer's picture unless it says otherwise.
        if textures.is_empty() {
            textures.push(TextureChoice::Previous);
        } else if matches!(textures[0], TextureChoice::Default) {
            textures[0] = TextureChoice::Previous;
        }

        let names: Vec<String> = buffers.iter().map(|b| b.name.clone()).collect();
        let shade = Shade::compile(
            &mut engine.share,
            files,
            &PassInput {
                shader: &base.shader,
                textures: &textures,
                combos: &combos,
                constants: &values,
                buffers: &names,
                slots: &[],
            },
            bindings,
        )?;
        let target = pass.target.as_ref().map(|name| buffer_slot(buffers, name, 1.0));
        steps.push(Step::Draw { shade, target });
    }
    Ok(steps)
}
