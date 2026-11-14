//! Material documents: which shader draws, with what, blended how.

use std::collections::BTreeMap;

use wepaper_json::Value;

use super::FromDoc;

/// How a pass combines with what is already in its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Blending {
    /// Replace.
    Normal,
    /// Alpha blending.
    #[default]
    Translucent,
    /// Add, scaled by alpha.
    Additive,
    /// Alpha blending of premultiplied colour.
    Premultiplied,
    /// Replace; the layer is drawn opaque.
    Opaque,
}

impl Blending {
    /// The blending a document names; unknown names blend by alpha.
    pub fn from_name(name: &str) -> Self {
        match name {
            "normal" => Blending::Normal,
            "additive" => Blending::Additive,
            "translucent_pa" | "premultiplied" => Blending::Premultiplied,
            "opaque" | "disabled" => Blending::Opaque,
            _ => Blending::Translucent,
        }
    }
}

/// One pass of a material.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MaterialPass {
    /// The shader's name, without `shaders/` and the extension.
    pub shader: String,
    /// How it blends.
    pub blending: Blending,
    /// Textures by slot; `None` leaves the slot to its default.
    pub textures: Vec<Option<String>>,
    /// By slot, the user property whose picture takes the slot when the user gave one.
    pub user_textures: Vec<Option<String>>,
    /// Shader combos the material sets.
    pub combos: BTreeMap<String, i32>,
    /// Shader constants by their material name.
    pub constants: Vec<(String, Value)>,
    /// Whether back faces are culled (`"normal"`); layers are drawn two-sided otherwise.
    pub cull: bool,
    /// Whether the pass tests against depth.
    pub depth_test: bool,
    /// Whether the pass writes depth.
    pub depth_write: bool,
}

/// A material: one or more passes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MaterialDoc {
    /// The passes, in drawing order.
    pub passes: Vec<MaterialPass>,
}

impl MaterialDoc {
    /// Reads a material document.
    pub fn parse(doc: &Value) -> Self {
        Self { passes: doc.get("passes").items().map(MaterialPass::parse).collect() }
    }

    /// Reads the material of a 3D model's mesh. What it leaves out is what a solid
    /// object wants: drawn opaque, front faces only, tested against and written into
    /// depth. A flat layer's material reads the other way round.
    pub fn parse_model(doc: &Value) -> Self {
        Self { passes: doc.get("passes").items().map(|pass| MaterialPass::read(pass, true)).collect() }
    }
}

impl MaterialPass {
    /// Reads one pass.
    pub fn parse(doc: &Value) -> Self {
        Self::read(doc, false)
    }

    fn read(doc: &Value, solid: bool) -> Self {
        let switch = |key: &str, on: &str, off: &str| match doc.get(key).as_str() {
            Some(value) if value == off => false,
            Some(value) => value == on || on.is_empty(),
            None => solid,
        };
        Self {
            shader: doc.get("shader").as_str().unwrap_or("").to_owned(),
            blending: Blending::from_name(doc.get("blending").as_str().unwrap_or(if solid {
                "normal"
            } else {
                "translucent"
            })),
            textures: texture_slots(doc.get("textures")),
            user_textures: texture_slots(doc.get("usertextures")),
            combos: combos(doc.get("combos")),
            constants: doc
                .get("constantshadervalues")
                .members()
                .map(|(k, v)| (k.to_owned(), v.clone()))
                .collect(),
            cull: switch("cullmode", "normal", "nocull"),
            depth_test: switch("depthtest", "", "disabled"),
            depth_write: switch("depthwrite", "", "disabled"),
        }
    }
}

/// A `textures` array: names by slot, with `null` for slots left alone.
pub(crate) fn texture_slots(doc: &Value) -> Vec<Option<String>> {
    doc.items().map(|t| t.as_str().filter(|s| !s.is_empty()).map(str::to_owned)).collect()
}

/// A `combos` object: names to integers.
pub(crate) fn combos(doc: &Value) -> BTreeMap<String, i32> {
    doc.members().filter_map(|(k, v)| Some((k.to_owned(), i32::from_doc(v)?))).collect()
}
