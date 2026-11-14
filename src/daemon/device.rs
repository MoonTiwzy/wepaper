//! Devices: outputs that are not screens.
//!
//! A device is a size, a rate and a program. The wallpaper it shows is drawn at that
//! size into a buffer of its own, with the output's fill and look like on any monitor,
//! and each finished frame goes to the program's standard input as raw RGB: three
//! bytes a pixel, rows from the top, nothing between frames. What the program does
//! with them is its business: a panel on a cooler, a keyboard's screen, a stream.
//!
//! The daemon's loop never waits for the program. Frames go through a thread that
//! holds the one frame not yet written; a program slower than the device's rate
//! simply gets fewer frames, always the newest.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use wepaper_engine::gpu;
use wepaper_json::{Map, Value, obj};

use super::settings::{typed_flag, typed_number};

/// Devices are numbered from here: far from the numbers a compositor gives its outputs.
pub const FIRST_ID: u32 = 0x4000_0000;

const DEFAULT_FPS: u32 = 30;
const MAX_SIDE: f64 = 8192.0;

/// What a device is, as the config file and `devices/<name>` have it.
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    /// Its picture's size in pixels.
    pub size: (u32, u32),
    /// Frames a second it is sent at most.
    pub fps: u32,
    /// The program that takes the frames, run through `sh -c`.
    pub exec: String,
    /// Off, the device is kept but is no output.
    pub enabled: bool,
}

/// The fields of a device, in the order they are listed.
pub const FIELDS: [&str; 4] = ["size", "fps", "exec", "enabled"];

fn size_of(value: &Value) -> Result<(u32, u32), String> {
    let wrong = || format!("size takes [width, height] or \"WIDTHxHEIGHT\", each from 1 to {MAX_SIDE}");
    let sides: Vec<f64> = match value {
        Value::String(text) => text
            .split(['x', 'X', ' ', ','])
            .filter(|part| !part.is_empty())
            .map(|part| part.parse().unwrap_or(0.0))
            .collect(),
        Value::Array(items) => items.iter().map(|item| typed_number(item).unwrap_or(0.0)).collect(),
        _ => return Err(wrong()),
    };
    match sides.as_slice() {
        [width, height] if (1.0..=MAX_SIDE).contains(width) && (1.0..=MAX_SIDE).contains(height) => {
            Ok((*width as u32, *height as u32))
        }
        _ => Err(wrong()),
    }
}

impl Spec {
    /// A device from its description: `size` and `exec` must be there.
    pub fn from_value(value: &Value) -> Result<Self, String> {
        if value.as_object().is_none() {
            return Err("a device is an object: {\"size\": [480, 480], \"exec\": \"program\"}".to_owned());
        }
        let mut spec = Self { size: (0, 0), fps: DEFAULT_FPS, exec: String::new(), enabled: true };
        for (key, field) in value.members() {
            spec.set(key, field)?;
        }
        if spec.size == (0, 0) {
            return Err("a device needs a size".to_owned());
        }
        if spec.exec.trim().is_empty() {
            return Err("a device needs an exec: the program its frames go to".to_owned());
        }
        Ok(spec)
    }

    pub fn to_value(&self) -> Value {
        obj! {
            "size" => vec![Value::from(self.size.0), Value::from(self.size.1)],
            "fps" => self.fps,
            "exec" => &self.exec,
            "enabled" => self.enabled,
        }
    }

    /// Changes one field. The error says what the field takes.
    pub fn set(&mut self, key: &str, value: &Value) -> Result<(), String> {
        match key {
            "size" => self.size = size_of(value)?,
            "fps" => {
                self.fps = typed_number(value)
                    .filter(|n| (1.0..=240.0).contains(n))
                    .ok_or("fps takes a number from 1 to 240")? as u32
            }
            "exec" => {
                self.exec = value
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                    .ok_or("exec takes a command line")?
                    .to_owned()
            }
            "enabled" => self.enabled = typed_flag(value).ok_or("enabled takes true or false")?,
            _ => return Err(format!("a device has no \"{key}\" (it has: {})", FIELDS.join(", "))),
        }
        Ok(())
    }
}

/// Whether `name` can be a device's: it is an output's name, a part of paths and of
/// a state file.
pub fn name_ok(name: &str) -> Result<(), String> {
    let fine = !name.is_empty()
        && name.len() <= 64
        && name != "*"
        && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '));
    if fine {
        Ok(())
    } else {
        Err(format!("\"{name}\" cannot be a device's name: letters, digits, - _ . and spaces, up to 64"))
    }
}

/// The devices there are: those of the config file, changed and added to by what
/// was set over the socket (`null` there takes a config file's device away).
pub fn specs(config: &Value, overrides: &Map) -> BTreeMap<String, Spec> {
    let mut found = BTreeMap::new();
    let mut read = |name: &str, value: &Value, from: &str| match value {
        Value::Null => {
            found.remove(name);
        }
        _ => match name_ok(name).and_then(|()| Spec::from_value(value)) {
            Ok(spec) => {
                found.insert(name.to_owned(), spec);
            }
            Err(why) => log::warn!("{from}: device {name}: {why}"),
        },
    };
    for (name, value) in config.members() {
        read(name, value, "the config file");
    }
    for (name, value) in overrides.iter() {
        read(name, value, "saved devices");
    }
    found
}

/// RGB from RGBA: what the program gets.
pub fn rgb_of(rgba: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
    for pixel in rgba.as_chunks::<4>().0 {
        rgb.extend_from_slice(&pixel[..3]);
    }
    rgb
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

/// The running program and the thread that feeds it.
struct Sink {
    child: Option<Child>,
    mailbox: Arc<Mailbox>,
    /// The program stopped taking frames.
    broken: Arc<AtomicBool>,
    started: Instant,
}

/// Starts a device's program, its input a pipe.
fn spawn(name: &str, spec: &Spec, socket: &Path) -> Result<Child, String> {
    Command::new("sh")
        .arg("-c")
        .arg(&spec.exec)
        .env("WEPAPER_DEVICE", name)
        .env("WEPAPER_WIDTH", spec.size.0.to_string())
        .env("WEPAPER_HEIGHT", spec.size.1.to_string())
        .env("WEPAPER_FPS", spec.fps.to_string())
        .env("WEPAPER_SOCKET", socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", spec.exec))
}

/// Sees a program off whose input was closed: it has three seconds to end, on a
/// thread of its own so that nothing here waits for it.
fn reap(mut child: Child) {
    let _ = std::thread::Builder::new().name("device reaper".to_owned()).spawn(move || {
        let limit = Instant::now() + Duration::from_secs(3);
        while Instant::now() < limit {
            if !matches!(child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
    });
}

impl Sink {
    fn start(name: &str, spec: &Spec, socket: &Path) -> Result<Self, String> {
        let mut child = spawn(name, spec, socket)?;
        let mut input = child.stdin.take().ok_or("the program has no standard input")?;
        let mailbox = Arc::new(Mailbox::default());
        let broken = Arc::new(AtomicBool::new(false));
        let (theirs, flag) = (mailbox.clone(), broken.clone());
        std::thread::Builder::new()
            .name(format!("device {name}"))
            .spawn(move || {
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
                    if input.write_all(&frame).and_then(|()| input.flush()).is_err() {
                        flag.store(true, Ordering::Relaxed);
                        return;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { child: Some(child), mailbox, broken, started: Instant::now() })
    }

    /// Hands over the newest frame; one that was still waiting is dropped for it.
    fn send(&self, frame: Vec<u8>) {
        self.mailbox.parcel.lock().unwrap_or_else(|e| e.into_inner()).frame = Some(frame);
        self.mailbox.wake.notify_one();
    }

    fn alive(&mut self) -> bool {
        !self.broken.load(Ordering::Relaxed)
            && self.child.as_mut().is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }
}

impl Sink {
    /// Closes the program's input, so that one that reads to the end ends, and
    /// hands the program over to be seen off.
    fn close(&mut self) -> Option<Child> {
        self.mailbox.parcel.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        self.mailbox.wake.notify_one();
        self.child.take()
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        if let Some(child) = self.close() {
            reap(child);
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // The device is gone: its programs are seen off without anybody waiting.
        self.stop();
        for (child, _) in self.leaving.drain(..) {
            reap(child);
        }
    }
}

/// A device that is an output right now.
pub struct Device {
    pub id: u32,
    pub name: String,
    pub spec: Spec,
    /// What its frames are read back from.
    pub frame: Option<gpu::Target>,
    sink: Option<Sink>,
    /// Its program when a renderer plugin feeds it: the plugin writes the frames
    /// straight to the program, and the daemon only keeps the program.
    piped: Option<Child>,
    /// Programs that were told to end (their input closed) and may not have yet,
    /// each with when it is ended by force. The next one waits for them: two at
    /// once would both talk to the device.
    leaving: Vec<(Child, Instant)>,
    /// Why its program does not run, when it should.
    pub error: Option<String>,
    retry_at: Instant,
    backoff: Duration,
    /// Frames handed to the program since it was started.
    pub sent: u64,
}

impl Device {
    pub fn new(id: u32, name: String, spec: Spec) -> Self {
        Self {
            id,
            name,
            spec,
            frame: None,
            sink: None,
            piped: None,
            leaving: Vec::new(),
            error: None,
            retry_at: Instant::now(),
            backoff: Duration::from_secs(1),
            sent: 0,
        }
    }

    /// The buffer its frames are drawn into, at its size.
    pub fn target(&mut self, gl: &gpu::Gl) -> Option<&gpu::Target> {
        let size = self.spec.size;
        if self.frame.as_ref().is_none_or(|t| (t.width(), t.height()) != size) {
            self.frame = gpu::Target::new(gl, size.0, size.1, gpu::TextureFormat::Rgba8)
                .inspect_err(|e| log::warn!("{}: {e}", self.name))
                .ok();
        }
        self.frame.as_ref()
    }

    /// Gives the program a frame, starting it when it does not run (and is not
    /// being given a rest after dying).
    pub fn send(&mut self, rgb: Vec<u8>, socket: &Path) {
        if self.sink.is_none() {
            // Not while the program before it is still going, or resting after dying.
            if !self.left() || Instant::now() < self.retry_at {
                return;
            }
            match Sink::start(&self.name, &self.spec, socket) {
                Ok(sink) => {
                    log::info!("{}: frames go to `{}`", self.name, self.spec.exec);
                    self.sink = Some(sink);
                    self.sent = 0;
                    self.error = None;
                }
                Err(why) => {
                    self.failed(why);
                    return;
                }
            }
        }
        if let Some(sink) = &self.sink {
            sink.send(rgb);
            self.sent += 1;
        }
    }

    fn failed(&mut self, why: String) {
        log::error!("{}: {why}; trying again in {} s", self.name, self.backoff.as_secs());
        self.error = Some(why);
        self.retry_at = Instant::now() + self.backoff;
        self.backoff = (self.backoff * 2).min(Duration::from_secs(60));
    }

    /// Starts the program for a renderer plugin to feed, and returns the program's
    /// input: the plugin gets it, writes the frames there itself, and when the plugin
    /// goes the program sees its input end.
    pub fn pipe(&mut self, socket: &Path) -> Result<std::os::fd::OwnedFd, String> {
        self.stop();
        // The one program this waits for: the plugin needs its pipe now. A program
        // ends at once when its input does; one that does not is ended in three seconds.
        while !self.left() {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut child = spawn(&self.name, &self.spec, socket)?;
        let input = child.stdin.take().ok_or("the program has no standard input")?;
        log::info!("{}: a plugin's frames go to `{}`", self.name, self.spec.exec);
        self.piped = Some(child);
        self.sent = 0;
        Ok(input.into())
    }

    /// Lets go of a program a plugin fed: the wallpaper is the engine's own again.
    /// (The plugin is gone by now, and the program with its input at an end.)
    pub fn unpipe(&mut self) {
        if let Some(child) = self.piped.take() {
            self.retire(child);
        }
    }

    /// A program whose input was closed: it has three seconds to end.
    fn retire(&mut self, child: Child) {
        self.leaving.push((child, Instant::now() + Duration::from_secs(3)));
    }

    /// Whether every program told to end has ended. One past its time is ended here.
    fn left(&mut self) -> bool {
        self.leaving.retain_mut(|(child, limit)| match child.try_wait() {
            Ok(None) if Instant::now() < *limit => true,
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                false
            }
            _ => false,
        });
        self.leaving.is_empty()
    }

    /// Stops the program: nothing is shown on the device, or it changed.
    pub fn stop(&mut self) {
        self.unpipe();
        if let Some(mut sink) = self.sink.take() {
            log::info!("{}: its program is stopped", self.name);
            if let Some(child) = sink.close() {
                self.retire(child);
            }
        }
        self.error = None;
        self.backoff = Duration::from_secs(1);
        self.retry_at = Instant::now();
    }

    pub fn running(&self) -> bool {
        self.sink.is_some() || self.piped.is_some()
    }

    /// Notices a program that went on its own: it is started again with the next
    /// frame, at once after a long run and after a growing pause when it keeps dying.
    pub fn upkeep(&mut self) {
        self.left();
        // A program a plugin feeds that went: the plugin finds its frames unread
        // and goes too, which the daemon says for the output.
        if self.piped.as_mut().is_some_and(|child| !matches!(child.try_wait(), Ok(None))) {
            self.piped = None;
            self.error = Some(format!("`{}` stopped taking frames", self.spec.exec));
        }
        let Some(sink) = &mut self.sink else { return };
        if sink.alive() {
            return;
        }
        let lived = sink.started.elapsed();
        self.sink = None;
        if lived > Duration::from_secs(60) {
            self.backoff = Duration::from_secs(1);
        }
        self.failed(format!("`{}` stopped taking frames after {} s", self.spec.exec, lived.as_secs()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Value {
        wepaper_json::parse(text).unwrap()
    }

    #[test]
    fn a_device_is_a_size_and_a_program() {
        let spec = Spec::from_value(&parse(r#"{"size": [480, 480], "exec": "send-frames"}"#)).unwrap();
        assert_eq!(spec, Spec { size: (480, 480), fps: 30, exec: "send-frames".into(), enabled: true });
        assert_eq!(
            Spec::from_value(&spec.to_value()).unwrap(),
            spec,
            "what it says of itself makes it again"
        );
        let spec =
            Spec::from_value(&parse(r#"{"size": "320x240", "fps": "24", "exec": "cat", "enabled": false}"#))
                .unwrap();
        assert_eq!((spec.size, spec.fps, spec.enabled), ((320, 240), 24, false));
        for bad in [
            r#"{"exec": "cat"}"#,
            r#"{"size": [480, 480]}"#,
            r#"{"size": [480, 480], "exec": "  "}"#,
            r#"{"size": [0, 480], "exec": "cat"}"#,
            r#"{"size": [480], "exec": "cat"}"#,
            r#"{"size": [480, 99999], "exec": "cat"}"#,
            r#"{"size": [480, 480], "exec": "cat", "fps": 0}"#,
            r#"{"size": [480, 480], "exec": "cat", "colour": 1}"#,
            r#""cat""#,
        ] {
            assert!(Spec::from_value(&parse(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn names_are_output_names() {
        for good in ["panel", "Side Panel", "panel-2", "a.b_c"] {
            assert!(name_ok(good).is_ok(), "{good}");
        }
        for bad in ["", "*", "a/b", "tab\tname", &"x".repeat(65)] {
            assert!(name_ok(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn what_was_set_over_the_socket_wins_over_the_file() {
        let config = parse(
            r#"{"panel": {"size": [480, 480], "exec": "a"}, "gone": {"size": [1, 1], "exec": "b"},
                "broken": {"size": "big", "exec": "c"}}"#,
        );
        let mut overrides = Map::new();
        overrides.insert("panel", parse(r#"{"size": [240, 240], "exec": "a", "fps": 10}"#));
        overrides.insert("gone", Value::Null);
        overrides.insert("new", parse(r#"{"size": [64, 32], "exec": "d", "enabled": false}"#));
        let found = specs(&config, &overrides);
        assert_eq!(found.keys().collect::<Vec<_>>(), ["new", "panel"]);
        assert_eq!((found["panel"].size, found["panel"].fps), ((240, 240), 10));
        assert!(!found["new"].enabled, "a device that is off is still a device");
    }

    #[test]
    fn frames_are_three_bytes_a_pixel() {
        assert_eq!(rgb_of(&[1, 2, 3, 255, 4, 5, 6, 0]), [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn the_program_gets_the_newest_frame_and_is_missed_when_it_goes() {
        let dir = std::env::temp_dir().join(format!("wepaper-test-device-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("frames");
        let spec = Spec {
            size: (2, 1),
            fps: 30,
            exec: format!(
                "printf '%s' \"$WEPAPER_DEVICE $WEPAPER_WIDTH $WEPAPER_HEIGHT \" > '{0}'; cat >> '{0}'",
                out.display()
            ),
            enabled: true,
        };
        let mut device = Device::new(FIRST_ID, "panel".to_owned(), spec);
        device.send(b"abcdef".to_vec(), Path::new("/nowhere"));
        assert!(device.running() && device.error.is_none());
        let wait = |want: &str| {
            let limit = Instant::now() + Duration::from_secs(5);
            while Instant::now() < limit {
                if std::fs::read_to_string(&out).is_ok_and(|text| text == want) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        };
        assert!(wait("panel 2 1 abcdef"), "{:?}", std::fs::read_to_string(&out));
        device.send(b"ghijkl".to_vec(), Path::new("/nowhere"));
        assert!(wait("panel 2 1 abcdefghijkl"));
        device.upkeep();
        assert!(device.running(), "a program that runs is left alone");
        device.stop();
        assert!(!device.running());

        // A program that exits at once is noticed, and not started again straight away.
        let mut device = Device::new(
            FIRST_ID + 1,
            "dead".to_owned(),
            Spec { size: (1, 1), fps: 30, exec: "exit 3".into(), enabled: true },
        );
        device.send(vec![0; 3], Path::new("/nowhere"));
        let limit = Instant::now() + Duration::from_secs(5);
        while device.running() && Instant::now() < limit {
            std::thread::sleep(Duration::from_millis(20));
            device.upkeep();
        }
        assert!(!device.running() && device.error.is_some());
        device.send(vec![0; 3], Path::new("/nowhere"));
        assert!(!device.running(), "it is given a rest before the next try");
        let _ = std::fs::remove_dir_all(dir);
    }
}
