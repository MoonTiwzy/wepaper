//! Log lines on stderr.

use std::io::Write;

struct Logger;

static LOGGER: Logger = Logger;

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let _ = writeln!(std::io::stderr().lock(), "wepaper: {}: {}", record.level().as_str().to_lowercase(), record.args());
        }
    }

    fn flush(&self) {}
}

/// Starts logging at `level`, or at what `WEPAPER_LOG` names (`error`, `warn`, `info`, `debug`, `trace`).
pub fn init(level: log::LevelFilter) {
    let level = std::env::var("WEPAPER_LOG").ok().and_then(|name| name.parse().ok()).unwrap_or(level);
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(level);
}
