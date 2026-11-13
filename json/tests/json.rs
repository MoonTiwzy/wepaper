use wepaper_json::{Value, obj, parse};

#[test]
fn parses_nested_document() {
    let v = parse(r#"{"a": [1, 2.5, -3e2, true, null, "x"], "b": {"c": "d"}}"#).unwrap();
    assert_eq!(v.get("a").at(0).as_f64(), Some(1.0));
    assert_eq!(v.get("a").at(1).as_f64(), Some(2.5));
    assert_eq!(v.get("a").at(2).as_f64(), Some(-300.0));
    assert_eq!(v.get("a").at(3).as_bool(), Some(true));
    assert!(v.get("a").at(4).is_null());
    assert_eq!(v.get("a").at(5).as_str(), Some("x"));
    assert_eq!(v.get("b").get("c").as_str(), Some("d"));
    assert_eq!(v.get("a").as_array().map(<[Value]>::len), Some(6));
}

#[test]
fn parses_nan_tokens_as_zero() {
    let v = parse(r#"{"a": -nan(ind), "b": nan, "c": [inf, -inf, NaN], "d": 1}"#).unwrap();
    assert_eq!(v.get("a").as_f64(), Some(0.0));
    assert_eq!(v.get("b").as_f64(), Some(0.0));
    assert_eq!(v.get("c").at(1).as_f64(), Some(0.0));
    assert_eq!(v.get("c").at(2).as_f64(), Some(0.0));
    assert_eq!(v.get("d").as_f64(), Some(1.0));
}

#[test]
fn allows_comments_and_trailing_commas() {
    let text = "{\n // a comment\n \"a\": 1, /* block */ \"b\": [1, 2,],\n}";
    let v = parse(text).unwrap();
    assert_eq!(v.get("a").as_i64(), Some(1));
    assert_eq!(v.get("b").as_array().map(<[Value]>::len), Some(2));
}

#[test]
fn strips_bom() {
    assert_eq!(parse("\u{feff}{\"a\": 1}").unwrap().get("a").as_i64(), Some(1));
}

#[test]
fn round_trips_escapes() {
    let original = Value::from("line\nquote\" back\\ tab\t \u{1} snow\u{2603} \u{1f600}");
    let text = original.to_json();
    assert_eq!(parse(&text).unwrap(), original);
    // Surrogate pairs and plain escapes read back too.
    assert_eq!(parse(r#""😀☃\/""#).unwrap().as_str(), Some("\u{1f600}\u{2603}/"));
}

#[test]
fn reports_line_and_column() {
    let error = parse("{\n  \"a\": ?\n}").unwrap_err();
    assert_eq!((error.line, error.column), (2, 8));
    assert!(parse("{\"a\": 1").is_err());
    assert!(parse("").is_err());
    assert!(parse("{} x").is_err());
}

#[test]
fn missing_keys_are_null() {
    let v = parse(r#"{"a": {"b": 1}}"#).unwrap();
    assert!(v.get("x").get("y").at(3).is_null());
    assert_eq!(v.get("a").get("b").as_i64(), Some(1));
    assert_eq!(v.get("a").get("b").as_str(), None);
}

#[test]
fn duplicate_keys_last_wins_and_order_is_kept() {
    let v = parse(r#"{"z": 1, "a": 2, "z": 3}"#).unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["z", "a"]);
    assert_eq!(v.get("z").as_i64(), Some(3));
}

#[test]
fn writes_compact_and_pretty() {
    let v = obj! { "name" => "x", "n" => 3, "half" => 0.5, "on" => true, "list" => vec![Value::from(1), Value::Null] };
    assert_eq!(v.to_json(), r#"{"name":"x","n":3,"half":0.5,"on":true,"list":[1,null]}"#);
    assert_eq!(parse(&v.to_json_pretty()).unwrap(), v);
    assert!(v.to_json_pretty().contains("\n  \"name\": \"x\""));
}

#[test]
fn numbers_in_loose_forms() {
    assert_eq!(parse("+5").unwrap().as_f64(), Some(5.0));
    assert_eq!(parse(".5").unwrap().as_f64(), Some(0.5));
    assert_eq!(parse("5.").unwrap().as_f64(), Some(5.0));
    assert_eq!(parse("1e3").unwrap().as_i64(), Some(1000));
    assert_eq!(parse("3838528067").unwrap().as_i64(), Some(3_838_528_067));
    assert_eq!(Value::from(3_838_528_067_i64).to_json(), "3838528067");
}

#[test]
fn deep_nesting_is_an_error_not_a_crash() {
    let text = "[".repeat(100_000);
    assert!(parse(&text).is_err());
}
