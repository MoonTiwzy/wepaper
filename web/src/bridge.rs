//! The plugin's side of the bridge: the script injected into the page and the messages
//! the two exchange.
//!
//! Everything sent to the page is one call, `wepaperBridge.receive(message)`, with the
//! message handed over as JSON text for the page to parse. JSON pasted into a script as
//! an object literal is almost the same thing, and the difference bites: a property
//! named `__proto__` would set a prototype instead of a key. Text parsed by
//! `JSON.parse` has no such corner.

use wepaper_json::{Map, Value, obj};

use crate::properties;

/// The page script: Wallpaper Engine's web API, as `bridge.js` implements it.
const SOURCE: &str = include_str!("bridge.js");

/// The name of the script message handler the page answers through.
pub const HANDLER: &str = "wepaper";

/// What a page starts with, and is given again when it is loaded anew.
#[derive(Debug, Clone, PartialEq)]
pub struct PageState {
    /// The value of every user property.
    pub properties: Map,
    /// The frame rate the page is told about through `applyGeneralProperties`.
    pub fps: u32,
    /// The frame rate its frames are held to; 0 for no cap.
    pub limit: u32,
    /// Whether it starts paused.
    pub paused: bool,
    /// Loudness, 0 to 100.
    pub volume: u8,
}

/// The script to run at document start: the bridge, primed with `state`.
pub fn document_start_script(state: &PageState) -> String {
    format!("{SOURCE}\n{}", init(state))
}

/// The whole state at once, for a document that has just started.
pub fn init(state: &PageState) -> String {
    receive(&obj! {
        "type" => "init",
        "properties" => properties::listener_shape(&state.properties),
        "fps" => state.fps,
        "limit" => state.limit,
        "paused" => state.paused,
        "volume" => gain(state.volume),
    })
}

/// User properties changed: only the ones in `changed`.
pub fn properties_changed(changed: &Map) -> String {
    receive(&obj! { "type" => "properties", "values" => properties::listener_shape(changed) })
}

/// The frame rate changed: `fps` for the page to know, `limit` to hold its frames to.
pub fn frame_rate(fps: u32, limit: u32) -> String {
    receive(&obj! { "type" => "general", "fps" => fps, "limit" => limit })
}

/// The page is paused, or goes on.
pub fn paused(paused: bool) -> String {
    receive(&obj! { "type" => "paused", "value" => paused })
}

/// The master volume changed.
pub fn volume(percent: u8) -> String {
    receive(&obj! { "type" => "volume", "value" => gain(percent) })
}

/// A frame of the audio spectrum for the page's listener.
pub fn audio(samples: &[f32]) -> String {
    let samples: Vec<Value> = samples.iter().map(|level| Value::from(*level)).collect();
    receive(&obj! { "type" => "audio", "samples" => samples })
}

/// A volume in percent as the factor sound is multiplied by.
fn gain(percent: u8) -> f64 {
    f64::from(percent.min(100)) / 100.0
}

/// The script that hands `message` to the bridge.
fn receive(message: &Value) -> String {
    let text = Value::String(message.to_json()).to_json();
    // U+2028 and U+2029 are fine in JSON but ended a line in JavaScript before
    // ES2019; escaped, the script is the same in every engine.
    let text = text.replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029");
    format!("wepaperBridge.receive(JSON.parse({text}));")
}

/// What the page tells the plugin.
#[derive(Debug, Clone, PartialEq)]
pub enum FromPage {
    /// A document has started and its bridge is listening.
    Hello,
    /// An audio listener was registered (`true`) or dropped.
    AudioWanted(bool),
    /// A script error or a resource that did not load, for the log.
    Log(String),
}

impl FromPage {
    /// Decodes what the page posted; `None` for anything the bridge does not send
    /// (the handler is reachable by the wallpaper's own scripts too).
    pub fn decode(text: &str) -> Option<Self> {
        let message = wepaper_json::parse(text).ok()?;
        match message.get("type").as_str()? {
            "hello" => Some(Self::Hello),
            "audio" => message.get("wanted").as_bool().map(Self::AudioWanted),
            "log" => message.get("message").as_str().map(|text| Self::Log(text.to_owned())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::process::{Command, Stdio};

    use super::*;

    /// The JSON text inside `wepaperBridge.receive(JSON.parse(<string literal>));`,
    /// read back the way a JavaScript engine reads the literal.
    fn sent(script: &str) -> Value {
        let literal = script
            .strip_prefix("wepaperBridge.receive(JSON.parse(")
            .and_then(|rest| rest.strip_suffix("));"))
            .expect("the script is one receive call");
        let text = wepaper_json::parse(literal).expect("the argument is a string literal");
        wepaper_json::parse(text.as_str().expect("a string")).expect("the string holds JSON")
    }

    fn tricky_values() -> Map {
        [
            ("quotes", Value::from("she said \"hi\" and 'bye' and `ok`")),
            ("backslash", Value::from("C:\\wall\\paper \\u0041 \\")),
            ("breaks", Value::from("line\nbreak\r\ttab\u{2028}sep\u{2029}end\u{0}nul")),
            ("script", Value::from("</script><script>alert(1)</script> ${x} //")),
            ("unicode", Value::from("右上大尾巴速度 🦊 é")),
            ("__proto__", Value::from("not a prototype")),
            ("with \"quote\" in name", Value::Bool(true)),
            ("nested", wepaper_json::parse(r#"{"a":[1,2.5,{"b":null,"c":"\"x\""}],"d":{}}"#).unwrap()),
            ("number", Value::Number(-0.000125)),
            ("empty", Value::from("")),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn tricky_property_values_survive_the_trip_into_a_script() {
        let values = tricky_values();
        let message = sent(&properties_changed(&values));
        assert_eq!(message.get("type").as_str(), Some("properties"));
        for (name, value) in values.iter() {
            assert_eq!(message.get("values").get(name).get("value"), value, "{name}");
        }
    }

    #[test]
    fn a_script_has_no_raw_line_breaks_or_separators() {
        let script = properties_changed(&tricky_values());
        assert!(!script.contains(['\n', '\r', '\u{2028}', '\u{2029}', '\u{0}']), "{script:?}");
    }

    #[test]
    fn node_reads_tricky_values_exactly_when_it_is_installed() {
        let values = tricky_values();
        let program = format!(
            "let got; const wepaperBridge = {{ receive(message) {{ got = message; }} }};\n{}\n\
             process.stdout.write(JSON.stringify(got));",
            properties_changed(&values)
        );
        let Ok(mut node) = Command::new("node").arg("-").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()
        else {
            eprintln!("node is not installed: skipped");
            return;
        };
        node.stdin.take().unwrap().write_all(program.as_bytes()).unwrap();
        let output = node.wait_with_output().unwrap();
        assert!(output.status.success(), "node refused the script");
        let got = wepaper_json::parse(&String::from_utf8(output.stdout).unwrap()).unwrap();
        for (name, value) in values.iter() {
            // An own key named __proto__ would be lost by an object literal.
            assert_eq!(got.get("values").get(name).get("value"), value, "{name}");
        }
    }

    #[test]
    fn the_document_start_script_carries_the_bridge_and_the_whole_state() {
        let state = PageState {
            properties: [("on", Value::Bool(true)), ("tint", Value::from("1 0 0"))].into_iter().collect(),
            fps: 60,
            limit: 30,
            paused: true,
            volume: 40,
        };
        let script = document_start_script(&state);
        let (source, init) = script.rsplit_once('\n').unwrap();
        assert_eq!(source, SOURCE);
        let message = sent(init);
        assert_eq!(message.get("type").as_str(), Some("init"));
        assert_eq!(message.get("properties").to_json(), r#"{"on":{"value":true},"tint":{"value":"1 0 0"}}"#);
        assert_eq!(message.get("fps").as_i64(), Some(60));
        assert_eq!(message.get("limit").as_i64(), Some(30));
        assert_eq!(message.get("paused").as_bool(), Some(true));
        assert_eq!(message.get("volume").as_f64(), Some(0.4));
    }

    #[test]
    fn the_bridge_defines_the_whole_documented_api() {
        for name in [
            "wallpaperPropertyListener",
            "applyUserProperties",
            "applyGeneralProperties",
            "setPaused",
            "wallpaperRegisterAudioListener",
            "wallpaperRegisterMediaStatusListener",
            "wallpaperRegisterMediaPropertiesListener",
            "wallpaperRegisterMediaThumbnailListener",
            "wallpaperRegisterMediaPlaybackListener",
            "wallpaperRegisterMediaTimelineListener",
            "wallpaperMediaIntegration",
        ] {
            assert!(SOURCE.contains(name), "{name} is missing from bridge.js");
        }
    }

    #[test]
    fn an_audio_frame_is_sent_as_128_numbers() {
        let samples: Vec<f32> = (0..128).map(|band| band as f32 / 128.0).collect();
        let message = sent(&audio(&samples));
        assert_eq!(message.get("samples").as_array().map(<[Value]>::len), Some(128));
        assert_eq!(message.get("samples").at(64).as_f64(), Some(0.5));
    }

    #[test]
    fn volume_is_sent_as_a_gain() {
        assert_eq!(sent(&volume(0)).get("value").as_f64(), Some(0.0));
        assert_eq!(sent(&volume(25)).get("value").as_f64(), Some(0.25));
        assert_eq!(sent(&volume(100)).get("value").as_f64(), Some(1.0));
    }

    #[test]
    fn pause_and_frame_rate_messages_have_their_fields() {
        assert_eq!(sent(&paused(true)).to_json(), r#"{"type":"paused","value":true}"#);
        assert_eq!(sent(&frame_rate(144, 30)).to_json(), r#"{"type":"general","fps":144,"limit":30}"#);
    }

    #[test]
    fn page_messages_decode_and_strangers_are_ignored() {
        assert_eq!(FromPage::decode(r#"{"type":"hello"}"#), Some(FromPage::Hello));
        assert_eq!(FromPage::decode(r#"{"type":"audio","wanted":true}"#), Some(FromPage::AudioWanted(true)));
        assert_eq!(
            FromPage::decode(r#"{"type":"log","message":"x is not defined"}"#),
            Some(FromPage::Log("x is not defined".to_owned()))
        );
        assert_eq!(FromPage::decode(r#"{"type":"audio"}"#), None);
        assert_eq!(FromPage::decode(r#"{"type":"quit"}"#), None);
        assert_eq!(FromPage::decode("hello"), None);
    }
}
