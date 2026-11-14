//! Frames instead of a surface: the page for a device.
//!
//! A device (the daemon's `docs/devices.md`) is not a screen, so there is nothing to
//! put a wallpaper's surface on. The page is still a widget in a window, because that
//! is how the browser engine lays a page out: the window is as large as the device,
//! lies on the background layer and is fully transparent, so nobody sees it. A few
//! times a second the browser engine is asked for a picture of the page, and the
//! picture is written to the descriptor the daemon handed over as raw RGB: three
//! bytes a pixel, rows from the top.
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

/// A window nobody sees, as large as the device, holding `content`.
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

/// The one frame waiting to be written, and whether there will be more.
#[derive(Default)]
struct Parcel {
    frame: Option<Vec<u8>>,
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
        gone: impl Fn() + 'static,
    ) -> Self {
        let mailbox = Arc::new(Mailbox::default());
        let theirs = mailbox.clone();
        let (said, heard) = std::sync::mpsc::channel::<()>();
        // SAFETY: the daemon opened this descriptor for us and nobody else here uses it.
        let mut sink = unsafe { std::fs::File::from_raw_fd(frames.fd) };
        let writer = move || {
            loop {
                let frame = {
                    let mut parcel = theirs.parcel.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if parcel.closed {
                            return;
                        }
                        if let Some(frame) = parcel.frame.take() {
                            break frame;
                        }
                        parcel = theirs.wake.wait(parcel).unwrap_or_else(|e| e.into_inner());
                    }
                };
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
            if paused.get() || asked.replace(true) {
                return glib::ControlFlow::Continue;
            }
            let (asked, ours) = (asked.clone(), ours.clone());
            page.snapshot(
                SnapshotRegion::Visible,
                SnapshotOptions::NONE,
                gio::Cancellable::NONE,
                move |answer| {
                    asked.set(false);
                    if let Some(frame) = answer.ok().and_then(|texture| rgb_of(&texture, frames.size)) {
                        ours.parcel.lock().unwrap_or_else(|e| e.into_inner()).frame = Some(frame);
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

/// A picture of the page as the device's frame: RGB at `size`. On an output with a
/// scale the picture has more pixels than the device: the nearest ones are taken.
fn rgb_of(texture: &gdk::Texture, size: (u32, u32)) -> Option<Vec<u8>> {
    let (have_w, have_h) = (usize::try_from(texture.width()).ok()?, usize::try_from(texture.height()).ok()?);
    if have_w == 0 || have_h == 0 {
        return None;
    }
    // Four bytes a pixel as GDK keeps them: blue, green, red, alpha.
    let mut pixels = vec![0u8; have_w * have_h * 4];
    texture.download(&mut pixels, have_w * 4);
    Some(resample(&pixels, (have_w, have_h), (size.0 as usize, size.1 as usize)))
}

/// BGRA pixels of one size as RGB of another, by the nearest pixel.
fn resample(bgra: &[u8], have: (usize, usize), want: (usize, usize)) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(want.0 * want.1 * 3);
    for y in 0..want.1 {
        let row = (y * have.1 / want.1).min(have.1 - 1) * have.0;
        for x in 0..want.0 {
            let at = (row + (x * have.0 / want.0).min(have.0 - 1)) * 4;
            rgb.extend_from_slice(&[bgra[at + 2], bgra[at + 1], bgra[at]]);
        }
    }
    rgb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_becomes_the_devices_rgb() {
        // Two pixels: blue, then red, as BGRA.
        let bgra = [255, 0, 0, 255, 0, 0, 255, 255];
        assert_eq!(resample(&bgra, (2, 1), (2, 1)), [0, 0, 255, 255, 0, 0]);
        // Twice as large on a scaled output: every other pixel.
        let wide = [1, 2, 3, 255, 9, 9, 9, 255, 4, 5, 6, 255, 9, 9, 9, 255];
        assert_eq!(resample(&wide, (4, 1), (2, 1)), [3, 2, 1, 6, 5, 4]);
        assert_eq!(resample(&bgra, (2, 1), (4, 2)).len(), 4 * 2 * 3);
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
