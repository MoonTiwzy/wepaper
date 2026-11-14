//! The lines the daemon and this plugin exchange: commands on stdin, events on stdout.
//!
//! Both directions are one JSON object per line. A line that does not decode is the
//! daemon's bug, never a reason to stop showing the wallpaper: the caller logs it and
//! reads on.

use wepaper_json::{Map, Value, obj};

/// How many bands of the spectrum each channel carries.
pub const BANDS: usize = 64;

/// The highest frame rate a command may ask for; anything above is a mistake, not a wish.
const FPS_CEILING: f64 = 1000.0;

/// What the daemon asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// User properties changed: the new value of each one that did.
    Properties(Map),
    /// Stop the page's work; the last frame stays up.
    Pause,
    /// Carry on after [`Command::Pause`].
    Resume,
    /// Cap the frame rate; 0 lifts the cap.
    Fps(u32),
    /// Loudness, 0 to 100.
    Volume(u8),
    /// Silence the page, or let it sound again.
    Mute(bool),
    /// One frame of the audio spectrum: the left channel's bands, then the right's.
    Audio(Vec<f32>),
    /// Leave, with exit code 0.
    Quit,
}

impl Command {
    /// Decodes one line from the daemon.
    pub fn decode(line: &str) -> Result<Self, String> {
        let message = wepaper_json::parse(line).map_err(|error| format!("not JSON: {error}"))?;
        let Some(name) = message.get("cmd").as_str() else {
            return Err("no \"cmd\"".to_owned());
        };
        let value = message.get("value");
        match name {
            "properties" => match message.get("values").as_object() {
                Some(values) => Ok(Self::Properties(values.clone())),
                None => Err("properties: \"values\" is not an object".to_owned()),
            },
            "pause" => Ok(Self::Pause),
            "resume" => Ok(Self::Resume),
            "fps" => match value.as_f64() {
                Some(fps) if fps.is_finite() => Ok(Self::Fps(fps.clamp(0.0, FPS_CEILING).round() as u32)),
                _ => Err("fps: \"value\" is not a number".to_owned()),
            },
            "volume" => match value.as_f64() {
                Some(volume) if volume.is_finite() => {
                    Ok(Self::Volume(volume.clamp(0.0, 100.0).round() as u8))
                }
                _ => Err("volume: \"value\" is not a number".to_owned()),
            },
            "mute" => match value.as_bool() {
                Some(muted) => Ok(Self::Mute(muted)),
                None => Err("mute: \"value\" is not a boolean".to_owned()),
            },
            "audio" => {
                let mut samples = Vec::with_capacity(BANDS * 2);
                for channel in ["left", "right"] {
                    let Some(bands) = message.get(channel).as_array() else {
                        return Err(format!("audio: \"{channel}\" is not an array"));
                    };
                    samples.extend(channel_bands(bands));
                }
                Ok(Self::Audio(samples))
            }
            "quit" => Ok(Self::Quit),
            other => Err(format!("unknown command \"{other}\"")),
        }
    }
}

/// Exactly [`BANDS`] levels: pages index the array blindly, so a short frame is padded
/// with silence and anything that is not a finite number counts as silence.
fn channel_bands(bands: &[Value]) -> impl Iterator<Item = f32> {
    (0..BANDS).map(|band| match bands.get(band).and_then(Value::as_f32) {
        Some(level) if level.is_finite() => level,
        _ => 0.0,
    })
}

/// What this plugin tells the daemon.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The page has loaded and the surface is on the output.
    Ready,
    /// The page started (or stopped) listening to the audio spectrum.
    Audio {
        /// Whether spectrum frames are wanted now.
        wanted: bool,
    },
    /// Something went wrong; fatal when the process exits right after.
    Error(String),
}

impl Event {
    /// This event as its line, without the line break.
    pub fn encode(&self) -> String {
        match self {
            Self::Ready => obj! { "event" => "ready" },
            Self::Audio { wanted } => obj! { "event" => "audio", "wanted" => *wanted },
            Self::Error(message) => obj! { "event" => "error", "message" => message },
        }
        .to_json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_without_arguments_decode() {
        assert_eq!(Command::decode(r#"{"cmd":"pause"}"#), Ok(Command::Pause));
        assert_eq!(Command::decode(r#"{"cmd":"resume"}"#), Ok(Command::Resume));
        assert_eq!(Command::decode(r#"{"cmd":"quit"}"#), Ok(Command::Quit));
    }

    #[test]
    fn properties_keep_their_order_and_json_types() {
        let line = r#"{"cmd":"properties","values":{"speed":1.5,"on":true,"tint":"1 0.5 0","name":"\"q\" \u00e9","list":[1,{"a":null}]}}"#;
        let Ok(Command::Properties(values)) = Command::decode(line) else { panic!("did not decode") };
        let names: Vec<&str> = values.iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["speed", "on", "tint", "name", "list"]);
        assert_eq!(values.get("speed"), Some(&Value::Number(1.5)));
        assert_eq!(values.get("on"), Some(&Value::Bool(true)));
        assert_eq!(values.get("tint"), Some(&Value::from("1 0.5 0")));
        assert_eq!(values.get("name"), Some(&Value::from("\"q\" é")));
        assert_eq!(values.get("list").map(Value::to_json).as_deref(), Some(r#"[1,{"a":null}]"#));
    }

    #[test]
    fn properties_without_an_object_are_refused() {
        assert!(Command::decode(r#"{"cmd":"properties"}"#).is_err());
        assert!(Command::decode(r#"{"cmd":"properties","values":[1]}"#).is_err());
    }

    #[test]
    fn fps_and_volume_are_clamped_to_their_ranges() {
        assert_eq!(Command::decode(r#"{"cmd":"fps","value":30}"#), Ok(Command::Fps(30)));
        assert_eq!(Command::decode(r#"{"cmd":"fps","value":-5}"#), Ok(Command::Fps(0)));
        assert_eq!(Command::decode(r#"{"cmd":"fps","value":1e9}"#), Ok(Command::Fps(1000)));
        assert_eq!(Command::decode(r#"{"cmd":"volume","value":40}"#), Ok(Command::Volume(40)));
        assert_eq!(Command::decode(r#"{"cmd":"volume","value":250}"#), Ok(Command::Volume(100)));
        assert_eq!(Command::decode(r#"{"cmd":"volume","value":-1}"#), Ok(Command::Volume(0)));
        assert!(Command::decode(r#"{"cmd":"fps","value":"fast"}"#).is_err());
        assert!(Command::decode(r#"{"cmd":"volume"}"#).is_err());
    }

    #[test]
    fn mute_needs_a_boolean() {
        assert_eq!(Command::decode(r#"{"cmd":"mute","value":true}"#), Ok(Command::Mute(true)));
        assert_eq!(Command::decode(r#"{"cmd":"mute","value":false}"#), Ok(Command::Mute(false)));
        assert!(Command::decode(r#"{"cmd":"mute","value":1}"#).is_err());
    }

    #[test]
    fn an_audio_frame_is_left_then_right() {
        let left: Vec<String> = (0..BANDS).map(|band| format!("{}", band as f32 / 100.0)).collect();
        let right: Vec<String> = (0..BANDS).map(|band| format!("{}", 1.0 + band as f32 / 100.0)).collect();
        let line = format!(r#"{{"cmd":"audio","left":[{}],"right":[{}]}}"#, left.join(","), right.join(","));
        let Ok(Command::Audio(samples)) = Command::decode(&line) else { panic!("did not decode") };
        assert_eq!(samples.len(), 128);
        assert_eq!(samples[0], 0.0);
        assert_eq!(samples[63], 0.63);
        assert_eq!(samples[64], 1.0);
        assert_eq!(samples[127], 1.63);
    }

    #[test]
    fn a_short_or_dirty_audio_frame_is_padded_with_silence() {
        let line = r#"{"cmd":"audio","left":[0.5,"x",null],"right":[]}"#;
        let Ok(Command::Audio(samples)) = Command::decode(line) else { panic!("did not decode") };
        assert_eq!(samples.len(), 128);
        assert_eq!(&samples[..4], &[0.5, 0.0, 0.0, 0.0]);
        assert!(samples[64..].iter().all(|level| *level == 0.0));
    }

    #[test]
    fn an_overlong_audio_frame_is_cut_to_64_bands_a_channel() {
        let long = vec!["1"; 100].join(",");
        let line = format!(r#"{{"cmd":"audio","left":[{long}],"right":[{long}]}}"#);
        let Ok(Command::Audio(samples)) = Command::decode(&line) else { panic!("did not decode") };
        assert_eq!(samples.len(), 128);
    }

    #[test]
    fn audio_without_both_channels_is_refused() {
        assert!(Command::decode(r#"{"cmd":"audio","left":[]}"#).is_err());
    }

    #[test]
    fn lines_that_are_not_commands_are_errors_not_panics() {
        for line in [
            "",
            "   ",
            "nonsense",
            "[]",
            "42",
            "{}",
            r#"{"cmd":5}"#,
            r#"{"cmd":"dance"}"#,
            "{\"cmd\":\"pause\"",
        ] {
            assert!(Command::decode(line).is_err(), "{line:?} decoded");
        }
    }

    #[test]
    fn events_encode_to_one_line_each() {
        assert_eq!(Event::Ready.encode(), r#"{"event":"ready"}"#);
        assert_eq!(Event::Audio { wanted: true }.encode(), r#"{"event":"audio","wanted":true}"#);
        assert_eq!(Event::Audio { wanted: false }.encode(), r#"{"event":"audio","wanted":false}"#);
    }

    #[test]
    fn an_error_message_with_line_breaks_and_quotes_stays_on_one_line() {
        let line = Event::Error("no \"index.html\"\nin C:\\wall".to_owned()).encode();
        assert!(!line.contains('\n'));
        let back = wepaper_json::parse(&line).unwrap();
        assert_eq!(back.get("event").as_str(), Some("error"));
        assert_eq!(back.get("message").as_str(), Some("no \"index.html\"\nin C:\\wall"));
    }
}
