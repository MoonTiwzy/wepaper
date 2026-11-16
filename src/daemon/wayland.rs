//! The Wayland side: outputs, and one layer surface on each.
//!
//! Event handlers here only record what the compositor said. The daemon's loop
//! looks at the records and acts, so no handler ever draws or loads anything.

use std::ffi::c_void;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_callback, wl_compositor, wl_output, wl_pointer, wl_region, wl_registry, wl_seat, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_egl::WlEglSurface;
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use wepaper_engine::gpu;

use super::settings::Layer;

/// The namespace of the layer surfaces: what compositor rules match on.
pub const NAMESPACE: &str = "wepaper";

/// The layer surface on an output and what is needed to draw to it.
pub struct Sheet {
    pub surface: wl_surface::WlSurface,
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    viewport: Option<WpViewport>,
    fractional: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    /// The size the compositor gave it, in logical pixels; zero until configured.
    pub logical: (u32, u32),
    /// The scale in 120ths: 120 is 1x, 180 is 1.5x.
    pub scale_120: u32,
    /// The size or scale changed since the buffers were last sized.
    pub resized: bool,
    /// A frame was shown and the compositor has not asked for the next one yet.
    pub frame_pending: bool,
    /// The compositor closed the surface.
    pub closed: bool,
    // The EGL surface goes before the window it draws to.
    pub egl: Option<gpu::Surface>,
    window: Option<WlEglSurface>,
    /// The frames the EGL surface has shown since it was made.
    shown: u32,
}

/// The frames a surface shows before it is made anew, where the driver's library
/// keeps a buffer release for each: two minutes at 60 a second, a megabyte and a half.
const RENEW_AFTER: u32 = 7200;

impl Sheet {
    /// The size of its buffers in device pixels.
    pub fn pixels(&self) -> (u32, u32) {
        let scaled =
            |logical: u32| ((u64::from(logical) * u64::from(self.scale_120) + 60) / 120).max(1) as u32;
        (scaled(self.logical.0), scaled(self.logical.1))
    }

    /// Makes the buffers match the configured size and scale. Returns whether there is something to draw on.
    pub fn fit(&mut self, context: &gpu::Context) -> bool {
        if self.logical.0 == 0 || self.logical.1 == 0 {
            return false;
        }
        let (width, height) = self.pixels();
        match &self.window {
            Some(window) => window.resize(width as i32, height as i32, 0, 0),
            None => {
                let window = match WlEglSurface::new(self.surface.id(), width as i32, height as i32) {
                    Ok(window) => window,
                    Err(error) => {
                        log::error!("no EGL window for a surface: {error}");
                        return false;
                    }
                };
                // SAFETY: the window is alive and is dropped after the EGL surface made from it.
                match unsafe { context.window_surface(window.ptr() as *mut c_void) } {
                    Ok(egl) => {
                        self.egl = Some(egl);
                        self.window = Some(window);
                    }
                    Err(error) => {
                        log::error!("{error}");
                        return false;
                    }
                }
            }
        }
        match &self.viewport {
            // The buffer is in device pixels; the viewport says how large it is in logical ones.
            Some(viewport) => viewport.set_destination(self.logical.0 as i32, self.logical.1 as i32),
            None => self.surface.set_buffer_scale((self.scale_120 / 120).max(1) as i32),
        }
        self.resized = false;
        true
    }
}

impl Sheet {
    /// Counts a frame shown. `true` when the surface has shown so many that it is time
    /// to make it anew.
    pub fn shown_one(&mut self) -> bool {
        self.shown += 1;
        self.shown >= RENEW_AFTER
    }

    /// Drops the EGL surface and its window, and with them what the driver's library
    /// held for them; the next [`fit`](Self::fit) makes new ones. The surface must not
    /// be the one the context draws to. What is on screen stays until the next frame.
    pub fn renew(&mut self) {
        self.egl = None;
        self.window = None;
        self.shown = 0;
        self.resized = true;
    }
}

impl Drop for Sheet {
    fn drop(&mut self) {
        self.egl = None;
        self.window = None;
        if let Some(fractional) = &self.fractional {
            fractional.destroy();
        }
        if let Some(viewport) = &self.viewport {
            viewport.destroy();
        }
        self.layer.destroy();
        self.surface.destroy();
    }
}

/// An output of the compositor.
pub struct Head {
    /// The compositor's number for the output; unique while it is connected.
    pub id: u32,
    output: wl_output::WlOutput,
    /// Its connector name (`DP-1`), once the compositor told it.
    pub name: Option<String>,
    /// Everything about the output has been sent at least once.
    pub announced: bool,
    integer_scale: i32,
    /// The surface on it.
    pub sheet: Option<Sheet>,
    /// The pointer on the surface, 0 to 1 from the top left; the middle when it is elsewhere.
    pub pointer: [f32; 2],
    /// Whether the pointer's left button is held over the surface.
    pub pointer_down: bool,
    /// The output was unplugged.
    pub removed: bool,
}

/// The connection's state: globals and outputs.
pub struct Display {
    compositor: wl_compositor::WlCompositor,
    layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1,
    viewporter: Option<WpViewporter>,
    fractional: Option<WpFractionalScaleManagerV1>,
    pointer: Option<wl_pointer::WlPointer>,
    /// The output the pointer is over.
    pointer_on: Option<u32>,
    /// The outputs, in the order they appeared.
    pub heads: Vec<Head>,
    queue: QueueHandle<Display>,
}

impl Display {
    /// Connects to the compositor the environment names and binds what the daemon needs.
    pub fn connect() -> Result<(Connection, EventQueue<Display>, Display), String> {
        let connection =
            Connection::connect_to_env().map_err(|e| format!("no Wayland compositor to connect to: {e}"))?;
        let (globals, mut queue) = registry_queue_init::<Display>(&connection).map_err(|e| e.to_string())?;
        let handle = queue.handle();
        let compositor =
            globals.bind(&handle, 4..=6, ()).map_err(|_| "the compositor has no wl_compositor version 4")?;
        let layer_shell = globals
            .bind(&handle, 1..=4, ())
            .map_err(|_| "the compositor has no wlr-layer-shell: wepaper needs it to be the desktop background (GNOME does not have it)")?;
        let mut display = Display {
            compositor,
            layer_shell,
            viewporter: globals.bind(&handle, 1..=1, ()).ok(),
            fractional: globals.bind(&handle, 1..=1, ()).ok(),
            pointer: None,
            pointer_on: None,
            heads: Vec::new(),
            queue: handle.clone(),
        };
        // A fractional scale is of no use without the viewport that applies it.
        if display.viewporter.is_none() {
            display.fractional = None;
        }
        let _seat: Option<wl_seat::WlSeat> = globals.bind(&handle, 1..=7, ()).ok();
        for global in globals.contents().clone_list() {
            if global.interface == "wl_output" {
                display.add_output(globals.registry(), global.name, global.version);
            }
        }
        queue.roundtrip(&mut display).map_err(|e| e.to_string())?;
        Ok((connection, queue, display))
    }

    fn add_output(&mut self, registry: &wl_registry::WlRegistry, id: u32, version: u32) {
        if version < 4 {
            log::warn!(
                "an output without a name (wl_output version {version}) is ignored: the compositor is too old"
            );
            return;
        }
        let output = registry.bind(id, 4, &self.queue, id);
        self.heads.push(Head {
            id,
            output,
            name: None,
            announced: false,
            integer_scale: 1,
            sheet: None,
            pointer: [0.5, 0.5],
            pointer_down: false,
            removed: false,
        });
    }

    fn head(&mut self, id: u32) -> Option<&mut Head> {
        self.heads.iter_mut().find(|head| head.id == id)
    }

    /// Puts a layer surface on an output. It can be drawn on once the compositor has configured it.
    pub fn cover(&mut self, id: u32, layer: Layer, passthrough: bool) {
        let Some(index) = self.heads.iter().position(|head| head.id == id) else { return };
        let surface = self.compositor.create_surface(&self.queue, ());
        let on = match layer {
            Layer::Background => zwlr_layer_shell_v1::Layer::Background,
            Layer::Bottom => zwlr_layer_shell_v1::Layer::Bottom,
        };
        let head = &self.heads[index];
        let layer = self.layer_shell.get_layer_surface(
            &surface,
            Some(&head.output),
            on,
            NAMESPACE.to_owned(),
            &self.queue,
            id,
        );
        layer.set_anchor(zwlr_layer_surface_v1::Anchor::all());
        layer.set_exclusive_zone(-1);
        layer.set_size(0, 0);
        layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        if passthrough {
            // An empty input region: the pointer goes to whatever is underneath.
            let region = self.compositor.create_region(&self.queue, ());
            surface.set_input_region(Some(&region));
            region.destroy();
        }
        let viewport =
            self.viewporter.as_ref().map(|viewporter| viewporter.get_viewport(&surface, &self.queue, ()));
        let fractional =
            self.fractional.as_ref().map(|manager| manager.get_fractional_scale(&surface, &self.queue, id));
        surface.commit();
        let scale_120 = (head.integer_scale.max(1) as u32) * 120;
        self.heads[index].sheet = Some(Sheet {
            surface,
            layer,
            viewport,
            fractional,
            logical: (0, 0),
            scale_120,
            resized: false,
            frame_pending: false,
            closed: false,
            egl: None,
            window: None,
            shown: 0,
        });
    }

    /// Asks the compositor to say when the surface on output `id` wants its next frame.
    pub fn request_frame(&mut self, id: u32) {
        let queue = self.queue.clone();
        if let Some(sheet) = self.head(id).and_then(|head| head.sheet.as_mut()) {
            sheet.surface.frame(&queue, id);
            sheet.frame_pending = true;
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Display {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == "wl_output" => {
                state.add_output(registry, name, version)
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(head) = state.head(name) {
                    head.removed = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for Display {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(head) = state.head(*id) else { return };
        match event {
            wl_output::Event::Name { name } => head.name = Some(name),
            wl_output::Event::Scale { factor } => {
                head.integer_scale = factor.max(1);
                // Where the compositor sends a fractional scale, that one is used instead.
                if let Some(sheet) = head.sheet.as_mut().filter(|sheet| sheet.fractional.is_none()) {
                    sheet.scale_120 = factor.max(1) as u32 * 120;
                    sheet.resized = true;
                }
            }
            wl_output::Event::Done => head.announced = true,
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, u32> for Display {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(sheet) = state.head(*id).and_then(|head| head.sheet.as_mut()) else { return };
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                layer.ack_configure(serial);
                if sheet.logical != (width, height) {
                    sheet.logical = (width, height);
                    sheet.resized = true;
                }
            }
            zwlr_layer_surface_v1::Event::Closed => sheet.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, u32> for Display {
    fn event(
        state: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event
            && let Some(sheet) = state.head(*id).and_then(|head| head.sheet.as_mut())
            && sheet.scale_120 != scale
        {
            sheet.scale_120 = scale.max(1);
            sheet.resized = true;
        }
    }
}

impl Dispatch<wl_callback::WlCallback, u32> for Display {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event
            && let Some(sheet) = state.head(*id).and_then(|head| head.sheet.as_mut())
        {
            sheet.frame_pending = false;
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Display {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(capabilities) } = event {
            let has_pointer = capabilities.contains(wl_seat::Capability::Pointer);
            if has_pointer && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(queue, ()));
            } else if !has_pointer && let Some(pointer) = state.pointer.take() {
                pointer.release();
            }
        }
    }
}

/// The left button, as the kernel numbers it (`BTN_LEFT`).
const LEFT_BUTTON: u32 = 0x110;

impl Dispatch<wl_pointer::WlPointer, ()> for Display {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let place = |state: &mut Self, x: f64, y: f64| {
            let Some(id) = state.pointer_on else { return };
            if let Some(head) = state.head(id)
                && let Some(sheet) = &head.sheet
                && sheet.logical.0 > 0
                && sheet.logical.1 > 0
            {
                head.pointer =
                    [(x / f64::from(sheet.logical.0)) as f32, (y / f64::from(sheet.logical.1)) as f32];
            }
        };
        match event {
            wl_pointer::Event::Enter { surface, surface_x, surface_y, .. } => {
                state.pointer_on = state
                    .heads
                    .iter()
                    .find(|head| head.sheet.as_ref().is_some_and(|sheet| sheet.surface == surface))
                    .map(|head| head.id);
                place(state, surface_x, surface_y);
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => place(state, surface_x, surface_y),
            wl_pointer::Event::Leave { .. } => {
                // A button let go of elsewhere is never heard of: it counts as let go.
                if let Some(head) = state.pointer_on.take().and_then(|id| state.head(id)) {
                    head.pointer_down = false;
                }
            }
            wl_pointer::Event::Button { button: LEFT_BUTTON, state: pressed, .. } => {
                let down = matches!(pressed, WEnum::Value(wl_pointer::ButtonState::Pressed));
                if let Some(head) = state.pointer_on.and_then(|id| state.head(id)) {
                    head.pointer_down = down;
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(Display: wl_compositor::WlCompositor);
delegate_noop!(Display: zwlr_layer_shell_v1::ZwlrLayerShellV1);
delegate_noop!(Display: WpViewporter);
delegate_noop!(Display: WpViewport);
delegate_noop!(Display: WpFractionalScaleManagerV1);
delegate_noop!(Display: wl_region::WlRegion);
delegate_noop!(Display: ignore wl_surface::WlSurface);
