//! What a shader declares in its comments.

use wepaper_json::Value;

/// A compile-time switch.
#[derive(Debug, Clone, PartialEq)]
pub struct ComboDecl {
    /// The macro's name.
    pub name: String,
    /// Its value unless the material says otherwise.
    pub default: i32,
}

/// A uniform fed by a material constant.
#[derive(Debug, Clone, PartialEq)]
pub struct UniformDecl {
    /// The uniform's name in the shader.
    pub name: String,
    /// The key materials and scenes use for it.
    pub material: Option<String>,
    /// Its value when nothing sets it.
    pub default: Value,
}

/// A texture slot.
#[derive(Debug, Clone, PartialEq)]
pub struct SamplerDecl {
    /// The slot: the `N` of `g_TextureN`.
    pub slot: usize,
    /// The texture used when the material binds none.
    pub default: Option<String>,
    /// The combo that is switched on when the material binds a texture here.
    pub combo: Option<String>,
}

/// Everything declared in a shader's comments, both stages merged.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Declared {
    /// The switches.
    pub combos: Vec<ComboDecl>,
    /// The uniforms with material names or defaults.
    pub uniforms: Vec<UniformDecl>,
    /// The texture slots.
    pub samplers: Vec<SamplerDecl>,
}

impl Declared {
    /// Reads the declarations out of shader text whose includes are inlined.
    pub fn scan(text: &str) -> Self {
        let mut declared = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("//") {
                let rest = rest.trim_start();
                if let Some(json) = rest.strip_prefix("[COMBO]")
                    && let Ok(doc) = wepaper_json::parse(json.trim())
                    && let Some(name) = doc.get("combo").as_str()
                {
                    let default = doc.get("default").as_f64().unwrap_or(0.0) as i32;
                    declared.combos.push(ComboDecl { name: name.to_owned(), default });
                }
                continue;
            }
            let Some(rest) = line.strip_prefix("uniform ") else { continue };
            let Some((declaration, comment)) = rest.split_once("//") else { continue };
            let Ok(doc) = wepaper_json::parse(comment.trim()) else { continue };
            let mut words = declaration.trim().trim_end_matches(';').split_whitespace();
            let (Some(kind), Some(name)) = (words.next(), words.next()) else { continue };
            let name = name.split('[').next().unwrap_or(name).to_owned();
            if kind.starts_with("sampler") {
                let Some(slot) = name.strip_prefix("g_Texture").and_then(|n| n.parse().ok()) else { continue };
                declared.samplers.push(SamplerDecl {
                    slot,
                    default: doc.get("default").as_str().filter(|s| !s.is_empty()).map(str::to_owned),
                    combo: doc.get("combo").as_str().map(str::to_owned),
                });
            } else {
                declared.uniforms.push(UniformDecl {
                    name,
                    material: doc.get("material").as_str().map(str::to_owned),
                    default: doc.get("default").clone(),
                });
            }
        }
        declared
    }

    /// Adds another stage's declarations; what is already declared wins.
    pub fn merge(&mut self, other: Self) {
        for combo in other.combos {
            if !self.combos.iter().any(|c| c.name == combo.name) {
                self.combos.push(combo);
            }
        }
        for uniform in other.uniforms {
            if !self.uniforms.iter().any(|u| u.name == uniform.name) {
                self.uniforms.push(uniform);
            }
        }
        for sampler in other.samplers {
            if !self.samplers.iter().any(|s| s.slot == sampler.slot) {
                self.samplers.push(sampler);
            }
        }
    }
}
