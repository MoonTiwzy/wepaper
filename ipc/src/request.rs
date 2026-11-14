//! Requests and responses.

use wepaper_json::{Value, obj};

/// The protocol's version. It only grows: a client of an older version keeps working.
pub const PROTOCOL: u32 = 1;

/// What a client asks of the daemon.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// The daemon's version, protocol and features.
    Version,
    /// The value at a path: a leaf, or a whole subtree.
    Get {
        /// The path, e.g. `config/fps`; empty for the whole tree.
        path: String,
    },
    /// Changes the value at a path.
    Set {
        /// The path of a settable leaf. `outputs/*/...` means every output.
        path: String,
        /// The new value.
        value: Value,
    },
    /// The names under a path.
    List {
        /// The path of a node.
        path: String,
    },
    /// Returns a path to its default: a property to the wallpaper's own value, an
    /// edited layer field to the scene's, a setting to the config file's.
    Reset {
        /// The path; a node resets everything under it.
        path: String,
    },
    /// Shows a wallpaper. Answered when it shows, or with why it could not.
    Show {
        /// A wallpaper folder or a workshop id.
        wallpaper: String,
        /// The output to show it on; every output when `None`.
        output: Option<String>,
        /// The user property values to show it with, over the wallpaper's own.
        /// `None` shows it with the values it had the last time it was shown.
        properties: Option<Vec<(String, Value)>>,
    },
    /// Takes the wallpaper off an output (or all), leaving nothing drawn.
    Hide {
        /// The output; every output when `None`.
        output: Option<String>,
    },
    /// Freezes an output (or all) on its current frame.
    Pause {
        /// The output; every output when `None`.
        output: Option<String>,
    },
    /// Lets a paused output (or all) play again.
    Resume {
        /// The output; every output when `None`.
        output: Option<String>,
    },
    /// Writes what an output shows right now to a PNG.
    Snapshot {
        /// The output; the first one when `None`.
        output: Option<String>,
        /// Where to write the PNG.
        file: String,
    },
    /// Turns this connection into a stream of events.
    Watch,
    /// Reads the config file again.
    Reload,
    /// Stops the daemon.
    Quit,
}

/// The response to a request that worked.
pub fn ok(value: Value) -> Value {
    obj! { "ok" => true, "value" => value }
}

/// The response to a request that did not.
pub fn error(message: impl Into<String>) -> Value {
    obj! { "ok" => false, "error" => message.into() }
}

/// The parts of a path: `outputs/DP-1/paused` is `["outputs", "DP-1", "paused"]`.
/// Leading, trailing and doubled slashes do not count.
pub fn split_path(path: &str) -> Vec<&str> {
    path.split('/').filter(|part| !part.is_empty()).collect()
}

impl Request {
    /// This request as the JSON object that goes on the wire.
    pub fn to_value(&self) -> Value {
        let on = |output: &Option<String>| Value::from(output.clone());
        match self {
            Request::Version => obj! { "cmd" => "version" },
            Request::Get { path } => obj! { "cmd" => "get", "path" => path },
            Request::Set { path, value } => obj! { "cmd" => "set", "path" => path, "value" => value.clone() },
            Request::List { path } => obj! { "cmd" => "list", "path" => path },
            Request::Reset { path } => obj! { "cmd" => "reset", "path" => path },
            Request::Show { wallpaper, output, properties } => {
                let mut message = obj! { "cmd" => "show", "wallpaper" => wallpaper, "output" => on(output) };
                if let (Some(properties), Value::Object(map)) = (properties, &mut message) {
                    map.insert("properties", Value::Object(properties.iter().map(|(name, value)| (name.clone(), value.clone())).collect()));
                }
                message
            }
            Request::Hide { output } => obj! { "cmd" => "hide", "output" => on(output) },
            Request::Pause { output } => obj! { "cmd" => "pause", "output" => on(output) },
            Request::Resume { output } => obj! { "cmd" => "resume", "output" => on(output) },
            Request::Snapshot { output, file } => obj! { "cmd" => "snapshot", "output" => on(output), "file" => file },
            Request::Watch => obj! { "cmd" => "watch" },
            Request::Reload => obj! { "cmd" => "reload" },
            Request::Quit => obj! { "cmd" => "quit" },
        }
    }

    /// Reads a request from the JSON object a client sent. The error says what is
    /// wrong in words a person at a terminal can act on.
    pub fn parse(message: &Value) -> Result<Self, String> {
        let Some(command) = message.get("cmd").as_str() else { return Err("a request is an object with a \"cmd\"".to_owned()) };
        let text = |key: &str| message.get(key).as_str().map(str::to_owned).ok_or_else(|| format!("{command} needs \"{key}\""));
        let path = || text("path");
        let output = || match message.get("output") {
            Value::Null => Ok(None),
            Value::String(name) if name == "*" => Ok(None),
            Value::String(name) => Ok(Some(name.clone())),
            _ => Err(format!("{command}: \"output\" is an output's name")),
        };
        Ok(match command {
            "version" => Request::Version,
            "get" => Request::Get { path: message.get("path").as_str().unwrap_or("").to_owned() },
            "status" => Request::Get { path: String::new() },
            "set" => {
                if message.as_object().is_none_or(|o| o.get("value").is_none()) {
                    return Err("set needs \"value\"".to_owned());
                }
                Request::Set { path: path()?, value: message.get("value").clone() }
            }
            "list" => Request::List { path: message.get("path").as_str().unwrap_or("").to_owned() },
            "reset" => Request::Reset { path: path()? },
            "show" => Request::Show {
                wallpaper: text("wallpaper")?,
                output: output()?,
                properties: message.get("properties").as_object().map(|map| map.iter().map(|(name, value)| (name.to_owned(), value.clone())).collect()),
            },
            "hide" => Request::Hide { output: output()? },
            "pause" => Request::Pause { output: output()? },
            "resume" => Request::Resume { output: output()? },
            "snapshot" => Request::Snapshot { output: output()?, file: text("file")? },
            "watch" => Request::Watch,
            "reload" => Request::Reload,
            "quit" => Request::Quit,
            other => return Err(format!("unknown command \"{other}\"")),
        })
    }
}
