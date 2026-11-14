//! Log lines on stderr, as `wepaper-web: <level>: <message>`; stdout belongs to the daemon.

use std::io::Write;

/// Something that stops the plugin.
pub fn error(message: &str) {
    write("error", message);
}

/// Something that went wrong and was worked around.
pub fn warning(message: &str) {
    write("warning", message);
}

/// What the plugin is doing, for someone following along.
pub fn info(message: &str) {
    write("info", message);
}

/// Detail that only matters when chasing a problem: shown when `WEPAPER_WEB_DEBUG` is set.
pub fn debug(message: &str) {
    if std::env::var_os("WEPAPER_WEB_DEBUG").is_some() {
        write("debug", message);
    }
}

fn write(level: &str, message: &str) {
    // One line per message, whatever a page or an error text puts in it.
    // A closed stderr is no reason to stop drawing.
    let _ = writeln!(std::io::stderr(), "wepaper-web: {level}: {}", message.replace(['\n', '\r'], " "));
}
