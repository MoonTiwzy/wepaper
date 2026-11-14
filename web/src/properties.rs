//! User property values: what `project.json` ships, what the command line overrides.
//!
//! Values travel as the JSON they are. Wallpaper Engine hands a page booleans as
//! booleans, sliders as numbers, and combos, colours (`"r g b"`) and text as strings,
//! which is how `project.json` stores them, so nothing is converted here.

use std::path::Path;

use wepaper_json::{Map, Value};

/// The value every property of `project` starts with, in the order the file lists them.
///
/// Entries without a value (group headings, text labels) are not properties a page can
/// read and are left out.
pub fn defaults(project: &Value) -> Map {
    project
        .get("general")
        .get("properties")
        .members()
        .filter(|(_, property)| !property.get("value").is_null())
        .map(|(name, property)| (name, property.get("value").clone()))
        .collect()
}

/// The defaults of the wallpaper in `dir`.
///
/// A web wallpaper is a page first: without a readable `project.json` it still shows,
/// just with no properties, so that is a warning for the caller to log, not a failure.
pub fn load(dir: &Path) -> Result<Map, String> {
    let path = dir.join("project.json");
    let bytes = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let project = wepaper_json::parse(&String::from_utf8_lossy(&bytes))
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(defaults(&project))
}

/// Splits a `--property name=json` argument.
///
/// A value that is not JSON is taken as text: `--property title=Hello` should not need
/// two layers of quoting on a command line.
pub fn parse_override(argument: &str) -> Result<(String, Value), String> {
    let Some((name, text)) = argument.split_once('=') else {
        return Err(format!("--property wants name=json, got \"{argument}\""));
    };
    if name.is_empty() {
        return Err(format!("--property wants name=json, got \"{argument}\""));
    }
    let value = wepaper_json::parse(text).unwrap_or_else(|_| Value::from(text));
    Ok((name.to_owned(), value))
}

/// Writes `changes` over `values`: a known property keeps its place, a new one is appended.
pub fn merge<'a>(values: &mut Map, changes: impl IntoIterator<Item = (&'a str, &'a Value)>) {
    for (name, value) in changes {
        values.insert(name, value.clone());
    }
}

/// `values` in the shape `applyUserProperties` receives: `{name: {value: ...}}`.
pub fn listener_shape(values: &Map) -> Value {
    let mut shaped = Map::new();
    for (name, value) in values.iter() {
        let mut property = Map::new();
        property.insert("value", value.clone());
        shaped.insert(name, Value::Object(property));
    }
    Value::Object(shaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &str = r#"{
        "file" : "index.html",
        "general" : {
            "properties" : {
                "dayCycle" : { "options" : [ { "label" : "Cycle", "value" : "0" } ], "type" : "combo", "value" : "0" },
                "schemecolor" : { "type" : "color", "value" : "0.5019607843137255 0 0.5019607843137255" },
                "show_clock" : { "type" : "bool", "value" : true },
                "arcspeed" : { "max" : 150, "min" : 30, "type" : "slider", "value" : 70 },
                "motto" : { "type" : "textinput", "value" : "say \"hi\"" },
                "heading" : { "type" : "group", "text" : "Clock" },
            }
        },
        "type" : "web"
    }"#;

    fn project_defaults() -> Map {
        defaults(&wepaper_json::parse(PROJECT).unwrap())
    }

    #[test]
    fn defaults_are_the_values_of_project_json_untouched() {
        let values = project_defaults();
        assert_eq!(values.get("dayCycle"), Some(&Value::from("0")));
        assert_eq!(values.get("schemecolor"), Some(&Value::from("0.5019607843137255 0 0.5019607843137255")));
        assert_eq!(values.get("show_clock"), Some(&Value::Bool(true)));
        assert_eq!(values.get("arcspeed"), Some(&Value::Number(70.0)));
        assert_eq!(values.get("motto"), Some(&Value::from("say \"hi\"")));
    }

    #[test]
    fn entries_without_a_value_are_not_properties() {
        let values = project_defaults();
        assert_eq!(values.get("heading"), None);
        assert_eq!(values.len(), 5);
    }

    #[test]
    fn a_project_without_properties_has_no_defaults() {
        assert!(defaults(&wepaper_json::parse(r#"{"type":"web"}"#).unwrap()).is_empty());
        assert!(defaults(&wepaper_json::parse(r#"{"general":{"properties":[]}}"#).unwrap()).is_empty());
        assert!(defaults(&Value::Null).is_empty());
    }

    #[test]
    fn an_override_is_parsed_as_json() {
        assert_eq!(parse_override("show_clock=false"), Ok(("show_clock".to_owned(), Value::Bool(false))));
        assert_eq!(parse_override("arcspeed=120.5"), Ok(("arcspeed".to_owned(), Value::Number(120.5))));
        assert_eq!(parse_override(r#"dayCycle="3""#), Ok(("dayCycle".to_owned(), Value::from("3"))));
        assert_eq!(parse_override(r#"tint="1 0 0""#), Ok(("tint".to_owned(), Value::from("1 0 0"))));
    }

    #[test]
    fn an_override_splits_at_the_first_equals_sign() {
        assert_eq!(parse_override(r#"formula="a=b=c""#), Ok(("formula".to_owned(), Value::from("a=b=c"))));
    }

    #[test]
    fn an_override_that_is_not_json_is_text() {
        assert_eq!(parse_override("motto=Hello there"), Ok(("motto".to_owned(), Value::from("Hello there"))));
        assert_eq!(parse_override("motto="), Ok(("motto".to_owned(), Value::from(""))));
    }

    #[test]
    fn an_override_without_a_name_is_refused() {
        assert!(parse_override("no-equals-sign").is_err());
        assert!(parse_override("=5").is_err());
    }

    #[test]
    fn merging_overrides_in_place_and_appends_what_is_new() {
        let mut values = project_defaults();
        let changes: Map =
            [("show_clock", Value::Bool(false)), ("extra", Value::Number(1.0))].into_iter().collect();
        merge(&mut values, changes.iter());
        assert_eq!(values.get("show_clock"), Some(&Value::Bool(false)));
        let names: Vec<&str> = values.iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["dayCycle", "schemecolor", "show_clock", "arcspeed", "motto", "extra"]);
    }

    #[test]
    fn the_listener_gets_each_value_wrapped_in_an_object() {
        let values: Map = [("on", Value::Bool(true)), ("tint", Value::from("0 0 0"))].into_iter().collect();
        assert_eq!(listener_shape(&values).to_json(), r#"{"on":{"value":true},"tint":{"value":"0 0 0"}}"#);
    }
}
