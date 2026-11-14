//! Drawing a frame.

use crate::gpu::{Blend, Depth, Gl, Target, Texture, TextureFormat};
use crate::math::Mat4;
use crate::{Engine, Fill};

use super::particles::ParticleLayer;
use super::shade::{Feed, Shade, TextureRef};
use super::{FrameInput, ImageLayer, ImageMode, Layer, LayerKind, SceneInstance, Step};

/// The largest side of a layer's own buffers, in pixels. A document can say a layer
/// is any size; its buffers stay what a GPU can be asked for.
const MAX_BUFFER_SIDE: f32 = 8192.0;

/// How far in front of a flat scene its camera is said to be.
const EYE_DISTANCE: f32 = 1000.0;

/// What is the same for every pass of a frame.
pub(super) struct Frame<'a> {
    time: f32,
    daytime: f32,
    pointer: [f32; 2],
    pub(super) screen: [f32; 2],
    pub(super) view_projection: Mat4,
    values: &'a [Vec<f32>],
    /// The pointer as the parallax sees it.
    eased_pointer: [f32; 2],
    /// What the pointer's place adds to every layer's position, before its depth.
    parallax: Option<([f32; 2], f32)>,
    scene_center: [f32; 2],
    audio: &'a crate::audio::Spectrum,
    /// Where spectrum bands are gathered before they go to a uniform.
    scratch: std::cell::RefCell<Vec<f32>>,
    /// Where the camera is, and its right, up and the way it looks.
    eye: [f32; 3],
    axes: [[f32; 3]; 3],
    /// The scene's light from above and from below, and its lights.
    ambient: [f32; 3],
    skylight: [f32; 3],
    lights: &'a super::lights::Lights,
    /// The scene behind the first layer that reflects it, with its smaller levels.
    reflection: std::cell::RefCell<Option<Target>>,
    /// What the frame and the layers' buffers are kept in: half floats in a scene drawn
    /// in a high dynamic range, bytes in any other.
    format: TextureFormat,
}

/// What is the same for every pass of a layer.
pub(super) struct LayerState {
    pub(super) mvp: Mat4,
    pub(super) model: Mat4,
    pub(super) color: [f32; 3],
    pub(super) alpha: f32,
    pub(super) brightness: f32,
    /// The size shaders are told the layer's buffers have: the authored one.
    pub(super) authored: [f32; 2],
    /// `g_RenderVar0` and `g_RenderVar1`, for particles.
    pub(super) vars: [[f32; 4]; 2],
}

/// Where the frame is drawn: a framebuffer (or the current surface) and its size.
#[derive(Clone, Copy)]
pub(super) struct Canvas {
    framebuffer: Option<glow::Framebuffer>,
    size: (u32, u32),
}

impl Canvas {
    pub(super) fn bind(&self, gl: &Gl) {
        gl.bind_framebuffer(self.framebuffer, self.size);
    }
}

/// Finished pictures of layers: the layer's id, the buffer, and the size shaders are told it has.
pub(super) type Pictures = Vec<(i64, Target, [f32; 2])>;

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
    pub fn render(
        &mut self,
        engine: &mut Engine,
        input: &FrameInput,
        to: Option<glow::Framebuffer>,
        size: (u32, u32),
    ) {
        self.viewport = crate::Viewport { width: size.0.max(1), height: size.1.max(1) };
        let size = (self.viewport.width, self.viewport.height);
        // A scene that blooms is drawn into a buffer first: the bloom reads the finished
        // frame. A 3D scene is too: it needs a depth buffer, and the surface has none. And
        // so is a scene in a high dynamic range: the surface keeps nothing over one.
        let deep = self.perspective.is_some();
        let format = if self.hdr { super::hdr::FORMAT } else { TextureFormat::Rgba8 };
        let staged = if deep {
            engine
                .pool
                .take_with_depth(size.0, size.1, format)
                .inspect_err(|e| log::warn!("3D scene: {e}"))
                .ok()
        } else if self.hdr || self.bloom.as_ref().is_some_and(|bloom| bloom.enabled) {
            engine.pool.take(size.0, size.1, format).inspect_err(|e| log::warn!("bloom: {e}")).ok()
        } else {
            None
        };
        let canvas =
            Canvas { framebuffer: staged.as_ref().map_or(to, |target| Some(target.framebuffer())), size };
        for video in &self.videos {
            video.stream.borrow_mut().poll();
        }
        let before = self.last_time;
        let dt = (input.time - before).clamp(0.0, 0.25) as f32;
        self.last_time = input.time;

        // What the timelines passed this frame, for the scripts on the same values.
        let mut passed = Vec::new();
        if !self.bindings.animations.is_empty() {
            let mut changes = Vec::new();
            self.bindings.animate(dt, &mut changes, &mut passed);
            for change in changes {
                self.apply(change);
            }
        }
        #[cfg(feature = "script")]
        self.run_scripts(engine, input, dt, &passed);
        #[cfg(not(feature = "script"))]
        let _ = passed;
        self.play_sounds();
        self.update_transforms();
        // Puppets play on: this frame's bones go into the value table shaders read.
        let stretch = (input.time - before).max(0.0) as f32;
        for layer in &mut self.layers {
            if !layer.shown && !layer.read {
                continue;
            }
            match &mut layer.kind {
                LayerKind::Image(image) => {
                    if let Some(puppet) = &mut image.puppet {
                        puppet.update(stretch, &mut self.bindings.values);
                    }
                }
                LayerKind::Model(model) => model.update(stretch, &mut self.bindings.values),
                _ => {}
            }
        }
        // What hangs from a bone follows the pose of this frame.
        if self.attached {
            self.update_transforms();
        }
        if self.lights.count() > 0 {
            self.lights.update(&self.layers, &self.bindings.values);
        }
        // The parallax camera trails the pointer.
        if self.parallax.enabled {
            let parallax = &self.parallax;
            let ease = if parallax.delay > 0.0 { (dt / parallax.delay).min(1.0) } else { 1.0 };
            for axis in 0..2 {
                self.eased_pointer[axis] += (input.pointer[axis] - self.eased_pointer[axis]) * ease;
            }
        }
        // A camera object takes the camera where it is this frame.
        if let (Some(camera), Some(rig)) = (&mut self.perspective, self.camera_layer)
            && let Some(layer) = self.layers.get(rig.index)
        {
            *camera = rig.place(&layer.world, camera.near, camera.far);
        }
        self.view = self.perspective.map(|camera| self.perspective_view(&camera, input.time as f32));
        self.view_projection = match &self.view {
            Some(view) => view.view_projection,
            None => self.camera(input.time as f32),
        };
        if self.frame != crate::Frame::default() {
            self.view_projection = self.frame.matrix().mul(&self.view_projection);
        }
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
            // The pointer counts from the top left; the scene's y points up. A 3D scene's
            // camera moves itself.
            parallax: Some(self.parallax).filter(|p| p.enabled && !deep).map(|p| {
                let pointer = [
                    (0.5 - self.eased_pointer[0]) * self.scene_size[0],
                    (self.eased_pointer[1] - 0.5) * self.scene_size[1],
                ];
                ([pointer[0] * p.mouse_influence, pointer[1] * p.mouse_influence], p.amount)
            }),
            scene_center: [self.scene_size[0] / 2.0, self.scene_size[1] / 2.0],
            audio: &spectrum,
            scratch: std::cell::RefCell::new(Vec::with_capacity(64)),
            // A flat scene is seen head on, from in front of its centre.
            eye: self
                .view
                .map_or([self.scene_size[0] / 2.0, self.scene_size[1] / 2.0, EYE_DISTANCE], |view| view.eye),
            axes: self.view.map_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], |view| {
                [view.right, view.up, view.forward.map(|n| -n)]
            }),
            ambient: self.ambient,
            skylight: self.skylight,
            lights: &self.lights,
            reflection: std::cell::RefCell::new(None),
            format,
        };
        canvas.bind(&engine.gl);
        if deep && staged.is_some() {
            engine.gl.clear_with_depth([self.clear[0], self.clear[1], self.clear[2], 1.0]);
        } else {
            engine.clear([self.clear[0], self.clear[1], self.clear[2], 1.0]);
        }
        // How many output pixels one scene unit covers: layer buffers are sized by it.
        let density = (frame.view_projection.0[0] * frame.screen[0] / 2.0)
            .abs()
            .max((frame.view_projection.0[5] * frame.screen[1] / 2.0).abs());
        // Text that changed is drawn into its texture before anything samples it.
        for layer in &mut self.layers {
            if let LayerKind::Image(image) = &mut layer.kind
                && let Some(text) = &mut image.text
                && (layer.shown || layer.read)
                && text.dirty
            {
                // Other text takes another box: the layer is as large as its text.
                if text.refit() {
                    match super::build::quads(engine, text.size, text.offset(), [1.0, 1.0]) {
                        Ok((quad, quad_unit, base_quad)) => {
                            (image.quad, image.quad_unit, image.base_quad) = (quad, quad_unit, base_quad);
                            image.size = text.size;
                        }
                        Err(error) => log::warn!("text layer {}: {error}", layer.name),
                    }
                }
                let reach = density * layer.world.0[0].abs().max(layer.world.0[5].abs());
                text.refresh(&engine.gl, reach.min(2.0));
            }
        }
        // Particles live on, and this frame's reach the GPU, before anything is drawn.
        let pointer = self.pointer_in_scene(input.pointer);
        for layer in &mut self.layers {
            if let LayerKind::Particles(particles) = &mut layer.kind
                && layer.shown
            {
                particles.update(&engine.gl, stretch, input.time as f32, &layer.world, Some(pointer));
            }
        }
        let mut background: Option<Target> = None;
        // The finished pictures of layers that others read, kept until the frame is done.
        let mut pictures: Pictures = Vec::new();
        for layer in self.order.iter().filter_map(|index| self.layers.get(*index)) {
            if !layer.shown && !layer.read {
                continue;
            }
            if deep {
                // Flat things in a 3D scene are hidden by what is nearer, and hide nothing.
                engine.gl.set_depth(Depth { test: true, write: false });
                engine.gl.set_cull(false);
            }
            if !self.reflects.is_empty() && self.reflects.contains(&layer.id) {
                reflect(engine, &frame, canvas);
            }
            match &layer.kind {
                LayerKind::Image(image) => {
                    let keep = self.provides.contains(&layer.id);
                    let density = match &self.view {
                        Some(view) => view.pixels_per_unit(
                            [layer.world.0[12], layer.world.0[13], layer.world.0[14]],
                            frame.screen[1],
                        ),
                        None => density,
                    };
                    draw_image(
                        engine,
                        &frame,
                        layer,
                        image,
                        canvas,
                        &mut background,
                        &mut pictures,
                        keep,
                        density,
                    );
                }
                LayerKind::Particles(particles) if layer.shown => {
                    draw_particles(engine, &frame, layer, particles, canvas, &pictures);
                }
                LayerKind::Particles(_) => {}
                LayerKind::Model(model) => {
                    model.draw(
                        engine,
                        &frame,
                        &layer.world,
                        canvas,
                        background.as_ref().map(|t| &t.texture),
                        &pictures,
                    );
                }
                LayerKind::Sound(_) | LayerKind::Group => {}
            }
        }
        if deep {
            // What comes after, and other scenes after this one, draw flat again.
            engine.gl.set_depth(Depth::default());
            engine.gl.set_cull(false);
        }
        if let (Some(staged), Some(bloom)) = (&staged, self.bloom.as_ref().filter(|bloom| bloom.enabled)) {
            let flat = LayerState {
                mvp: Mat4::IDENTITY,
                model: Mat4::IDENTITY,
                color: [1.0; 3],
                alpha: 1.0,
                brightness: 1.0,
                authored: frame.screen,
                vars: [[0.0; 4]; 2],
            };
            let run = |engine: &mut Engine,
                       shade: &super::shade::Shade,
                       input: &Texture,
                       extra: Option<&Texture>| {
                engine.gl.set_blend(Blend::None);
                apply(
                    &engine.gl,
                    shade,
                    &frame,
                    &flat,
                    Some(input),
                    &[extra],
                    None,
                    &pictures,
                    engine.white(),
                );
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
        } else if let (Some(staged), Some(bloom)) =
            (&staged, self.hdr_bloom.as_ref().filter(|bloom| bloom.enabled))
        {
            bloom.finish(engine, &frame, staged, to, size);
        } else if let Some(staged) = &staged {
            // Out of half floats, a copy clamps to what the output holds.
            staged.blit(to, size);
        }
        let reflection = frame.reflection.into_inner();
        for target in background
            .into_iter()
            .chain(pictures.into_iter().map(|(_, target, _)| target))
            .chain(staged)
            .chain(reflection)
        {
            engine.pool.give(target);
        }
        engine.pool.end_frame();
    }

    /// Where a point of the output (0 to 1 from the top left) is in the scene, as the
    /// camera showed it last: a scene that is cropped or zoomed to fill an output does
    /// not start in the output's corner.
    pub(super) fn pointer_in_scene(&self, pointer: [f32; 2]) -> [f32; 2] {
        // The pointer counts from the top left; the scene's y points up.
        let clip = [pointer[0] * 2.0 - 1.0, 1.0 - pointer[1] * 2.0, 0.0];
        if self.view_projection == Mat4::IDENTITY {
            // Nothing was drawn yet: the scene as if it filled the output exactly.
            return [pointer[0] * self.scene_size[0], (1.0 - pointer[1]) * self.scene_size[1]];
        }
        let at = self.view_projection.inverse().transform_point(clip);
        [at[0], at[1]]
    }

    /// Recomputes every layer's place in the scene from its own values and its parents'.
    fn update_transforms(&mut self) {
        for index in 0..self.layers.len() {
            let layer = &self.layers[index];
            let local = Mat4::translation(layer.origin)
                .mul(&Mat4::rotation(layer.angles))
                .mul(&Mat4::scaling(layer.scale));
            // Parents come before their children in a scene; one that does not is ignored.
            let parent = layer.parent.filter(|&p| p < index).map(|p| {
                let parent = &self.layers[p];
                let world = match (layer.attachment, &parent.kind) {
                    (Some(at), LayerKind::Model(model)) => {
                        model.places.get(at).map_or(parent.world, |place| parent.world.mul(place))
                    }
                    _ => parent.world,
                };
                (world, parent.shown)
            });
            let (world, shown) = match parent {
                Some((parent_world, parent_shown)) => {
                    (parent_world.mul(&local), parent_shown && layer.visible)
                }
                None => (local, layer.visible),
            };
            let layer = &mut self.layers[index];
            layer.world = world;
            layer.shown = shown;
            layer.read = false;
        }
        // A hidden layer that is read may itself read another: until nothing changes.
        let mut changed = !self.reads.is_empty();
        while changed {
            changed = false;
            for &(reader, read) in &self.reads {
                let wanted = self.layers[reader].shown || self.layers[reader].read;
                let layer = &mut self.layers[read];
                if wanted && !layer.shown && !layer.read {
                    layer.read = true;
                    changed = true;
                }
            }
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
        if let Some(shake) = Some(&self.shake).filter(|shake| shake.enabled) {
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

/// Where a layer is this frame: its place in the scene, moved by the parallax.
///
/// Parallax moves a layer by how far it is from the camera's centre and from the
/// pointer, scaled by its depth: near layers travel further than far ones.
fn placed(frame: &Frame<'_>, layer: &Layer) -> Mat4 {
    match frame.parallax {
        Some((pointer, amount)) if layer.parallax_depth != [0.0, 0.0] => {
            let from_center =
                [layer.world.0[12] - frame.scene_center[0], layer.world.0[13] - frame.scene_center[1]];
            let shift = [
                (from_center[0] + pointer[0]) * layer.parallax_depth[0] * amount,
                (from_center[1] + pointer[1]) * layer.parallax_depth[1] * amount,
            ];
            Mat4::translation([shift[0], shift[1], 0.0]).mul(&layer.world)
        }
        _ => layer.world,
    }
}

/// Copies the scene drawn so far for the reflections of this frame, once: at half its
/// size, with every smaller level made from it, so a rough surface reads a blurred one.
fn reflect(engine: &mut Engine, frame: &Frame<'_>, canvas: Canvas) {
    if frame.reflection.borrow().is_some() {
        return;
    }
    let (width, height) = ((canvas.size.0 / 2).max(1), (canvas.size.1 / 2).max(1));
    match engine.pool.take_mipmapped(width, height, frame.format) {
        Ok(copy) => {
            engine.gl.copy_framebuffer(
                canvas.framebuffer,
                canvas.size,
                Some(copy.framebuffer()),
                (width, height),
            );
            copy.generate_mipmaps();
            *frame.reflection.borrow_mut() = Some(copy);
        }
        Err(error) => log::warn!("reflections: {error}"),
    }
}

/// Draws a particle layer: each of its systems with its own material, straight into the scene.
fn draw_particles(
    engine: &mut Engine,
    frame: &Frame<'_>,
    layer: &Layer,
    particles: &ParticleLayer,
    scene: Canvas,
    pictures: &Pictures,
) {
    let world = placed(frame, layer);
    scene.bind(&engine.gl);
    for part in &particles.parts {
        let Some(mesh) = part.mesh.as_ref().filter(|_| part.vertices > 0) else { continue };
        let state = LayerState {
            mvp: frame.view_projection.mul(&world),
            model: world,
            color: [1.0; 3],
            alpha: 1.0,
            brightness: 1.0,
            authored: frame.screen,
            vars: [part.trail, part.sheet],
        };
        engine.gl.set_blend(part.blend);
        apply(&engine.gl, &part.shade, frame, &state, None, &[], None, pictures, engine.white());
        mesh.draw();
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
    let world = placed(frame, layer);
    let state = LayerState {
        mvp: frame.view_projection.mul(&world),
        model: world,
        color: image.color,
        alpha: image.alpha,
        brightness: image.brightness,
        authored: image.size,
        vars: [[0.0; 4]; 2],
    };
    let steps: Vec<&Step> = image.effects.iter().filter(|e| e.visible).flat_map(|e| &e.steps).collect();
    if steps.is_empty() && image.mode != ImageMode::Normal {
        return; // A compose layer without effects changes nothing.
    }

    let needs_background = image.mode != ImageMode::Normal
        || image.blend_pass.is_some()
        || image.base.wants_background
        || steps.iter().any(|s| matches!(s, Step::Draw { shade, .. } if shade.wants_background));
    if needs_background {
        let copy = match background.take() {
            Some(copy) => copy,
            None => match engine.pool.take(scene.size.0, scene.size.1, frame.format) {
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

    // A layer with a blend mode, or one others read, always ends in a buffer; and so
    // does a puppet with effects: they work on the flat picture, the mesh comes after.
    let keep = keep || image.blend_pass.is_some() || (image.puppet.is_some() && !steps.is_empty());
    if steps.is_empty() && !keep {
        scene.bind(&engine.gl);
        engine.gl.set_blend(image.blend);
        // A puppet's mesh stands in for the quad, placed from the picture's centre.
        let (shade, direct) = match &image.puppet {
            Some(puppet) => {
                (&puppet.direct, LayerState { mvp: state.mvp.mul(&puppet.placement()), ..copy_state(&state) })
            }
            None => (&image.base, copy_state(&state)),
        };
        apply(&engine.gl, shade, frame, &direct, None, &[], background_texture, pictures, engine.white());
        match &image.puppet {
            Some(puppet) => puppet.shared.texture_mesh().draw(),
            None => image.quad.draw(),
        }
        return;
    }

    // The layer's buffers: as large as the layer reaches the output, never larger than authored.
    let reach = density * world.0[0].abs().max(world.0[5].abs()).max(1e-6);
    let scale = reach.min(1.0);
    let size = match image.mode {
        ImageMode::Fullscreen => scene.size,
        _ => {
            // Both sides shrink together when one is over the limit: a layer keeps its shape.
            let longest = image.size[0].max(image.size[1]) * scale;
            let scale = if longest > MAX_BUFFER_SIDE { scale * MAX_BUFFER_SIDE / longest } else { scale };
            let side = |units: f32| ((units * scale).ceil() as u32).clamp(1, MAX_BUFFER_SIDE as u32);
            (side(image.size[0]), side(image.size[1]))
        }
    };
    let mut chain =
        Chain { ping: [None, None], current: None, named: image.buffers.iter().map(|_| None).collect() };
    let fullscreen = image.mode == ImageMode::Fullscreen;

    // The base pass fills the first buffer.
    let ok = (|| -> Result<(), crate::gpu::GpuError> {
        let first = engine.pool.take(size.0, size.1, frame.format)?;
        first.bind();
        engine.gl.set_blend(Blend::None);
        match image.mode {
            // The compose shader places itself by texture coordinate and reads the scene by position.
            ImageMode::Compose => {
                apply(
                    &engine.gl,
                    &image.base,
                    frame,
                    &state,
                    None,
                    &[],
                    background_texture,
                    pictures,
                    engine.white(),
                );
                image.quad_unit.draw();
            }
            ImageMode::Fullscreen => {
                let flat = LayerState { mvp: Mat4::IDENTITY, ..copy_state(&state) };
                apply(
                    &engine.gl,
                    &image.base,
                    frame,
                    &flat,
                    None,
                    &[],
                    background_texture,
                    pictures,
                    engine.white(),
                );
                engine.fullscreen.draw();
            }
            ImageMode::Normal => {
                let flat = LayerState { mvp: Mat4::IDENTITY, ..copy_state(&state) };
                apply(
                    &engine.gl,
                    &image.base,
                    frame,
                    &flat,
                    None,
                    &[],
                    background_texture,
                    pictures,
                    engine.white(),
                );
                image.base_quad.draw();
            }
        }
        chain.ping[0] = Some(first);
        chain.current = Some(0);

        // The step after which each named buffer is not looked at again: it goes back
        // to the pool there, for the next effect to borrow, not at the end of the layer.
        let mut last_use = vec![usize::MAX; chain.named.len()];
        for (index, step) in steps.iter().enumerate() {
            let mut used = |slot: usize| {
                if let Some(last) = last_use.get_mut(slot) {
                    *last = index;
                }
            };
            match step {
                Step::Copy { source, target } => {
                    used(*source);
                    used(*target);
                }
                Step::Draw { shade, target } => {
                    target.iter().copied().for_each(&mut used);
                    for (_, _, source) in &shade.samplers {
                        if let TextureRef::Buffer(slot) = source {
                            used(*slot);
                        }
                    }
                }
            }
        }

        let mut landed = false;
        for (index, step) in steps.iter().enumerate() {
            let last = index + 1 == steps.len();
            match step {
                Step::Copy { source, target } => {
                    ensure_named(engine, &mut chain, image, *target, size, frame.format)?;
                    if let (Some(from), Some(to)) =
                        (chain.named[*source].as_ref(), chain.named[*target].as_ref())
                    {
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
                        ensure_named(engine, &mut chain, image, *slot, size, frame.format)?;
                        chain.named[*slot].as_ref().expect("just ensured").bind();
                        engine.gl.set_blend(Blend::None);
                    } else {
                        if chain.ping[next].is_none() {
                            chain.ping[next] = Some(engine.pool.take(size.0, size.1, frame.format)?);
                        }
                        chain.ping[next].as_ref().expect("just ensured").bind();
                        engine.gl.set_blend(Blend::None);
                    }
                    let previous = chain.current.and_then(|c| chain.ping[c].as_ref()).map(|t| &t.texture);
                    // A named target may be one this very pass reads: GL forbids that, so it reads white instead.
                    let reading: Vec<Option<&Texture>> = chain
                        .named
                        .iter()
                        .enumerate()
                        .map(|(i, t)| if Some(i) == *target { None } else { t.as_ref().map(|t| &t.texture) })
                        .collect();
                    if to_scene && !fullscreen {
                        apply(
                            &engine.gl,
                            shade,
                            frame,
                            &state,
                            previous,
                            &reading,
                            background_texture,
                            pictures,
                            engine.white(),
                        );
                        image.quad_unit.draw();
                    } else {
                        let flat = LayerState { mvp: Mat4::IDENTITY, ..copy_state(&state) };
                        apply(
                            &engine.gl,
                            shade,
                            frame,
                            &flat,
                            previous,
                            &reading,
                            background_texture,
                            pictures,
                            engine.white(),
                        );
                        engine.fullscreen.draw();
                    }
                    if to_scene {
                        landed = true;
                    } else if target.is_none() {
                        chain.current = Some(next);
                    }
                }
            }
            for (slot, last) in last_use.iter().enumerate() {
                if *last == index
                    && let Some(done) = chain.named[slot].take()
                {
                    engine.pool.give(done);
                }
            }
        }
        // The chain ended in a buffer: put its picture into the scene, unless the layer
        // is hidden and drawn for its picture alone.
        if !landed
            && layer.shown
            && let Some(result) = chain.current.and_then(|c| chain.ping[c].as_ref())
        {
            scene.bind(&engine.gl);
            match (&image.blend_pass, &image.puppet) {
                // The mesh bends the finished picture on its way into the scene. The
                // layer's tint and opacity are in the picture already.
                (_, Some(puppet)) if !fullscreen => {
                    engine.gl.set_blend(image.blend);
                    let warped = LayerState {
                        mvp: state.mvp.mul(&puppet.placement()),
                        color: [1.0; 3],
                        alpha: 1.0,
                        brightness: 1.0,
                        ..copy_state(&state)
                    };
                    apply(
                        &engine.gl,
                        &puppet.warp,
                        frame,
                        &warped,
                        Some(&result.texture),
                        &[],
                        background_texture,
                        pictures,
                        engine.white(),
                    );
                    puppet.shared.mesh.draw();
                }
                // The blend shader reads the frame behind the layer and writes the mix.
                (Some(blend), _) if !fullscreen => {
                    engine.gl.set_blend(Blend::None);
                    apply(
                        &engine.gl,
                        blend,
                        frame,
                        &state,
                        Some(&result.texture),
                        &[],
                        background_texture,
                        pictures,
                        engine.white(),
                    );
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
    LayerState {
        mvp: state.mvp,
        model: state.model,
        color: state.color,
        alpha: state.alpha,
        brightness: state.brightness,
        authored: state.authored,
        vars: state.vars,
    }
}

/// Makes sure the layer's named buffer `slot` exists.
fn ensure_named(
    engine: &mut Engine,
    chain: &mut Chain,
    image: &ImageLayer,
    slot: usize,
    size: (u32, u32),
    format: TextureFormat,
) -> Result<(), crate::gpu::GpuError> {
    if chain.named[slot].is_none() {
        let scale = image.buffers[slot].scale.max(1.0);
        let (w, h) =
            (((size.0 as f32 / scale).ceil() as u32).max(1), ((size.1 as f32 / scale).ceil() as u32).max(1));
        chain.named[slot] = Some(engine.pool.take(w, h, format)?);
    }
    Ok(())
}

/// Binds a pass's program and textures and sets every uniform it has.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply(
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
    let mut levels = [0.0f32; 16];
    let reflection = frame.reflection.borrow();
    let mut sprites: [Option<([f32; 4], [f32; 2])>; 16] = [None; 16];
    for (uniform, unit, source) in &shade.samplers {
        let authored = [state.authored[0], state.authored[1], state.authored[0], state.authored[1]];
        let (texture, resolution) = match source {
            TextureRef::Shared(shared) => {
                if let Some(sheet) = &shared.sprites
                    && let Some(sprite) = sheet.frames.get(sheet.frame_at(frame.time))
                {
                    let (w, h) = (shared.resolution[0], shared.resolution[1]);
                    sprites[*unit as usize & 15] = Some((
                        [sprite.width / w, sprite.skew.0 / h, sprite.skew.1 / w, sprite.height / h],
                        [sprite.x / w, sprite.y / h],
                    ));
                }
                (shared.texture.raw(), shared.resolution)
            }
            TextureRef::Previous => (previous.unwrap_or(white).raw(), authored),
            TextureRef::Buffer(slot) => {
                (named.get(*slot).copied().flatten().unwrap_or(white).raw(), authored)
            }
            TextureRef::Background => (
                background.unwrap_or(white).raw(),
                [frame.screen[0], frame.screen[1], frame.screen[0], frame.screen[1]],
            ),
            TextureRef::Layer(id) => match pictures.iter().find(|(layer, ..)| layer == id) {
                Some((_, picture, size)) => (picture.texture.raw(), [size[0], size[1], size[0], size[1]]),
                // A layer drawn later, or hidden, has no picture yet.
                None => (white.raw(), authored),
            },
            TextureRef::Text(surface) => match surface.texture.borrow().as_ref() {
                Some(texture) => (
                    texture.raw(),
                    [
                        texture.width as f32,
                        texture.height as f32,
                        texture.width as f32,
                        texture.height as f32,
                    ],
                ),
                None => (white.raw(), authored),
            },
            TextureRef::Video(video) => {
                let video = video.borrow();
                match (video.texture(), video.size()) {
                    (Some(texture), Some((w, h))) => {
                        (texture.raw(), [w as f32, h as f32, w as f32, h as f32])
                    }
                    // Until its first frame a video shows nothing.
                    _ => (white.raw(), authored),
                }
            }
            TextureRef::Reflection => match reflection.as_ref() {
                Some(copy) => {
                    levels[*unit as usize & 15] = copy.levels() as f32;
                    let (w, h) = (copy.width() as f32, copy.height() as f32);
                    (copy.texture.raw(), [w, h, w, h])
                }
                None => (white.raw(), authored),
            },
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
            Feed::ModelInverse => set(&state.model.inverse().0),
            Feed::Eye => set(&frame.eye),
            Feed::Axis(axis) => set(&frame.axes[*axis % 3]),
            Feed::Ambient => set(&frame.ambient),
            Feed::Skylight => set(&frame.skylight),
            Feed::NormalModel => set(&state.model.normal_matrix()),
            Feed::LightColors => set(frame.lights.color_radius.as_flattened()),
            Feed::LightPlaces => set(frame.lights.position_exponent.as_flattened()),
            Feed::MipLevels(unit) => set(&[levels[*unit as usize & 15]]),
            Feed::RenderVar(index) => set(&state.vars[*index & 1]),
            Feed::ViewProjection => set(&frame.view_projection.0),
            Feed::Identity => {
                if uniform.kind == glow::FLOAT_MAT3 {
                    set(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
                } else {
                    set(&Mat4::IDENTITY.0);
                }
            }
            Feed::Resolution(unit) => set(&resolutions[*unit as usize & 15]),
            Feed::SpriteRotation(unit) => {
                set(&sprites[*unit as usize & 15].map_or([1.0, 0.0, 0.0, 1.0], |s| s.0))
            }
            Feed::SpriteTranslation(unit) => set(&sprites[*unit as usize & 15].map_or([0.0, 0.0], |s| s.1)),
            Feed::Color4 => set(&[
                state.color[0] * state.brightness,
                state.color[1] * state.brightness,
                state.color[2] * state.brightness,
                state.alpha,
            ]),
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
