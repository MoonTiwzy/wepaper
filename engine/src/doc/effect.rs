//! Effect documents: a chain of passes over a layer's picture.

use wepaper_json::Value;

use super::FromDoc;

/// A texture slot bound to a buffer instead of a file.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectBind {
    /// `previous` (the chain so far) or a buffer's name.
    pub name: String,
    /// The texture slot.
    pub slot: usize,
}

/// One step of an effect.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EffectPass {
    /// The material that draws the step; empty for a command.
    pub material: String,
    /// The buffer the step draws into, when not the chain's next buffer.
    pub target: Option<String>,
    /// Slots bound to buffers.
    pub binds: Vec<EffectBind>,
    /// `copy`: copy `source` into `target` without drawing.
    pub command: Option<String>,
    /// The buffer a command reads.
    pub source: Option<String>,
}

/// A buffer an effect declares for itself.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectFbo {
    /// The name passes refer to it by.
    pub name: String,
    /// How many times smaller than the layer it is.
    pub scale: f32,
    /// Its pixel format's name.
    pub format: String,
}

/// An effect.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EffectDoc {
    /// The effect's display name.
    pub name: String,
    /// The steps, in order.
    pub passes: Vec<EffectPass>,
    /// The buffers the effect declares.
    pub fbos: Vec<EffectFbo>,
}

impl EffectDoc {
    /// Reads an effect document.
    pub fn parse(doc: &Value) -> Self {
        let text =
            |doc: &Value, key: &str| doc.get(key).as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        let passes = doc
            .get("passes")
            .items()
            .map(|pass| EffectPass {
                material: text(pass, "material").unwrap_or_default(),
                target: text(pass, "target"),
                binds: pass
                    .get("bind")
                    .items()
                    .filter_map(|bind| {
                        Some(EffectBind {
                            name: text(bind, "name")?,
                            slot: i32::from_doc(bind.get("index"))?.max(0) as usize,
                        })
                    })
                    .collect(),
                command: text(pass, "command"),
                source: text(pass, "source"),
            })
            .collect();
        let fbos = doc
            .get("fbos")
            .items()
            .filter_map(|fbo| {
                Some(EffectFbo {
                    name: text(fbo, "name")?,
                    scale: f32::from_doc(fbo.get("scale")).filter(|s| *s > 0.0).unwrap_or(1.0),
                    format: text(fbo, "format").unwrap_or_default(),
                })
            })
            .collect();
        Self { name: text(doc, "name").unwrap_or_default(), passes, fbos }
    }
}
