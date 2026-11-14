//! Drawing a frame.

use crate::gpu::{Blend, Gl, Target, Texture, TextureFormat};
use crate::math::Mat4;
use crate::{Engine, Fill};

use super::shade::{Feed, Shade, TextureRef};
use super::{FrameInput, ImageLayer, ImageMode, Layer, LayerKind, SceneInstance, Step};

/// What is the same for every pass of a frame.
struct Frame<'a> {
    time: f32,
    daytime: f32,
    pointer: [f32; 2],
    screen: [f32; 2],
    view_projection: Mat4,
    values: &'a [Vec<f32>],
    /// The pointer as the parallax sees it.
    eased_pointer: [f32; 2],
    /// What the pointer's place adds to every layer's position, before its depth.
    parallax: Option<([f32; 2], f32)>,
    scene_center: [f32; 2],
    audio: &'a crate::audio::Spectrum,
    /// Where spectrum bands are gathered before they go to a uniform.
    scratch: std::cell::RefCell<Vec<f32>>,
}

/// What is the same for every pass of a layer.
struct LayerState {
    mvp: Mat4,
    model: Mat4,
    color: [f32; 3],
    alpha: f32,
    brightness: f32,
    /// The size shaders are told the layer's buffers have: the authored one.
    authored: [f32; 2],
}

/// Where the frame is drawn: a framebuffer (or the current surface) and its size.
#[derive(Clone, Copy)]
struct Canvas {
    framebuffer: Option<glow::Framebuffer>,
    size: (u32, u32),
}

impl Canvas {
    fn bind(&self, gl: &Gl) {
        gl.bind_framebuffer(self.framebuffer, self.size);
    }
}

/// Finished pictures of layers: the layer's id, the buffer, and the size shaders are told it has.
type Pictures = Vec<(i64, Target, [f32; 2])>;

/// The buffers a layer holds while its chain runs.
struct Chain {
    ping: [Option<Target>; 2],
    current: Option<usize>,
    named: Vec<Option<Target>>,
}

impl SceneInstance {
    /// Draws the frame for `input` into `to` (`None`: the current surface) of
    /// `size` pixels. The scene has no buffer of its own: layers go straight
    /// to where the frame is wanted.
    pub fn render(&mut self, engine: &mut Engine, input: &FrameInput, to: Option<glow::Framebuffer>, size: (u32, u32)) {
        self.viewport = crate::Viewport { width: size.0.max(1), height: size.1.max(1) };
        let size = (self.viewport.width, self.viewport.height);
        // A scene that blooms is drawn into a buffer first: the bloom reads the finished frame.
        let staged = match self.bloom.as_ref().filter(|bloom| bloom.enabled) {
            Some(_) => engine.pool.take(size.0, size.1, TextureFormat::Rgba8).inspect_err(|e| log::warn!("bloom: {e}")).ok(),
            None => None,
        };
        let canvas = Canvas { framebuffer: staged.as_ref().map_or(to, |target| Some(target.framebuffer())), size };
        for video in &self.videos {
            video.stream.borrow_mut().poll();
        }
        let dt = (input.time - self.last_time).clamp(0.0, 0.25) as f32;
        self.last_time = input.time;
        if !self.bindings.animations.is_empty() {
            let mut changes = Vec::new();
            self.bindings.animate(dt, &mut changes);
            for change in changes {
                self.apply(change);
            }
        }
        self.update_transforms();
        // The parallax camera trails the pointer.
        if let Some(parallax) = &self.parallax {
            let ease = if parallax.delay > 0.0 { (dt / parallax.delay).min(1.0) } else { 1.0 };
            for axis in 0..2 {
                self.eased_pointer[axis] += (input.pointer[axis] - self.eased_pointer[axis]) * ease;
            }
        }
        self.view_projection = self.camera(input.time as f32);
        // A copy: the frame borrows it while the engine is being drawn with.
        let spectrum = engine.listener.spectrum().clone();
        let frame = Frame {
            time: input.time as f32,
            daytime: input.daytime,
            pointer: input.pointer,
            screen: [self.viewport.width as f32, self.viewport.height as f32],
            view_projection: self.view_projection,
            values: &self.bindings.values,
            eased_pointer: self.eased_pointer,
            // The pointer counts from the top left; the scene's y points up.
            parallax: self.parallax.map(|p| {
                let pointer = [(0.5 - self.eased_pointer[0]) * self.scene_size[0], (self.eased_pointer[1] - 0.5) * self.scene_size[1]];
                ([pointer[0] * p.mouse_influence, pointer[1] * p.mouse_influence], p.amount)
            }),
            scene_center: [self.scene_size[0] / 2.0, self.scene_size[1] / 2.0],
            audio: &spectrum,
            scratch: std::cell::RefCell::new(Vec::with_capacity(64)),
        };
        canvas.bind(&engine.gl);
        engine.clear([self.clear[0], self.clear[1], self.clear[2], 1.0]);
        // How many output pixels one scene unit covers: layer buffers are sized by it.
        let density = (frame.view_projection.0[0] * frame.screen[0] / 2.0).abs().max((frame.view_projection.0[5] * frame.screen[1] / 2.0).abs());
        // Text that changed is drawn into its texture before anything samples it.
        for layer in &mut self.layers {
            if let LayerKind::Image(image) = &mut layer.kind
                && let Some(text) = &mut image.text
                && layer.shown
            {
                let reach = density * layer.world.0[0].abs().max(layer.world.0[5].abs());
                text.refresh(&engine.gl, reach.min(2.0));
            }
        }
        let mut background: Option<Target> = None;
        // The finished pictures of layers that others read, kept until the frame is done.
        let mut pictures: Pictures = Vec::new();
        for layer in &self.layers {
            if !layer.shown {
                continue;
            }
            if let LayerKind::Image(image) = &layer.kind {
                let keep = self.provides.contains(&layer.id);
                draw_image(engine, &frame, layer, image, canvas, &mut background, &mut pictures, keep, density);
            }
        }
        if let (Some(staged), Some(bloom)) = (&staged, &self.bloom) {
            let flat = LayerState { mvp: Mat4::IDENTITY, model: Mat4::IDENTITY, color: [1.0; 3], alpha: 1.0, brightness: 1.0, authored: frame.screen };
            let run = |engine: &mut Engine, shade: &super::shade::Shade, input: &Texture, extra: Option<&Texture>| {
                engine.gl.set_blend(Blend::None);
                apply(&engine.gl, shade, &frame, &flat, Some(input), &[extra], None, &pictures, engine.white());
                engine.fullscreen.draw();
            };
            let small = |divisor: u32| ((size.0 / divisor).max(1), (size.1 / divisor).max(1));
            let targets = (
                engine.pool.take(small(4).0, small(4).1, TextureFormat::Rgba8),
                engine.pool.take(small(8).0, small(8).1, TextureFormat::Rgba8),
                engine.pool.take(small(8).0, small(8).1, TextureFormat::Rgba8),
            );
            if let (Ok(quarter), Ok(across), Ok(down)) = targets {
                quarter.bind();
                run(engine, &bloom.bright, &staged.texture, None);
                across.bind();
                run(engine, &bloom.across, &quarter.texture, None);
                down.bind();
                run(engine, &bloom.down, &across.texture, None);
                engine.gl.bind_framebuffer(to, size);
                run(engine, &bloom.combine, &staged.texture, Some(&down.texture));
                for target in [quarter, across, down] {
                    engine.pool.give(target);
                }
            } else {
                // No memory for the bloom's buffers: the frame without it.
                staged.blit(to, size);
            }
        }
        for target in background.into_iter().chain(pictures.into_iter().map(|(_, target, _)| target)).chain(staged) {
            engine.pool.give(target);
        }
        engine.pool.end_frame();
    }

    /// Recomputes every layer's place in the scene from its own values and its parents'.
    fn update_transforms(&mut self) {
        for index in 0..self.layers.len() {
            let layer = &self.layers[index];
            let local = Mat4::translation(layer.origin).mul(&Mat4::rotation(layer.angles)).mul(&Mat4::scaling(layer.scale));
            // Parents come before their children in a scene; one that does not is ignored.
            let parent = layer.parent.filter(|&p| p < index).map(|p| (self.layers[p].world, self.layers[p].shown));
            let (world, shown) = match parent {
                Some((parent_world, parent_shown)) => (parent_world.mul(&local), parent_shown && layer.visible),
                None => (local, layer.visible),
            };
            let layer = &mut self.layers[index];
            layer.world = world;
            layer.shown = shown;
        }
    }

    /// The projection that puts the scene on the output the way `fill` says.
    fn camera(&self, time: f32) -> Mat4 {
        let [scene_w, scene_h] = self.scene_size;
        let (out_w, out_h) = (self.viewport.width as f32, self.viewport.height as f32);
        let (sx, sy) = (out_w / scene_w, out_h / scene_h);
        // The part of the scene that is shown, around its centre.
        let (half_w, half_h) = match self.fill {
            Fill::Stretch => (scene_w / 2.0, scene_h / 2.0),
            Fill::Cover => (out_w / sx.max(sy) / 2.0, out_h / sx.max(sy) / 2.0),
            Fill::Contain => (out_w / sx.min(sy) / 2.0, out_h / sx.min(sy) / 2.0),
        };
        let (half_w, half_h) = (half_w / self.zoom, half_h / self.zoom);
        let (mut cx, mut cy) = (scene_w / 2.0, scene_h / 2.0);
        if let Some(shake) = &self.shake {
            // Two sines an axis at unrelated rates wander without repeating visibly;
            // roughness adds the faster one.
            let t = time * shake.speed;
            let wander = |a: f32, b: f32| (t * a).sin() + shake.roughness * 0.5 * (t * b + 1.7).sin();
            let reach = shake.amplitude * scene_h * 0.01;
            cx += wander(1.0, 2.9) * reach;
            cy += wander(1.3, 3.7) * reach;
        }
        Mat4::orthographic(cx - half_w, cx + half_w, cy - half_h, cy + half_h, -10000.0, 10000.0)
    }
}

/// Draws one image layer: its base pass, then each visible effect's steps, the last
/// of them straight into the scene.
#[allow(clippy::too_many_arguments)]
fn draw_image(
    engine: &mut Engine,
    frame: &Frame<'_>,
    layer: &Layer,
    image: &ImageLayer,
    scene: Canvas,
    background: &mut Option<Target>,
    pictures: &mut Pictures,
    provides: bool,
    density: f32,
) {
    let keep = provides;
    // Parallax moves a layer by how far it is from the camera's centre and from
    // the pointer, scaled by its depth: near layers travel further than far ones.
    let world = match frame.parallax {
        Some((pointer, amount)) if layer.parallax_depth != [0.0, 0.0] => {
            let from_center = [layer.world.0[12] - frame.scene_center[0], layer.world.0[13] - frame.scene_center[1]];
            let shift = [(from_center[0] + pointer[0]) * layer.parallax_depth[0] * amount, (from_center[1] + pointer[1]) * layer.parallax_depth[1] * amount];
            Mat4::translation([shift[0], shift[1], 0.0]).mul(&layer.world)
        }
        _ => layer.world,
    };
    let state = LayerState {
        mvp: frame.view_projection.mul(&world),
        model: world,
        color: image.color,
        alpha: image.alpha,
        brightness: image.brightness,
        authored: image.size,
    };
    let steps: Vec<&Step> = image.effects.iter().filter(|e| e.visible).flat_map(|e| &e.steps).collect();
    if steps.is_empty() && image.mode != ImageMode::Normal {
        return; // A compose layer without effects changes nothing.
    }

    let needs_background =
        image.mode != ImageMode::Normal || image.blend_pass.is_some() || image.base.wants_background || steps.iter().any(|s| matches!(s, Step::Draw { shade, .. } if shade.wants_background));
    if needs_background {
        let copy = match background.take() {
            Some(copy) => copy,
            None => match engine.pool.take(scene.size.0, scene.size.1, TextureFormat::Rgba8) {
                Ok(copy) => copy,
                Err(error) => {
                    log::warn!("layer {}: {error}", layer.name);
                    return;
                }
            },
        };
        engine.gl.copy_framebuffer(scene.framebuffer, scene.size, Some(copy.framebuffer()), scene.size);
        *background = Some(copy);
    }
    let background_texture = background.as_ref().map(|t| &t.texture);

    // A layer with a blend mode, or one others read, always ends in a buffer.
    let keep = keep || image.blend_pass.is_some();
    if steps.is_empty() && !keep {
        scene.bind(&engine.gl);
        engine.gl.set_blend(image.blend);
        apply(&engine.gl, &image.base, frame, &state, None, &[], background_texture, pictures, engine.white());
        image.quad.draw();
        return;
    }

    // The layer's buffers: as large as the layer reaches the output, never larger than authored.
    let reach = density * world.0[0].abs().max(world.0[5].abs()).max(1e-6);
    let scale = reach.min(1.0);
    let size = match image.mode {
        ImageMode::Fullscreen => scene.size,
        _ => (((image.size[0] * scale).ceil() as u32).max(1), ((image.size[1] * scale).ceil() as u32).max(1)),
    };
    let mut chain = Chain { ping: [None, None], current: None, named: image.buffers.iter().map(|_| None).collect() };
    let fullscreen = image.mode == ImageMode::Fullscreen;

    // The base pass fills the first buffer.
    let ok = (|| -> Result<(), crate::gpu::GpuError> {
        let first = engine.pool.take(size.0, size.1, TextureFormat::Rgba8)?;
        first.bind();
        engine.gl.set_blend(Blend::None);
        match image.mode {
            // The compose shader places itself by texture coordinate and reads the scene by position.
            ImageMode::Compose => {
                apply(&engine.gl, &image.base, frame, &state, None, &[], background_texture, pictures, engine.white());
                image.quad_unit.draw();
            }
            ImageMode::Fullscreen => {
                let flat = LayerState { mvp: Mat4::IDENTITY, ..copy_state(&state) };
                apply(&engine.gl, &image.base, frame, &flat, None, &[], background_texture, pictures, engine.white());
                engine.fullscreen.draw();
            }
            ImageMode::Normal => {
                let flat = LayerState { mvp: Mat4::IDENTITY, ..copy_state(&state) };
                apply(&engine.gl, &image.base, frame, &flat, None, &[], background_texture, pictures, engine.white());
                image.base_quad.draw();
            }
        }
        chain.ping[0] = Some(first);
        chain.current = Some(0);

        let mut landed = false;
        for (index, step) in steps.iter().enumerate() {
            let last = index + 1 == steps.len();
            match step {
                Step::Copy { source, target } => {
                    ensure_named(engine, &mut chain, image, *target, size)?;
                    if let (Some(from), Some(to)) = (chain.named[*source].as_ref(), chain.named[*target].as_ref()) {
                        from.copy_to(to);
                    }
                }
                Step::Draw { shade, target } => {
                    let to_scene = last && target.is_none() && !keep;
                    let next = 1 - chain.current.unwrap_or(1);
                    if to_scene {
                        scene.bind(&engine.gl);
                        engine.gl.set_blend(image.blend);
                    } else if let Some(slot) = target {
                        ensure_named(engine, &mut chain, image, *slot, size)?;
                        chain.named[*slot].as_ref().expect("just ensured").bind();
                        engine.gl.set_blend(Blend::None);
                    } else {
                        if chain.ping[next].is_none() {
                            chain.ping[next] = Some(engine.pool.take(size.0, size.1, TextureFormat::Rgba8)?);
                        }
                        chain.ping[next].as_ref().expect("just ensured").bind();
                        engine.gl.set_blend(Blend::None);
                    }
                    let previous = chain.current.and_then(|c| chain.ping[c].as_ref()).map(|t| &t.texture);
                    // A named target may be one this very pass reads: GL forbids that, so it reads white instead.
                    let reading: Vec<Option<&Texture>> =
                        chain.named.iter().enumerate().map(|(i, t)| if Some(i) == *target { None } else { t.as_ref().map(|t| &t.texture) }).collect();
                    if to_scene && !fullscreen {
                        apply(&engine.gl, shade, frame, &state, previous, &reading, background_texture, pictures, engine.white());
                        image.quad_unit.draw();
                    } else {
                        let flat = LayerState { mvp: Mat4::IDENTITY, ..copy_state(&state) };
                        apply(&engine.gl, shade, frame, &flat, previous, &reading, background_texture, pictures, engine.white());
                        engine.fullscreen.draw();
                    }
                    if to_scene {
                        landed = true;
                    } else if target.is_none() {
                        chain.current = Some(next);
                    }
                }
            }
        }
        // The chain ended in a buffer: put its picture into the scene.
        if !landed && let Some(result) = chain.current.and_then(|c| chain.ping[c].as_ref()) {
            scene.bind(&engine.gl);
            match &image.blend_pass {
                // The blend shader reads the frame behind the layer and writes the mix.
                Some(blend) if !fullscreen => {
                    engine.gl.set_blend(Blend::None);
                    apply(&engine.gl, blend, frame, &state, Some(&result.texture), &[], background_texture, pictures, engine.white());
                    image.quad_unit.draw();
                }
                _ => {
                    engine.gl.set_blend(image.blend);
                    let mvp = if fullscreen { Mat4::IDENTITY } else { state.mvp };
                    engine.composite(&result.texture, &mvp, 1.0);
                    if fullscreen { engine.fullscreen.draw() } else { image.quad_unit.draw() }
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = &ok {
        log::warn!("layer {}: {error}", layer.name);
    }
    // A layer others read keeps its finished picture for the rest of the frame.
    if provides
        && ok.is_ok()
        && let Some(index) = chain.current
        && let Some(picture) = chain.ping[index].take()
    {
        pictures.push((layer.id, picture, image.size));
    }
    for target in chain.ping.into_iter().flatten().chain(chain.named.into_iter().flatten()) {
        engine.pool.give(target);
    }
}

fn copy_state(state: &LayerState) -> LayerState {
    LayerState { mvp: state.mvp, model: state.model, color: state.color, alpha: state.alpha, brightness: state.brightness, authored: state.authored }
}

/// Makes sure the layer's named buffer `slot` exists.
fn ensure_named(engine: &mut Engine, chain: &mut Chain, image: &ImageLayer, slot: usize, size: (u32, u32)) -> Result<(), crate::gpu::GpuError> {
    if chain.named[slot].is_none() {
        let scale = image.buffers[slot].scale.max(1.0);
        let (w, h) = (((size.0 as f32 / scale).ceil() as u32).max(1), ((size.1 as f32 / scale).ceil() as u32).max(1));
        chain.named[slot] = Some(engine.pool.take(w, h, TextureFormat::Rgba8)?);
    }
    Ok(())
}

/// Binds a pass's program and textures and sets every uniform it has.
#[allow(clippy::too_many_arguments)]
fn apply(
    gl: &Gl,
    shade: &Shade,
    frame: &Frame<'_>,
    state: &LayerState,
    previous: Option<&Texture>,
    named: &[Option<&Texture>],
    background: Option<&Texture>,
    pictures: &Pictures,
    white: &Texture,
) {
    shade.program.bind();
    // Per texture unit: the resolution shaders are told, and the sprite frame showing.
    let mut resolutions = [[1.0f32; 4]; 16];
    let mut sprites: [Option<([f32; 4], [f32; 2])>; 16] = [None; 16];
    for (uniform, unit, source) in &shade.samplers {
        let authored = [state.authored[0], state.authored[1], state.authored[0], state.authored[1]];
        let (texture, resolution) = match source {
            TextureRef::Shared(shared) => {
                if let Some(sheet) = &shared.sprites
                    && let Some(sprite) = sheet.frames.get(sheet.frame_at(frame.time))
                {
                    let (w, h) = (shared.resolution[0], shared.resolution[1]);
                    sprites[*unit as usize & 15] =
                        Some(([sprite.width / w, sprite.skew.0 / h, sprite.skew.1 / w, sprite.height / h], [sprite.x / w, sprite.y / h]));
                }
                (shared.texture.raw(), shared.resolution)
            }
            TextureRef::Previous => (previous.unwrap_or(white).raw(), authored),
            TextureRef::Buffer(slot) => (named.get(*slot).copied().flatten().unwrap_or(white).raw(), authored),
            TextureRef::Background => (background.unwrap_or(white).raw(), [frame.screen[0], frame.screen[1], frame.screen[0], frame.screen[1]]),
            TextureRef::Layer(id) => match pictures.iter().find(|(layer, ..)| layer == id) {
                Some((_, picture, size)) => (picture.texture.raw(), [size[0], size[1], size[0], size[1]]),
                // A layer drawn later, or hidden, has no picture yet.
                None => (white.raw(), authored),
            },
            TextureRef::Text(surface) => match surface.texture.borrow().as_ref() {
                Some(texture) => (texture.raw(), [texture.width as f32, texture.height as f32, texture.width as f32, texture.height as f32]),
                None => (white.raw(), authored),
            },
            TextureRef::Video(video) => {
                let video = video.borrow();
                match (video.texture(), video.size()) {
                    (Some(texture), Some((w, h))) => (texture.raw(), [w as f32, h as f32, w as f32, h as f32]),
                    // Until its first frame a video shows nothing.
                    _ => (white.raw(), authored),
                }
            }
        };
        resolutions[*unit as usize & 15] = resolution;
        gl.bind_texture(*unit, texture);
        shade.program.set_sampler(uniform, *unit);
    }
    for (uniform, feed) in &shade.feeds {
        let set = |values: &[f32]| shade.program.set(uniform, values);
        match feed {
            Feed::Const(values) => set(values),
            Feed::Value(slot) => set(frame.values.get(*slot).map_or(&[][..], Vec::as_slice)),
            Feed::Time => set(&[frame.time]),
            Feed::Daytime => set(&[frame.daytime]),
            Feed::PointerPosition => set(&frame.pointer),
            Feed::ParallaxPosition => set(&frame.eased_pointer),
            Feed::Screen => set(&[frame.screen[0], frame.screen[1], frame.screen[0] / frame.screen[1]]),
            Feed::TexelSize => set(&[1.0 / frame.screen[0], 1.0 / frame.screen[1]]),
            Feed::TexelSizeHalf => set(&[0.5 / frame.screen[0], 0.5 / frame.screen[1]]),
            Feed::Mvp => set(&state.mvp.0),
            Feed::MvpInverse => set(&state.mvp.inverse().0),
            Feed::Model => set(&state.model.0),
            Feed::ViewProjection => set(&frame.view_projection.0),
            Feed::Identity => {
                if uniform.kind == glow::FLOAT_MAT3 {
                    set(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
                } else {
                    set(&Mat4::IDENTITY.0);
                }
            }
            Feed::Resolution(unit) => set(&resolutions[*unit as usize & 15]),
            Feed::SpriteRotation(unit) => set(&sprites[*unit as usize & 15].map_or([1.0, 0.0, 0.0, 1.0], |s| s.0)),
            Feed::SpriteTranslation(unit) => set(&sprites[*unit as usize & 15].map_or([0.0, 0.0], |s| s.1)),
            Feed::Color4 => set(&[state.color[0] * state.brightness, state.color[1] * state.brightness, state.color[2] * state.brightness, state.alpha]),
            Feed::Color => set(&state.color),
            Feed::Alpha => set(&[state.alpha]),
            Feed::Brightness => set(&[state.brightness]),
            Feed::Audio { bands, right } => {
                let mut scratch = frame.scratch.borrow_mut();
                frame.audio.bands(*bands, *right, &mut scratch);
                set(&scratch);
            }
        }
    }
}

impl Engine {
    /// Fills whatever is bound with `color`.
    pub(crate) fn clear(&self, color: [f32; 4]) {
        use glow::HasContext;
        // SAFETY: plain state and a clear on the bound framebuffer.
        unsafe {
            self.gl.raw().clear_color(color[0], color[1], color[2], color[3]);
            self.gl.raw().clear(glow::COLOR_BUFFER_BIT);
        }
    }

    /// Sets up the built-in program that draws `texture` as it is; the caller draws the mesh.
    pub(crate) fn composite(&self, texture: &Texture, mvp: &Mat4, alpha: f32) {
        self.blit.bind();
        self.gl.bind_texture(0, texture.raw());
        for uniform in &self.blit.uniforms {
            match uniform.name.as_str() {
                "u_Transform" => self.blit.set(uniform, &mvp.0),
                "u_Alpha" => self.blit.set(uniform, &[alpha]),
                "u_Texture" => self.blit.set_sampler(uniform, 0),
                _ => {}
            }
        }
    }
}
