//! Frames instead of a surface: the page for a device.
//!
//! A device (the daemon's `docs/devices.md`) is not a screen, so there is nothing to
//! put a wallpaper's surface on. The page is still a widget in a window, because that
//! is how the browser engine lays a page out: the window lies on the background layer
//! and is fully transparent, so nobody sees it. It is as large as a monitor would be
//! (the stage: the monitor's shape, at most Full HD), so the page lays itself out as
//! it was made to; the device gets the part of it its fill and look frame (the
//! daemon's `frame` command, the engine's placement), like a scene. At the device's
//! rate the browser engine is asked for a picture of the page, and the device's part
//! is written to the descriptor the daemon handed over as raw RGB: three bytes a
//! pixel, rows from the top. While a snapshot of the device is wanted (`stage`), the
//! whole page goes to a file too, smaller, for the daemon to cut it from.
//!
//! The picture is the engine's own (`webkit_web_view_get_snapshot`), not the
//! widget's: a window nobody sees is not drawn by the toolkit, and may never be
//! asked for a frame by the compositor.
//!
//! The main loop never waits for whoever reads the frames. A thread holds the one
//! frame not yet written; a reader slower than the frame rate gets the newest.

use std::io::Write;
use std::os::fd::FromRawFd;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use gtk4 as gtk;

use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, LayerShell};
use webkit6::prelude::*;
use webkit6::{SnapshotOptions, SnapshotRegion, WebView};
use wepaper_ipc::{Cut, Fit, Framing};

/// The largest stage: a page is laid out no larger than Full HD.
const STAGE_MAX: (u32, u32) = (1920, 1080);
/// The widest the stage is written for a snapshot.
pub const STAGE_FILE_WIDTH: u32 = 960;

/// The page's size for a monitor of `monitor` logical pixels: its shape, at most
/// [`STAGE_MAX`].
pub fn stage_of(monitor: (u32, u32)) -> (u32, u32) {
    if monitor.0 == 0 || monitor.1 == 0 {
        return STAGE_MAX;
    }
    let (w, h) = (monitor.0 as f32, monitor.1 as f32);
    let scale = (STAGE_MAX.0 as f32 / w).min(STAGE_MAX.1 as f32 / h).min(1.0);
    ((w * scale).round() as u32, (h * scale).round() as u32)
}

/// How finely the page is drawn for a device: as many of the stage's pixels as the
/// device takes (its fill's scale times the zoom), to the next eighth, at least half
/// (the page for a snapshot is cut from it) and at most all of them.
pub fn render_scale(stage: (u32, u32), device: (u32, u32), fit: Fit, zoom: f32) -> f64 {
    let across = f64::from(device.0) / f64::from(stage.0.max(1));
    let down = f64::from(device.1) / f64::from(stage.1.max(1));
    let fill = if fit == Fit::Contain { across.min(down) } else { across.max(down) };
    let need = fill * f64::from(zoom.max(0.0));
    ((need * 8.0).ceil() / 8.0).clamp(0.5, 1.0)
}

/// The page of a device, laid out at the stage's size and drawn at the scale the
/// device's look needs: fewer pixels to draw and take each frame, the same layout.
pub struct Stage {
    size: (u32, u32),
    device: (u32, u32),
    /// The monitor's scale: the page's pixels are its logical size times this.
    monitor_scale: f64,
    scale: std::cell::Cell<f64>,
    window: gtk::Window,
    page: WebView,
}

impl Stage {
    /// The hidden window for `page` on `monitor`, for a device `device` large.
    pub fn new(monitor: &gdk::Monitor, device: (u32, u32), page: &WebView) -> Self {
        let geometry = monitor.geometry();
        let size = stage_of((geometry.width().max(0) as u32, geometry.height().max(0) as u32));
        let window = hidden_window(monitor, size, page);
        let stage = Self {
            size,
            device,
            monitor_scale: f64::from(monitor.scale_factor().max(1)),
            scale: std::cell::Cell::new(0.0),
            window,
            page: page.clone(),
        };
        stage.frame(Fit::Cover, 1.0);
        stage
    }

    pub fn window(&self) -> &gtk::Window {
        &self.window
    }

    /// The device's fill and zoom changed: the page is drawn as finely as they need.
    pub fn frame(&self, fit: Fit, zoom: f32) {
        let scale = render_scale(self.size, self.device, fit, zoom);
        if self.scale.replace(scale) == scale {
            return;
        }
        // The page's zoom shrinks its pixels by as much as the window: the layout
        // keeps the stage's size in CSS pixels.
        let zoom_level = scale / self.monitor_scale;
        let logical = |side: u32| (f64::from(side) * zoom_level).round().max(1.0) as i32;
        self.page.set_zoom_level(zoom_level);
        self.page.set_size_request(logical(self.size.0), logical(self.size.1));
        self.window.set_default_size(logical(self.size.0), logical(self.size.1));
    }
}

/// Which part of the page the device shows, and where the page goes for a snapshot.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Look {
    pub fit: Fit,
    pub framing: Framing,
    pub stage: Option<String>,
    /// Something changed that a paused page has to show once: a new framing, or a
    /// snapshot asked for.
    pub once: bool,
}

/// Where the frames go and how large they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frames {
    /// The device's size in pixels.
    pub size: (u32, u32),
    /// The descriptor to write them to.
    pub fd: i32,
}

/// `WIDTHxHEIGHT`, each from 1 to 8192.
pub fn parse_size(text: &str) -> Option<(u32, u32)> {
    let (width, height) = text.split_once(['x', 'X'])?;
    let side = |part: &str| part.parse::<u32>().ok().filter(|n| (1..=8192).contains(n));
    Some((side(width)?, side(height)?))
}

/// A window nobody sees, as large as the stage, holding `content`.
pub fn hidden_window(
    monitor: &gdk::Monitor,
    size: (u32, u32),
    content: &impl IsA<gtk::Widget>,
) -> gtk::Window {
    let window = gtk::Window::new();
    window.set_decorated(false);
    content.set_size_request(size.0 as i32, size.1 as i32);
    content.set_can_target(false);
    window.set_child(Some(content));
    window.set_default_size(size.0 as i32, size.1 as i32);
    window.set_opacity(0.0);

    window.init_layer_shell();
    window.set_monitor(Some(monitor));
    window.set_namespace(Some("wepaper-device"));
    window.set_layer(gtk4_layer_shell::Layer::Background);
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    window.set_exclusive_zone(0);
    window.set_keyboard_mode(KeyboardMode::None);
    // No input at all: the pointer goes to whatever is really there.
    window.connect_realize(|window| {
        if let Some(surface) = window.surface() {
            surface.set_input_region(Some(&gtk::cairo::Region::create()));
        }
    });
    window
}

/// A picture of the page as GDK gave it (four bytes a pixel, blue first), with the
/// look it is to be cut by.
struct Shot {
    bgra: Vec<u8>,
    size: (u32, u32),
    fit: Fit,
    framing: Framing,
    stage: Option<String>,
}

/// The device's frame cut from the page: RGB at `device`.
fn device_frame(shot: &Shot, device: (u32, u32)) -> Vec<u8> {
    let cut = Cut::new(shot.size, device, shot.fit, shot.framing).apply(&shot.bgra);
    cut.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0]]).collect()
}

/// The page for a snapshot: its width and height (u32, little endian), then RGBA,
/// no wider than [`STAGE_FILE_WIDTH`].
fn stage_file(shot: &Shot) -> Vec<u8> {
    let width = shot.size.0.clamp(1, STAGE_FILE_WIDTH);
    let height = ((shot.size.1 as u64 * width as u64 + shot.size.0 as u64 / 2) / shot.size.0.max(1) as u64)
        .max(1) as u32;
    let cut = Cut::new(shot.size, (width, height), Fit::Stretch, Framing::default()).apply(&shot.bgra);
    let mut file = Vec::with_capacity(8 + cut.len());
    file.extend_from_slice(&width.to_le_bytes());
    file.extend_from_slice(&height.to_le_bytes());
    file.extend(cut.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]));
    file
}

/// Written beside and moved in place: a reader never sees half a page.
fn write_stage(path: &str, file: &[u8]) {
    let part = format!("{path}.part");
    if std::fs::write(&part, file).and_then(|()| std::fs::rename(&part, path)).is_err() {
        let _ = std::fs::remove_file(&part);
    }
}

/// The one frame waiting to be written, and whether there will be more.
#[derive(Default)]
struct Parcel {
    frame: Option<Shot>,
    closed: bool,
}

#[derive(Default)]
struct Mailbox {
    parcel: Mutex<Parcel>,
    wake: Condvar,
}

/// Takes the page's picture at the device's rate and hands it to the writing thread.
pub struct Capture {
    mailbox: Arc<Mailbox>,
}

impl Capture {
    /// Starts capturing `page`. While `paused` says so no pictures are taken: the
    /// device keeps the frame it has. `gone` is called on the main loop when the
    /// reader of the frames went away.
    pub fn start(
        page: &WebView,
        frames: Frames,
        fps: u32,
        paused: std::rc::Rc<std::cell::Cell<bool>>,
        look: std::rc::Rc<std::cell::RefCell<Look>>,
        gone: impl Fn() + 'static,
    ) -> Self {
        let mailbox = Arc::new(Mailbox::default());
        let theirs = mailbox.clone();
        let (said, heard) = std::sync::mpsc::channel::<()>();
        // SAFETY: the daemon opened this descriptor for us and nobody else here uses it.
        let mut sink = unsafe { std::fs::File::from_raw_fd(frames.fd) };
        let writer = move || {
            loop {
                let shot = {
                    let mut parcel = theirs.parcel.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if parcel.closed {
                            return;
                        }
                        if let Some(shot) = parcel.frame.take() {
                            break shot;
                        }
                        parcel = theirs.wake.wait(parcel).unwrap_or_else(|e| e.into_inner());
                    }
                };
                // The cutting is done here, off the main loop.
                if let Some(path) = &shot.stage {
                    write_stage(path, &stage_file(&shot));
                }
                let frame = device_frame(&shot, frames.size);
                if sink.write_all(&frame).and_then(|()| sink.flush()).is_err() {
                    let _ = said.send(());
                    return;
                }
            }
        };
        if let Err(error) = std::thread::Builder::new().name("frames".to_owned()).spawn(writer) {
            crate::log::warning(&format!("frames are not written: {error}"));
        }
        // The writer's word that the reader is gone, looked for between frames.
        glib::timeout_add_local(Duration::from_millis(250), move || {
            if heard.try_recv().is_ok() {
                gone();
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });

        // One picture at a time: a page slower than the rate is asked again when it
        // has answered, not while it still draws.
        let interval = Duration::from_secs_f64(1.0 / f64::from(fps.clamp(1, 240)));
        let asked = std::rc::Rc::new(std::cell::Cell::new(false));
        let page = page.clone();
        let ours = mailbox.clone();
        glib::timeout_add_local(interval, move || {
            if (paused.get() && !look.borrow().once) || asked.replace(true) {
                return glib::ControlFlow::Continue;
            }
            look.borrow_mut().once = false;
            let (asked, ours, look) = (asked.clone(), ours.clone(), look.clone());
            page.snapshot(
                SnapshotRegion::Visible,
                SnapshotOptions::NONE,
                gio::Cancellable::NONE,
                move |answer| {
                    asked.set(false);
                    if let Some((bgra, size)) = answer.ok().and_then(|texture| bgra_of(&texture)) {
                        let look = look.borrow();
                        let shot = Shot {
                            bgra,
                            size,
                            fit: look.fit,
                            framing: look.framing,
                            stage: look.stage.clone(),
                        };
                        ours.parcel.lock().unwrap_or_else(|e| e.into_inner()).frame = Some(shot);
                        ours.wake.notify_one();
                    }
                },
            );
            glib::ControlFlow::Continue
        });
        Self { mailbox }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.mailbox.parcel.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        self.mailbox.wake.notify_one();
    }
}

/// A picture of the page as GDK keeps it: four bytes a pixel, blue, green, red,
/// alpha. On an output with a scale it has more pixels than the stage.
fn bgra_of(texture: &gdk::Texture) -> Option<(Vec<u8>, (u32, u32))> {
    let (width, height) = (u32::try_from(texture.width()).ok()?, u32::try_from(texture.height()).ok()?);
    if width == 0 || height == 0 {
        return None;
    }
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    texture.download(&mut pixels, width as usize * 4);
    Some((pixels, (width, height)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(bgra: Vec<u8>, size: (u32, u32), fit: Fit, framing: Framing) -> Shot {
        Shot { bgra, size, fit, framing, stage: None }
    }

    fn plain() -> Framing {
        Framing { zoom: 1.0, shift_x: 0.0, shift_y: 0.0 }
    }

    #[test]
    fn a_picture_becomes_the_devices_rgb() {
        // Two pixels: blue, then red, as BGRA.
        let bgra = vec![255, 0, 0, 255, 0, 0, 255, 255];
        assert_eq!(
            device_frame(&shot(bgra.clone(), (2, 1), Fit::Stretch, plain()), (2, 1)),
            [0, 0, 255, 255, 0, 0]
        );
        assert_eq!(device_frame(&shot(bgra, (2, 1), Fit::Stretch, plain()), (4, 2)).len(), 4 * 2 * 3);
    }

    #[test]
    fn a_square_device_shows_the_middle_of_a_wide_page_and_follows_its_look() {
        // Four columns: red, green, blue, white (BGRA); a square device covers the middle two.
        let page: Vec<u8> =
            [[0, 0, 255, 255], [0, 255, 0, 255], [255, 0, 0, 255], [255, 255, 255, 255]].concat();
        let middle = device_frame(&shot(page.clone(), (4, 1), Fit::Cover, plain()), (1, 1));
        assert_eq!(middle.len(), 3);
        // Moved half the device right: what was left of the middle comes into view.
        let moved = device_frame(
            &shot(page, (4, 1), Fit::Cover, Framing { zoom: 4.0, shift_x: 0.0, shift_y: 0.0 }),
            (1, 1),
        );
        assert_eq!(moved.len(), 3);
        assert!(middle != [0, 0, 0] && moved != [0, 0, 0]);
    }

    #[test]
    fn the_stage_file_is_a_header_and_rgba() {
        let page = vec![255, 0, 0, 255, 0, 0, 255, 255];
        let file = stage_file(&shot(page, (2, 1), Fit::Cover, plain()));
        assert_eq!(&file[..8], &[2, 0, 0, 0, 1, 0, 0, 0]);
        // Blue then red, now as RGBA.
        assert_eq!(&file[8..], &[0, 0, 255, 255, 255, 0, 0, 255]);
        // A large page is written smaller: no wider than STAGE_FILE_WIDTH.
        let big = stage_file(&shot(vec![0; 1920 * 1080 * 4], (1920, 1080), Fit::Cover, plain()));
        let width = u32::from_le_bytes(big[..4].try_into().unwrap());
        let height = u32::from_le_bytes(big[4..8].try_into().unwrap());
        assert_eq!((width, height), (STAGE_FILE_WIDTH, STAGE_FILE_WIDTH * 1080 / 1920));
    }

    #[test]
    fn the_page_is_drawn_as_finely_as_the_device_needs() {
        let (stage, device) = ((1920, 1080), (480, 480));
        // Covered at 100 %: the device needs 0.44 of the stage's pixels, never less than half.
        assert_eq!(render_scale(stage, device, Fit::Cover, 1.0), 0.5);
        // Enlarged 1.6 times: 0.71, to the next eighth.
        assert_eq!(render_scale(stage, device, Fit::Cover, 1.6), 0.75);
        // More than the stage's own pixels is not drawn.
        assert_eq!(render_scale(stage, device, Fit::Cover, 2.57), 1.0);
        assert_eq!(render_scale(stage, (1920, 200), Fit::Contain, 1.0), 0.5);
    }

    #[test]
    fn the_stage_has_the_monitors_shape_and_no_more_than_full_hd() {
        assert_eq!(stage_of((2560, 1440)), (1920, 1080));
        assert_eq!(stage_of((1920, 1080)), (1920, 1080));
        assert_eq!(stage_of((1280, 1024)), (1280, 1024));
        assert_eq!(stage_of((3440, 1440)), (1920, 804));
        assert_eq!(stage_of((0, 0)), (1920, 1080));
    }

    #[test]
    fn a_size_is_two_sides() {
        assert_eq!(parse_size("480x480"), Some((480, 480)));
        assert_eq!(parse_size("1920X1080"), Some((1920, 1080)));
        for bad in ["480", "0x480", "480x", "x", "480x9000", "a x b", ""] {
            assert_eq!(parse_size(bad), None, "{bad}");
        }
    }
}
