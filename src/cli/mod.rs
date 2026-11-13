//! The command line: the daemon's client and the offline tools.

pub mod client;
pub mod render;
pub mod tools;

use std::path::PathBuf;

/// What a failed command says before the process exits with status 1.
pub type Failure = String;

/// The value of the option that was just read, as text.
pub fn value(args: &mut lexopt::Parser) -> Result<String, Failure> {
    args.value().map(|value| value.to_string_lossy().into_owned()).map_err(|e| e.to_string())
}

/// A `WIDTHxHEIGHT` argument.
pub fn parse_size(text: &str) -> Result<(u32, u32), Failure> {
    let size = text.split_once(['x', 'X']).and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
    size.filter(|&(w, h): &(u32, u32)| w > 0 && h > 0 && w <= 16384 && h <= 16384).ok_or_else(|| format!("'{text}' is not a size like 1920x1080"))
}

/// A `name=value` argument; the value is JSON when it parses as JSON, text otherwise.
pub fn parse_property(text: &str) -> Result<(String, wepaper_json::Value), Failure> {
    let (name, value) = text.split_once('=').ok_or_else(|| format!("'{text}' is not name=value"))?;
    Ok((name.to_owned(), wepaper_json::parse(value).unwrap_or_else(|_| wepaper_json::Value::from(value))))
}

/// Where wepaper keeps what it can rebuild: `$XDG_CACHE_HOME/wepaper`.
pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"))
        .join("wepaper")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_properties_parse() {
        assert_eq!(parse_size("1920x1080"), Ok((1920, 1080)));
        assert!(parse_size("1920").is_err() && parse_size("0x5").is_err() && parse_size("axb").is_err());
        assert_eq!(parse_property("speed=2").unwrap(), ("speed".to_owned(), wepaper_json::Value::from(2)));
        assert_eq!(parse_property("on=true").unwrap().1, wepaper_json::Value::from(true));
        assert_eq!(parse_property("name=hello world").unwrap().1, wepaper_json::Value::from("hello world"));
        assert_eq!(parse_property("tint=1 0.5 0").unwrap().1, wepaper_json::Value::from("1 0.5 0"));
        assert!(parse_property("novalue").is_err());
    }
}
