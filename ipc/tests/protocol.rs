//! The wire format: every request survives the trip, and bad ones say why.

use wepaper_ipc::{Request, error, ok, split_path};
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
        Request::Snapshot { output: Some("DP-1".into()), file: "/tmp/x.png".into() },
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
