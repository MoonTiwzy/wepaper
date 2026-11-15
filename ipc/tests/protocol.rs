//! The wire format: every request survives the trip, and bad ones say why.

use wepaper_ipc::{Framing, Request, error, ok, split_path};
use wepaper_json::{Value, obj, parse};

#[test]
fn every_request_round_trips() {
    let requests = [
        Request::Version,
        Request::Get { path: "config/fps".into() },
        Request::Get { path: String::new() },
        Request::Set { path: "outputs/DP-1/wallpaper/properties/tint".into(), value: Value::from("1 0.5 0") },
        Request::Set { path: "config/mute".into(), value: Value::Null },
        Request::List { path: "outputs".into() },
        Request::Reset { path: "outputs/DP-1/wallpaper/properties".into() },
        Request::Show {
            wallpaper: "3566329932".into(),
            output: Some("DP-1".into()),
            properties: Some(vec![("speed".into(), Value::from(2)), ("on".into(), Value::from(true))]),
        },
        Request::Show { wallpaper: "/walls/a b".into(), output: None, properties: None },
        Request::Show { wallpaper: "/walls/a b".into(), output: None, properties: Some(vec![]) },
        Request::Hide { output: None },
        Request::Pause { output: Some("HDMI-A-1".into()) },
        Request::Resume { output: None },
        Request::Snapshot {
            output: Some("DP-1".into()),
            file: "/tmp/x.png".into(),
            look: None,
            size: None,
            raw: false,
        },
        Request::Snapshot {
            output: Some("LC III".into()),
            file: "/tmp/x.png".into(),
            look: Some(Framing { zoom: 0.725, shift_x: 0.15, shift_y: -0.05 }),
            size: Some((420, 420)),
            raw: true,
        },
        Request::Watch,
        Request::Reload,
        Request::Quit,
    ];
    for request in requests {
        let wire = request.to_value().to_json();
        assert!(!wire.contains('\n'), "a request is one line: {wire}");
        assert_eq!(Request::parse(&parse(&wire).unwrap()), Ok(request), "{wire}");
    }
}

#[test]
fn snapshot_look_is_checked() {
    let bad =
        Request::parse(&parse(r#"{"cmd":"snapshot","file":"/tmp/x.png","look":{"zoom":"big"}}"#).unwrap());
    assert!(bad.unwrap_err().contains("look"));
    let half =
        Request::parse(&parse(r#"{"cmd":"snapshot","file":"/tmp/x.png","look":{"zoom":0.5}}"#).unwrap());
    assert_eq!(
        half,
        Ok(Request::Snapshot {
            output: None,
            file: "/tmp/x.png".into(),
            look: Some(Framing { zoom: 0.5, shift_x: 0.0, shift_y: 0.0 }),
            size: None,
            raw: false,
        })
    );
}

#[test]
fn snapshot_format_is_png_or_rgba() {
    let with = |format: &str| {
        Request::parse(&parse(&format!(r#"{{"cmd":"snapshot","file":"/tmp/x","format":{format}}}"#)).unwrap())
    };
    assert!(matches!(with("\"rgba\""), Ok(Request::Snapshot { raw: true, .. })));
    assert!(matches!(with("\"png\""), Ok(Request::Snapshot { raw: false, .. })));
    assert!(with("\"jpeg\"").unwrap_err().contains("format"));
}

#[test]
fn snapshot_size_is_checked() {
    let at = |size: &str| {
        Request::parse(&parse(&format!(r#"{{"cmd":"snapshot","file":"/tmp/x.png","size":{size}}}"#)).unwrap())
    };
    assert!(matches!(at("[420, 236]"), Ok(Request::Snapshot { size: Some((420, 236)), .. })));
    for wrong in ["[0, 10]", "[10]", "[9000, 10]", "\"big\"", "[1.5, 2]"] {
        assert!(at(wrong).unwrap_err().contains("size"), "{wrong}");
    }
}

#[test]
fn unknown_command_is_an_error() {
    assert_eq!(Request::parse(&obj! { "cmd" => "dance" }), Err("unknown command \"dance\"".to_owned()));
    assert!(Request::parse(&Value::from("status")).is_err());
    assert!(Request::parse(&obj! { "path" => "config" }).is_err());
}

#[test]
fn missing_field_names_the_field() {
    assert_eq!(
        Request::parse(&obj! { "cmd" => "set", "path" => "config/fps" }),
        Err("set needs \"value\"".to_owned())
    );
    assert_eq!(Request::parse(&obj! { "cmd" => "set", "value" => 1 }), Err("set needs \"path\"".to_owned()));
    assert_eq!(Request::parse(&obj! { "cmd" => "show" }), Err("show needs \"wallpaper\"".to_owned()));
    assert_eq!(Request::parse(&obj! { "cmd" => "snapshot" }), Err("snapshot needs \"file\"".to_owned()));
    assert!(Request::parse(&obj! { "cmd" => "pause", "output" => 3 }).unwrap_err().contains("output"));
}

#[test]
fn conveniences_of_the_wire_format() {
    // `status` is the whole tree; an output of "*" or null is every output.
    assert_eq!(Request::parse(&obj! { "cmd" => "status" }), Ok(Request::Get { path: String::new() }));
    assert_eq!(
        Request::parse(&obj! { "cmd" => "pause", "output" => "*" }),
        Ok(Request::Pause { output: None })
    );
    assert_eq!(split_path("/outputs//DP-1/paused/"), ["outputs", "DP-1", "paused"]);
    assert!(split_path("").is_empty());
    assert_eq!(ok(Value::from(60)).to_json(), r#"{"ok":true,"value":60}"#);
    assert_eq!(error("no such output").to_json(), r#"{"ok":false,"error":"no such output"}"#);
}
