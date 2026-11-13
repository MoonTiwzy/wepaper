//! Image model documents: the thing an image layer points at.

use wepaper_json::Value;

use super::FromDoc;

/// An image layer's model.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelDoc {
    /// The material that draws it.
    pub material: String,
    /// The size the document gives, if it gives one.
    pub size: Option<[f32; 2]>,
    /// Take the size from the material's first texture.
    pub autosize: bool,
    /// Cover the whole scene.
    pub fullscreen: bool,
    /// Draw from what is already on screen instead of an own picture.
    pub passthrough: bool,
    /// A layer of one colour.
    pub solid: bool,
    /// The puppet (`.mdl`) that deforms the layer.
    pub puppet: Option<String>,
    /// The texture is not padded to a power of two.
    pub no_padding: bool,
}

impl ModelDoc {
    /// Reads a model document.
    pub fn parse(doc: &Value) -> Self {
        let flag = |key: &str| bool::from_doc(doc.get(key)).unwrap_or(false);
        let size = match (f32::from_doc(doc.get("width")), f32::from_doc(doc.get("height"))) {
            (Some(w), Some(h)) if w > 0.0 && h > 0.0 => Some([w, h]),
            _ => None,
        };
        Self {
            material: doc.get("material").as_str().unwrap_or("").to_owned(),
            size,
            autosize: flag("autosize"),
            fullscreen: flag("fullscreen"),
            passthrough: flag("passthrough"),
            solid: flag("solidlayer"),
            puppet: doc.get("puppet").as_str().map(str::to_owned),
            no_padding: flag("nopadding"),
        }
    }
}
