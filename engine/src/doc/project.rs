//! `project.json`: what a wallpaper is.

use wepaper_json::Value;

/// The kind of a wallpaper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Layers, effects and scripts rendered by the engine.
    Scene,
    /// A video file.
    Video,
    /// A web page.
    Web,
    /// A program; not playable.
    Application,
    /// Property values for another wallpaper.
    Preset,
    /// Something this version does not know.
    Unknown,
}

impl Kind {
    /// The name used in documents and in the protocol.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Scene => "scene",
            Kind::Video => "video",
            Kind::Web => "web",
            Kind::Application => "application",
            Kind::Preset => "preset",
            Kind::Unknown => "unknown",
        }
    }
}

/// What a user property is, which decides how its value is read and shown.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyKind {
    /// A number in a range.
    Slider {
        /// Smallest value.
        min: f64,
        /// Largest value.
        max: f64,
        /// Step between values, when the document gives one.
        step: Option<f64>,
    },
    /// On or off.
    Bool,
    /// An RGB colour, components 0 to 1.
    Color,
    /// One of several options.
    Combo(Vec<(String, Value)>),
    /// Free text.
    Text,
    /// A file or folder picked by the user.
    File,
    /// A picture picked by the user, used as a texture.
    SceneTexture,
    /// A heading in the property list; holds no value.
    Group,
    /// A kind this version does not know.
    Other(String),
}

/// One user property of a wallpaper.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    /// The name values are linked by.
    pub name: String,
    /// The label shown to the user.
    pub label: String,
    /// What it is.
    pub kind: PropertyKind,
    /// The value the wallpaper ships with.
    pub default: Value,
    /// Where it sorts in the list.
    pub order: f64,
    /// When it is shown, as an expression over other properties.
    pub condition: Option<String>,
}

/// A wallpaper's `project.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    /// What kind of wallpaper this is.
    pub kind: Kind,
    /// The file the wallpaper starts from (`scene.json`, a video, `index.html`).
    pub file: String,
    /// Its title.
    pub title: String,
    /// The preview picture's file name.
    pub preview: Option<String>,
    /// Its workshop id, when it came from the workshop.
    pub workshop_id: Option<String>,
    /// The wallpaper a preset applies to (a workshop id).
    pub dependency: Option<String>,
    /// A preset's property values.
    pub preset: Value,
    /// The user properties, in display order.
    pub properties: Vec<Property>,
}

impl Project {
    /// Reads a `project.json`.
    pub fn parse(doc: &Value) -> Self {
        let text = |key: &str| doc.get(key).as_str().map(str::to_owned);
        let id = |key: &str| match doc.get(key) {
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            Value::Number(n) => Some(format!("{}", *n as u64)),
            _ => None,
        };
        let dependency = id("dependency");
        let kind = match doc.get("type").as_str().map(str::to_lowercase).as_deref() {
            Some("scene") => Kind::Scene,
            Some("video") => Kind::Video,
            Some("web") => Kind::Web,
            Some("application") => Kind::Application,
            Some("preset") => Kind::Preset,
            None if dependency.is_some() => Kind::Preset,
            // Wallpapers from before the type field existed are scenes.
            None => Kind::Scene,
            Some(_) => Kind::Unknown,
        };
        let mut properties: Vec<Property> =
            doc.get("general").get("properties").members().map(|(name, doc)| Property::parse(name, doc)).collect();
        properties.sort_by(|a, b| a.order.total_cmp(&b.order));
        Self {
            kind,
            file: text("file").unwrap_or_else(|| "scene.json".to_owned()),
            title: text("title").unwrap_or_default(),
            preview: text("preview"),
            workshop_id: id("workshopid"),
            dependency,
            preset: doc.get("preset").clone(),
            properties,
        }
    }
}

impl Property {
    fn parse(name: &str, doc: &Value) -> Self {
        let number = |key: &str| doc.get(key).as_f64();
        let kind_name = doc.get("type").as_str().unwrap_or("").to_lowercase();
        let kind = match kind_name.as_str() {
            "slider" => PropertyKind::Slider { min: number("min").unwrap_or(0.0), max: number("max").unwrap_or(1.0), step: number("step") },
            "bool" => PropertyKind::Bool,
            "color" => PropertyKind::Color,
            "combo" => PropertyKind::Combo(
                doc.get("options").items().map(|o| (o.get("label").as_str().unwrap_or("").to_owned(), o.get("value").clone())).collect(),
            ),
            "textinput" => PropertyKind::Text,
            // A `text` property without a value is a label between properties.
            "text" if doc.get("value").is_null() => PropertyKind::Group,
            "text" => PropertyKind::Text,
            "file" | "directory" => PropertyKind::File,
            "scenetexture" => PropertyKind::SceneTexture,
            "group" => PropertyKind::Group,
            _ => PropertyKind::Other(kind_name),
        };
        Self {
            name: name.to_owned(),
            label: doc.get("text").as_str().unwrap_or(name).to_owned(),
            kind,
            default: doc.get("value").clone(),
            order: number("order").or_else(|| number("index")).unwrap_or(f64::MAX),
            condition: doc.get("condition").as_str().map(str::to_owned),
        }
    }
}
