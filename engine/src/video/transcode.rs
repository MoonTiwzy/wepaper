//! Videos that cost more to decode than they are worth get a cheaper copy.
//!
//! A 4K video on a 1440p output, or a 120 fps video under a 60 fps cap, makes the
//! decoder work for pixels and frames nobody sees. Such a video is re-encoded once,
//! in the background and at low priority, to the size and rate that are shown; the
//! copy lives in the cache and the original is never touched. Playback starts on
//! the original at once and moves to the copy when it is ready.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::UNIX_EPOCH;

use crate::read::{FNV_SEED, fnv};

/// What a video is cut down to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// The widest a frame may be; 0 for no limit.
    pub max_width: u32,
    /// The most frames a second; 0 for no limit.
    pub max_fps: f32,
}

impl Limits {
    /// Whether a video of this size and rate is over the limits by enough to be
    /// worth re-encoding (a few percent over is not).
    pub fn exceeded_by(&self, width: u32, fps: f64) -> bool {
        (self.max_width > 0 && f64::from(width) > f64::from(self.max_width) * 1.1) || (self.max_fps > 0.0 && fps > f64::from(self.max_fps) * 1.2)
    }
}

/// Where the cheaper copy of `source` for `limits` lives in `dir`. The name changes
/// when the source file or the limits do, so a stale copy is never played.
pub fn cached_path(dir: &Path, source: &Path, limits: Limits) -> PathBuf {
    let meta = std::fs::metadata(source).ok();
    let modified = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let mut key = fnv(FNV_SEED, source.to_string_lossy().as_bytes());
    key = fnv(key, &meta.map_or(0, |m| m.len()).to_le_bytes());
    key = fnv(key, &modified.to_le_bytes());
    dir.join(format!("{key:016x}-w{}-f{:.0}.mp4", limits.max_width, limits.max_fps))
}

/// A re-encode running in the background.
pub(crate) struct Job {
    pub(crate) target: PathBuf,
    done: Receiver<bool>,
}

impl Job {
    /// `Some(succeeded)` once ffmpeg has finished.
    pub(crate) fn finished(&self) -> Option<bool> {
        match self.done.try_recv() {
            Ok(ok) => Some(ok),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(false),
        }
    }
}

/// The ffmpeg arguments that cut `source` down to `limits`.
pub(crate) fn arguments(source: &Path, part: &Path, limits: Limits) -> Vec<String> {
    let mut filters = Vec::new();
    if limits.max_width > 0 {
        // Never upscale; keep the height even, as the encoder requires.
        filters.push(format!("scale='min({},iw)':-2:flags=bicubic", limits.max_width));
    }
    if limits.max_fps > 0.0 {
        filters.push(format!("fps='min({:.3},source_fps)'", limits.max_fps));
    }
    let mut args: Vec<String> = ["-nostdin", "-v", "error", "-y", "-threads", "4", "-i"].map(str::to_owned).into();
    args.push(source.to_string_lossy().into_owned());
    if !filters.is_empty() {
        args.extend(["-vf".to_owned(), filters.join(",")]);
    }
    // Quality close to the source, and a stream that is cheap to decode: that is the point.
    args.extend(["-c:v", "libx264", "-preset", "veryfast", "-crf", "17", "-tune", "fastdecode", "-pix_fmt", "yuv420p", "-c:a", "copy", "-f", "mp4"].map(str::to_owned));
    args.push(part.to_string_lossy().into_owned());
    args
}

/// Starts re-encoding `source` into `target`. `None` when ffmpeg cannot be started.
pub(crate) fn start(source: &Path, target: &Path, limits: Limits) -> Option<Job> {
    std::fs::create_dir_all(target.parent()?).ok()?;
    // Unique to this process: two daemons never write the same partial file.
    let part = target.with_extension(format!("{}.part", std::process::id()));
    let mut command = Command::new("ffmpeg");
    command.args(arguments(source, &part, limits)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: `setpriority` is async-signal-safe and touches only the child.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            // The lowest priority: the desktop, and the wallpaper itself, come first.
            libc::setpriority(libc::PRIO_PROCESS, 0, 19);
            Ok(())
        });
    }
    let mut child = command.spawn().inspect_err(|error| log::info!("videos are not optimised: ffmpeg could not be started ({error})")).ok()?;
    let (sender, done) = channel();
    let (target_path, name) = (target.to_owned(), source.display().to_string());
    std::thread::Builder::new()
        .name("wepaper-transcode".to_owned())
        .spawn(move || {
            let ok = child.wait().is_ok_and(|status| status.success()) && std::fs::rename(&part, &target_path).is_ok();
            if !ok {
                let _ = std::fs::remove_file(&part);
                log::warn!("{name} could not be re-encoded; the original keeps playing");
            }
            let _ = sender.send(ok);
        })
        .ok()?;
    Some(Job { target: target.to_owned(), done })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_name_depends_on_caps() {
        let dir = Path::new("/cache");
        let source = Path::new("/nonexistent/video.mp4");
        let a = cached_path(dir, source, Limits { max_width: 2560, max_fps: 60.0 });
        let b = cached_path(dir, source, Limits { max_width: 1920, max_fps: 60.0 });
        let c = cached_path(dir, source, Limits { max_width: 2560, max_fps: 30.0 });
        let other = cached_path(dir, Path::new("/nonexistent/other.mp4"), Limits { max_width: 2560, max_fps: 60.0 });
        assert!(a != b && a != c && b != c && a != other);
        assert_eq!(a, cached_path(dir, source, Limits { max_width: 2560, max_fps: 60.0 }), "the same video and limits name the same file");
        assert!(a.starts_with(dir) && a.extension().is_some_and(|e| e == "mp4"));
    }

    #[test]
    fn only_videos_well_over_the_limits_are_re_encoded() {
        let limits = Limits { max_width: 2560, max_fps: 60.0 };
        assert!(limits.exceeded_by(3840, 30.0), "4K on a 1440p limit");
        assert!(limits.exceeded_by(1920, 120.0), "120 fps under a 60 fps cap");
        assert!(!limits.exceeded_by(2560, 60.0) && !limits.exceeded_by(2700, 62.0), "a few percent over is left alone");
        assert!(!Limits { max_width: 0, max_fps: 0.0 }.exceeded_by(7680, 240.0), "no limits, nothing exceeds them");
    }

    #[test]
    fn ffmpeg_is_told_to_scale_cap_and_keep_the_sound() {
        let args = arguments(Path::new("/in.mp4"), Path::new("/out.part"), Limits { max_width: 2560, max_fps: 30.0 });
        let line = args.join(" ");
        assert!(line.contains("scale='min(2560,iw)':-2") && line.contains("fps='min(30.000,source_fps)'"));
        assert!(line.contains("-tune fastdecode") && line.contains("-c:a copy") && line.ends_with("/out.part"));
        let plain = arguments(Path::new("/in.mp4"), Path::new("/out.part"), Limits { max_width: 0, max_fps: 0.0 }).join(" ");
        assert!(!plain.contains("-vf"), "no limits, no filters");
    }
}
