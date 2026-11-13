//! What the system is playing, as samples.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// How many stereo frames are kept: the newest window the spectrum looks at.
pub(super) const WINDOW: usize = 1024;
const RATE: u32 = 44100;

/// A recording of the default output's monitor, kept as the newest [`WINDOW`] frames.
///
/// The recording is done by PulseAudio's `parec` (PipeWire's Pulse server speaks to
/// it as well) in a child process, read by one thread. Linking an audio stack into
/// the engine would cost every user of it, for a feature many wallpapers never use.
pub(super) struct Capture {
    child: Child,
    /// Interleaved left/right frames, oldest first, at most `WINDOW` of them.
    pub(super) samples: Arc<Mutex<Vec<f32>>>,
}

impl Capture {
    /// Starts recording; `None` when `parec` is not there to do it.
    pub(super) fn start() -> Option<Self> {
        let rate = RATE.to_string();
        let mut child = Command::new("parec")
            .args(["--device=@DEFAULT_MONITOR@", "--raw", "--format=s16le", "--channels=2", "--latency-msec=25", "--client-name=wepaper", "--stream-name=spectrum"])
            .arg(format!("--rate={rate}"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .inspect_err(|error| log::warn!("no audio spectrum: parec could not be started ({error})"))
            .ok()?;
        let mut pipe = child.stdout.take()?;
        let samples = Arc::new(Mutex::new(Vec::with_capacity(WINDOW * 2)));
        let shared = samples.clone();
        let spawned = std::thread::Builder::new().name("wepaper-audio".to_owned()).spawn(move || {
            let mut bytes = [0u8; 4096];
            let mut pending = 0;
            // Ends when the child does: the pipe closes and the read returns zero.
            while let Ok(count) = pipe.read(&mut bytes[pending..]) {
                if count == 0 {
                    break;
                }
                let have = pending + count;
                let whole = have / 4 * 4;
                let Ok(mut samples) = shared.lock() else { break };
                for frame in bytes[..whole].chunks_exact(4) {
                    samples.push(f32::from(i16::from_le_bytes([frame[0], frame[1]])) / 32768.0);
                    samples.push(f32::from(i16::from_le_bytes([frame[2], frame[3]])) / 32768.0);
                }
                let excess = samples.len().saturating_sub(WINDOW * 2);
                samples.drain(..excess);
                drop(samples);
                bytes.copy_within(whole..have, 0);
                pending = have - whole;
            }
        });
        if spawned.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        Some(Self { child, samples })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
