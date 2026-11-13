//! A scene document becomes layers.

use std::collections::HashMap;

use wepaper_json::Value;

use crate::doc::{Blending, Dyn, EffectDoc, ImageObject, MaterialDoc, MaterialPass, ModelDoc, Object, ObjectKind, Project, Scene};
use crate::glsl::Combos;
use crate::gpu::{Blend, Mesh, Target, TextureFormat};
use crate::math::Mat4;
use crate::read::Files;
use crate::{Engine, Fill, LoadError, Viewport};

use super::bindings::{Bindings, Slot};
use super::shade::{PassInput, Shade, TextureChoice, TextureRef};
use super::{BufferSpec, Effect, ImageLayer, ImageMode, Layer, LayerKind, SceneInstance, Step};

impl SceneInstance {
    /// Builds a playing copy of the scene in `files` for an output of `viewport`.
    ///
    /// `values` are the user's property values; properties not in it keep the
    /// wallpaper's own. A layer that cannot be built is skipped and noted.
    pub(crate) fn build(engine: &mut Engine, files: &Files, project: Project, viewport: Viewport, values: &[(String, Value)]) -> Result<Self, LoadError> {
        let text = files.read_text(&project.file).map_err(|e| LoadError(format!("{}: {e}", project.file)))?;
        let document = wepaper_json::parse(&text).map_err(|e| LoadError(format!("{}: {e}", project.file)))?;
        let scene = Scene::parse(&document);

        let mut bindings = Bindings::default();
        let mut skipped = Vec::new();
        let index_of: HashMap<i64, usize> = scene.objects.iter().enumerate().map(|(index, object)| (object.id, index)).collect();
        let mut scene_size = scene.general.orthogonal.filter(|s| s[0] > 0.0 && s[1] > 0.0);
        if scene.general.orthogonal.is_none() {
            skipped.push("the scene uses a perspective camera, which is drawn flat".to_owned());
        }

        let mut layers = Vec::with_capacity(scene.objects.len());
        for (index, object) in scene.objects.iter().enumerate() {
            let kind = match &object.kind {
                ObjectKind::Image(image) => {
                    match build_image(engine, files, &mut bindings, index, image, scene_size.unwrap_or([1920.0, 1080.0])) {
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

        let target = Target::new(&engine.gl, viewport.width.max(1), viewport.height.max(1), TextureFormat::Rgba8).map_err(|e| LoadError(e.to_string()))?;
        let mut instance = Self {
            layers,
            bindings,
            scene_size: scene_size.unwrap_or([1920.0, 1080.0]),
            clear: scene.general.clear_color.value,
            viewport,
            fill: Fill::Cover,
            target,
            view_projection: Mat4::IDENTITY,
            skipped,
            project,
        };
        // Every property takes effect once: the wallpaper's own value, or the user's.
        let defaults: Vec<(String, Value)> = instance.project.properties.iter().map(|p| (p.name.clone(), p.default.clone())).collect();
        for (name, value) in defaults.iter().chain(values) {
            if !value.is_null() {
                instance.set_property(name, value);
            }
        }
        Ok(instance)
    }
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
}

fn read_doc(files: &Files, path: &str) -> Result<Value, String> {
    let text = files.read_text(path).map_err(|e| e.to_string())?;
    wepaper_json::parse(&text).map_err(|e| format!("{path}: {e}"))
}

fn choices(textures: &[Option<String>]) -> Vec<TextureChoice> {
    textures.iter().map(|t| t.clone().map_or(TextureChoice::Default, TextureChoice::Named)).collect()
}

fn constants(pass: &MaterialPass) -> Vec<(String, Dyn<Value>)> {
    pass.constants.iter().map(|(name, value)| (name.clone(), Dyn::constant(value.clone()))).collect()
}

fn build_image(engine: &mut Engine, files: &Files, bindings: &mut Bindings, index: usize, image: &ImageObject, scene_size: [f32; 2]) -> Result<ImageLayer, String> {
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
        &PassInput { shader: &pass.shader, textures: &choices(&pass.textures), combos: &pass.combos, constants: &constants(pass), buffers: &[] },
        bindings,
    )?;

    // The picture: its size and how much of its texture it fills.
    let picture = base.samplers.iter().find(|(_, unit, _)| *unit == 0).and_then(|(_, _, source)| match source {
        TextureRef::Shared(texture) => Some(texture.clone()),
        _ => None,
    });
    let (picture_size, extent) = match &picture {
        Some(texture) if texture.sprites.is_some() => {
            let frame = texture.sprites.as_ref().and_then(|s| s.frames.first().copied());
            (frame.map(|f| [f.width.abs().max(f.skew.1.abs()), f.height.abs().max(f.skew.0.abs())]), [1.0, 1.0])
        }
        Some(texture) => {
            let r = texture.resolution;
            (Some([r[2], r[3]]), [(r[2] / r[0]).min(1.0), (r[3] / r[1]).min(1.0)])
        }
        None => (None, [1.0, 1.0]),
    };
    let size = if mode == ImageMode::Fullscreen { scene_size } else { image.size.or(model.size).or(picture_size).unwrap_or(scene_size) };

    // Where the origin sits on the layer.
    let (w, h) = (size[0], size[1]);
    let (ax, ay) = (-f32::from(image.alignment.horizontal) * w / 2.0, -f32::from(image.alignment.vertical) * h / 2.0);
    let (x0, x1, y0, y1) = (ax - w / 2.0, ax + w / 2.0, ay - h / 2.0, ay + h / 2.0);
    let [u, v] = extent;
    let gl = &engine.gl;
    let mesh = |corners: &[[f32; 5]; 4]| Mesh::quad(gl, corners).map_err(|e| e.to_string());
    let quad = mesh(&[[x0, y0, 0.0, 0.0, v], [x1, y0, 0.0, u, v], [x0, y1, 0.0, 0.0, 0.0], [x1, y1, 0.0, u, 0.0]])?;
    let quad_unit = mesh(&[[x0, y0, 0.0, 0.0, 1.0], [x1, y0, 0.0, 1.0, 1.0], [x0, y1, 0.0, 0.0, 0.0], [x1, y1, 0.0, 1.0, 0.0]])?;
    let base_quad = mesh(&[[-1.0, -1.0, 0.0, 0.0, 0.0], [1.0, -1.0, 0.0, u, 0.0], [-1.0, 1.0, 0.0, 0.0, v], [1.0, 1.0, 0.0, u, v]])?;

    let mut buffers: Vec<BufferSpec> = Vec::new();
    let mut effects = Vec::new();
    for effect_use in &image.effects {
        match build_effect(engine, files, bindings, effect_use, &mut buffers) {
            Ok(steps) => effects.push(Effect { name: effect_use.name.clone(), visible: effect_use.visible.value, steps }),
            Err(why) => {
                // Without the effect the chain still joins up: the layer shows as if it were off.
                log::warn!("effect {} is skipped: {why}", effect_use.file);
                effects.push(Effect { name: effect_use.name.clone(), visible: false, steps: Vec::new() });
            }
        }
    }
    let _ = index;

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

fn build_effect(engine: &mut Engine, files: &Files, bindings: &mut Bindings, effect_use: &crate::doc::EffectUse, buffers: &mut Vec<BufferSpec>) -> Result<Vec<Step>, String> {
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
            steps.push(Step::Copy { source: buffer_slot(buffers, source, 1.0), target: buffer_slot(buffers, target, 1.0) });
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
            for (slot, texture) in overrides.textures.iter().enumerate() {
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
        for bind in &pass.binds {
            if textures.len() <= bind.slot {
                textures.resize(bind.slot + 1, TextureChoice::Default);
            }
            textures[bind.slot] = match bind.name.as_str() {
                "previous" => TextureChoice::Previous,
                name if buffers.iter().any(|b| b.name == name) => TextureChoice::Buffer(buffer_slot(buffers, name, 1.0)),
                name => TextureChoice::Named(name.to_owned()),
            };
        }
        // An effect works on the layer's picture unless it says otherwise.
        if textures.is_empty() {
            textures.push(TextureChoice::Previous);
        } else if textures[0] == TextureChoice::Default {
            textures[0] = TextureChoice::Previous;
        }

        let names: Vec<String> = buffers.iter().map(|b| b.name.clone()).collect();
        let shade =
            Shade::compile(&mut engine.share, files, &PassInput { shader: &base.shader, textures: &textures, combos: &combos, constants: &values, buffers: &names }, bindings)?;
        let target = pass.target.as_ref().map(|name| buffer_slot(buffers, name, 1.0));
        steps.push(Step::Draw { shade, target });
    }
    Ok(steps)
}
