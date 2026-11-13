//! What the daemon is set to, and what it remembers between runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wepaper_engine::Fill;
use wepaper_engine::doc::FromDoc;
use wepaper_json::{Map, Value, obj};

/// Which layer-shell layer the wallpaper sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layer {
    /// Under everything: the wallpaper's usual place.
    #[default]
    Background,
    /// Above the background layer, still under windows: for desktops whose own
    /// wallpaper would cover the background layer.
    Bottom,
}

impl Layer {
    pub fn name(self) -> &'static str {
        match self {
            Layer::Background => "background",
            Layer::Bottom => "bottom",
        }
    }
}

/// The daemon's settings. Their names here are their names in the config file and
/// under `config/` in the control tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Frames a second at most; 0 follows the display.
    pub fps: u32,
    /// Volume, 0 to 100.
    pub volume: f32,
    /// No sound at all: sound tracks are not even decoded.
    pub mute: bool,
    /// How wallpapers fill outputs of another shape.
    pub fill: Fill,
    /// The layer the surfaces sit on.
    pub layer: Layer,
    /// The pointer passes through to what is under the wallpaper.
    pub passthrough: bool,
    /// Wallpapers that react to sound hear what the system plays.
    pub audio_response: bool,
    /// Decode video on the GPU: less CPU, more video memory.
    pub video_hardware: bool,
    /// Video frames are kept no wider than this; 0 means the widest output.
    pub video_max_width: u32,
    /// Textures larger than this on a side load from a smaller level; 0 for no limit.
    pub texture_max_side: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { fps: 0, volume: 100.0, mute: true, fill: Fill::Cover, layer: Layer::Background, passthrough: false, audio_response: true, video_hardware: false, video_max_width: 0, texture_max_side: 0 }
    }
}

/// The names of the settings, in the order they are listed.
pub const KEYS: &[&str] = &["fps", "volume", "mute", "fill", "layer", "passthrough", "audio_response", "video_hardware", "video_max_width", "texture_max_side"];

impl Settings {
    /// The settings as the `config` node of the control tree.
    pub fn to_value(&self) -> Value {
        obj! {
            "fps" => self.fps,
            "volume" => self.volume,
            "mute" => self.mute,
            "fill" => self.fill.name(),
            "layer" => self.layer.name(),
            "passthrough" => self.passthrough,
            "audio_response" => self.audio_response,
            "video_hardware" => self.video_hardware,
            "video_max_width" => self.video_max_width,
            "texture_max_side" => self.texture_max_side,
        }
    }

    /// Changes one setting. The error says what the setting takes.
    pub fn set(&mut self, key: &str, value: &Value) -> Result<(), String> {
        let number = |low: f64, high: f64| -> Result<f64, String> {
            f32::from_doc(value).map(f64::from).filter(|n| (low..=high).contains(n)).ok_or_else(|| format!("{key} takes a number from {low} to {high}"))
        };
        let flag = || bool::from_doc(value).filter(|_| !value.is_null()).ok_or_else(|| format!("{key} takes true or false"));
        match key {
            "fps" => self.fps = number(0.0, 1000.0)? as u32,
            "volume" => self.volume = number(0.0, 100.0)? as f32,
            "mute" => self.mute = flag()?,
            "fill" => self.fill = value.as_str().and_then(Fill::from_name).ok_or("fill takes cover, contain or stretch")?,
            "layer" => {
                self.layer = match value.as_str() {
                    Some("background") => Layer::Background,
                    Some("bottom") => Layer::Bottom,
                    _ => return Err("layer takes background or bottom".to_owned()),
                }
            }
            "passthrough" => self.passthrough = flag()?,
            "audio_response" => self.audio_response = flag()?,
            "video_hardware" => self.video_hardware = flag()?,
            "video_max_width" => self.video_max_width = number(0.0, 16384.0)? as u32,
            "texture_max_side" => self.texture_max_side = number(0.0, 16384.0)? as u32,
            _ => return Err(format!("there is no setting \"{key}\" (settings: {})", KEYS.join(", "))),
        }
        Ok(())
    }

    /// Applies every setting in a JSON object; a bad one is reported and left as it was.
    pub fn apply(&mut self, values: &Value, from: &str) {
        for (key, value) in values.members() {
            if let Err(why) = self.set(key, value) {
                log::warn!("{from}: {why}");
            }
        }
    }
}

/// What one output shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shown {
    /// The wallpaper's folder.
    pub wallpaper: Option<PathBuf>,
    /// The user's property values for it.
    pub properties: BTreeMap<String, Value>,
    /// This output's own fill, when it has one.
    pub fill: Option<Fill>,
}

/// What the daemon remembers between runs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    /// Settings changed over the socket: they win over the config file until `reload`.
    pub overrides: Map,
    /// What each output showed, by output name.
    pub outputs: BTreeMap<String, Shown>,
    /// What was shown last on any output: what an output seen for the first time gets.
    pub last: Shown,
}

fn shown_to_value(shown: &Shown) -> Value {
    obj! {
        "wallpaper" => shown.wallpaper.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "properties" => Value::Object(shown.properties.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        "fill" => shown.fill.map(Fill::name),
    }
}

fn shown_from_value(value: &Value) -> Shown {
    Shown {
        wallpaper: value.get("wallpaper").as_str().map(PathBuf::from),
        properties: value.get("properties").members().map(|(k, v)| (k.to_owned(), v.clone())).collect(),
        fill: value.get("fill").as_str().and_then(Fill::from_name),
    }
}

impl State {
    /// Reads the state file; a missing or broken one is an empty state.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else { return Self::default() };
        let Ok(doc) = wepaper_json::parse(&text) else {
            log::warn!("{} is not readable JSON: starting with nothing shown", path.display());
            return Self::default();
        };
        Self {
            overrides: doc.get("settings").as_object().cloned().unwrap_or_default(),
            outputs: doc.get("outputs").members().map(|(name, shown)| (name.to_owned(), shown_from_value(shown))).collect(),
            last: shown_from_value(doc.get("last")),
        }
    }

    /// Writes the state file, through a temporary file so it is never half-written.
    pub fn save(&self, path: &Path) {
        let doc = obj! {
            "settings" => Value::Object(self.overrides.clone()),
            "outputs" => Value::Object(self.outputs.iter().map(|(name, shown)| (name.clone(), shown_to_value(shown))).collect()),
            "last" => shown_to_value(&self.last),
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let part = path.with_extension("json.part");
        let written = std::fs::write(&part, doc.to_json_pretty()).and_then(|()| std::fs::rename(&part, path));
        if let Err(error) = written {
            log::warn!("{}: {error}", path.display());
        }
    }
}

/// A directory under an XDG base: `$XDG_<KIND>_HOME/wepaper`, else `~/<fallback>/wepaper`.
pub fn xdg_dir(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback))
        .join("wepaper")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let settings = Settings::default();
        assert_eq!((settings.fps, settings.volume, settings.mute, settings.fill, settings.layer), (0, 100.0, true, Fill::Cover, Layer::Background));
        assert!(!settings.passthrough && !settings.video_hardware);
        let keys: Vec<&str> = settings.to_value().members().map(|(k, _)| k).collect();
        assert_eq!(keys, KEYS, "every setting is listed, and the list is every setting");
    }

    #[test]
    fn settings_take_what_they_say_they_take() {
        let mut settings = Settings::default();
        assert!(settings.set("fps", &Value::from(30)).is_ok() && settings.fps == 30);
        assert!(settings.set("fps", &Value::from("60")).is_ok() && settings.fps == 60, "a number typed as text is a number");
        assert_eq!(settings.set("fps", &Value::from(-1)).unwrap_err(), "fps takes a number from 0 to 1000");
        assert!(settings.set("volume", &Value::from(150)).is_err() && settings.volume == 100.0);
        assert!(settings.set("fill", &Value::from("contain")).is_ok() && settings.fill == Fill::Contain);
        assert!(settings.set("fill", &Value::from("sideways")).is_err());
        assert!(settings.set("layer", &Value::from("bottom")).is_ok() && settings.layer == Layer::Bottom);
        assert!(settings.set("mute", &Value::from(false)).is_ok() && !settings.mute);
        assert!(settings.set("mute", &Value::Null).is_err());
        assert!(settings.set("colour", &Value::from(1)).unwrap_err().contains("there is no setting"));
        settings.apply(&wepaper_json::parse(r#"{"fps": 24, "nonsense": 1, "passthrough": true}"#).unwrap(), "test");
        assert!(settings.fps == 24 && settings.passthrough, "good settings apply past a bad one");
    }

    #[test]
    fn state_round_trips() {
        let dir = std::env::temp_dir().join(format!("wepaper-test-state-{}", std::process::id()));
        let path = dir.join("state.json");
        let mut state = State::default();
        state.overrides.insert("fps", Value::from(30));
        let shown = Shown { wallpaper: Some("/walls/a b".into()), properties: [("speed".to_owned(), Value::from(2))].into(), fill: Some(Fill::Contain) };
        state.outputs.insert("DP-1".to_owned(), shown.clone());
        state.last = shown;
        state.save(&path);
        assert_eq!(State::load(&path), state);
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(State::load(&path), State::default(), "a broken file is an empty state");
        assert_eq!(State::load(&dir.join("missing.json")), State::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
