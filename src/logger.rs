//! Log lines on stderr.

use std::io::Write;

use std::sync::atomic::{AtomicBool, Ordering};

struct Logger;

static LOGGER: Logger = Logger;
/// Whether lines carry the time: the daemon's do, a log is read hours later.
static TIMED: AtomicBool = AtomicBool::new(false);

/// The time of day as a log line starts with it: `14:03:27.518`.
fn clock() -> String {
    // SAFETY: plain libc calls filling structures of ours.
    unsafe {
        let mut now: libc::timespec = std::mem::zeroed();
        libc::clock_gettime(libc::CLOCK_REALTIME, &mut now);
        let mut local: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now.tv_sec, &mut local);
        format!("{:02}:{:02}:{:02}.{:03}", local.tm_hour, local.tm_min, local.tm_sec, now.tv_nsec / 1_000_000)
    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let time = if TIMED.load(Ordering::Relaxed) { format!("{} ", clock()) } else { String::new() };
            let _ = writeln!(
                std::io::stderr().lock(),
                "{time}wepaper: {}: {}",
                record.level().as_str().to_lowercase(),
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

/// Starts logging at `level`, or at what `WEPAPER_LOG` names (`error`, `warn`, `info`, `debug`, `trace`).
/// `timed` puts the time of day in front of every line.
pub fn init(level: log::LevelFilter, timed: bool) {
    TIMED.store(timed, Ordering::Relaxed);
    let level = std::env::var("WEPAPER_LOG").ok().and_then(|name| name.parse().ok()).unwrap_or(level);
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(level);
}
