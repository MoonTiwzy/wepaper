//! One drawable pass: a program with everything that feeds it.

use std::rc::Rc;

use wepaper_json::Value;

use crate::doc::{Dyn, FromDoc, floats};
use crate::glsl::Combos;
use crate::gpu::{Program, Uniform};
use crate::read::Files;
use crate::share::{Loaded, Share, SharedTexture};
use crate::video::SharedVideo;

use super::bindings::{Bindings, Slot};

/// Where a texture slot's picture comes from when the pass is drawn.
pub(crate) enum TextureRef {
    /// A texture from the cache.
    Shared(Rc<SharedTexture>),
    /// What the layer's chain has produced so far.
    Previous,
    /// One of the layer's named buffers.
    Buffer(usize),
    /// A copy of the scene as drawn so far.
    Background,
    /// The current frame of a video.
    Video(SharedVideo),
    /// A text layer's rasterised text.
    Text(Rc<super::text::TextSurface>),
    /// The finished picture of another layer, by its id.
    Layer(i64),
}

/// What a texture slot is asked to hold, before it is resolved.
#[derive(Clone)]
pub(crate) enum TextureChoice {
    /// Whatever the shader defaults to.
    Default,
    /// A texture or buffer by name.
    Named(String),
    /// The chain so far.
    Previous,
    /// A named buffer of the layer.
    Buffer(usize),
    /// A text layer's rasterised text.
    Text(Rc<super::text::TextSurface>),
}

/// Where a uniform's value comes from when the pass is drawn.
pub(crate) enum Feed {
    /// A fixed value.
    Const(Vec<f32>),
    /// A value something drives: a slot of the instance's value table.
    Value(usize),
    Time,
    Daytime,
    PointerPosition,
    ParallaxPosition,
    Screen,
    TexelSize,
    TexelSizeHalf,
    Mvp,
    MvpInverse,
    Model,
    ViewProjection,
    Identity,
    Resolution(u32),
    SpriteRotation(u32),
    SpriteTranslation(u32),
    Color4,
    Color,
    Alpha,
    Brightness,
    Audio {
        bands: usize,
        right: bool,
    },
}

/// A pass ready to draw.
pub(crate) struct Shade {
    pub(crate) program: Rc<Program>,
    pub(crate) samplers: Vec<(Uniform, u32, TextureRef)>,
    pub(crate) feeds: Vec<(Uniform, Feed)>,
    /// The pass's constants by the name materials give them, with the feed each is.
    pub(crate) constants: Vec<(String, usize)>,
    /// The pass reads the scene behind the layer.
    pub(crate) wants_background: bool,
    /// The pass reads the audio spectrum.
    pub(crate) wants_audio: bool,
}

impl Shade {
    /// The current value of the constant `feed` is.
    pub(crate) fn constant<'a>(&'a self, feed: usize, values: &'a [Vec<f32>]) -> &'a [f32] {
        match self.feeds.get(feed) {
            Some((_, Feed::Const(value))) => value,
            Some((_, Feed::Value(slot))) => values.get(*slot).map_or(&[], Vec::as_slice),
            _ => &[],
        }
    }

    /// Sets every constant called `name`; whether there was one.
    pub(crate) fn set_constant(&mut self, name: &str, value: &Value, values: &mut [Vec<f32>]) -> bool {
        let mut found = false;
        for (_, feed) in self.constants.iter().filter(|(constant, _)| constant == name) {
            let Some((uniform, feed)) = self.feeds.get_mut(*feed) else { continue };
            let count = components(uniform.kind) * uniform.count.max(1) as usize;
            match feed {
                Feed::Const(old) => *old = value_floats(value, count),
                Feed::Value(slot) => {
                    if let Some(old) = values.get_mut(*slot) {
                        *old = value_floats(value, count);
                    }
                }
                _ => continue,
            }
            found = true;
        }
        found
    }
}

/// What a pass is compiled from.
pub(crate) struct PassInput<'a> {
    pub(crate) shader: &'a str,
    pub(crate) textures: &'a [TextureChoice],
    pub(crate) combos: &'a Combos,
    pub(crate) constants: &'a [(String, Dyn<Value>)],
    /// The names of the layer's buffers, by slot.
    pub(crate) buffers: &'a [String],
}

/// How many floats a uniform of this GL type takes per element.
fn components(kind: u32) -> usize {
    match kind {
        glow::FLOAT | glow::INT | glow::BOOL => 1,
        glow::FLOAT_VEC2 => 2,
        glow::FLOAT_VEC3 => 3,
        glow::FLOAT_VEC4 => 4,
        glow::FLOAT_MAT3 => 9,
        glow::FLOAT_MAT4 => 16,
        glow::FLOAT_MAT4x3 => 12,
        _ => 1,
    }
}

/// A document value as the floats a uniform takes.
pub(crate) fn value_floats(value: &Value, count: usize) -> Vec<f32> {
    let mut out = match value {
        Value::Number(n) => vec![*n as f32],
        Value::Bool(b) => vec![f32::from(u8::from(*b))],
        Value::String(s) => floats(s),
        Value::Array(items) => items.iter().filter_map(f32::from_doc).collect(),
        _ => Vec::new(),
    };
    // One number for a vector means that number in every component.
    if out.len() == 1 && count > 1 && count <= 4 {
        out = vec![out[0]; count];
    }
    out.resize(count, 0.0);
    out
}

/// The feed for a uniform the engine itself provides, by its name.
fn builtin(name: &str) -> Option<Feed> {
    if let Some(rest) = name.strip_prefix("g_Texture") {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        let unit: u32 = rest[..digits].parse().ok()?;
        return match &rest[digits..] {
            "Resolution" => Some(Feed::Resolution(unit)),
            "Rotation" => Some(Feed::SpriteRotation(unit)),
            "Translation" => Some(Feed::SpriteTranslation(unit)),
            "MipMapInfo" => Some(Feed::Const(vec![0.0])),
            _ => None,
        };
    }
    if let Some(rest) = name.strip_prefix("g_AudioSpectrum") {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        return Some(Feed::Audio { bands: rest[..digits].parse().ok()?, right: rest.ends_with("Right") });
    }
    Some(match name {
        "g_ModelViewProjectionMatrix" | "g_EffectModelViewProjectionMatrix" => Feed::Mvp,
        "g_ModelViewProjectionMatrixInverse" => Feed::MvpInverse,
        "g_ModelMatrix" | "g_AltModelMatrix" => Feed::Model,
        "g_ViewProjectionMatrix" | "g_AltViewProjectionMatrix" => Feed::ViewProjection,
        "g_EffectTextureProjectionMatrix"
        | "g_EffectTextureProjectionMatrixInverse"
        | "g_ModelMatrixInverse"
        | "g_NormalModelMatrix" => Feed::Identity,
        "g_Time" => Feed::Time,
        "g_Daytime" => Feed::Daytime,
        "g_PointerPosition" | "g_PointerPositionLast" => Feed::PointerPosition,
        "g_ParallaxPosition" => Feed::ParallaxPosition,
        "g_Screen" => Feed::Screen,
        "g_TexelSize" => Feed::TexelSize,
        "g_TexelSizeHalf" => Feed::TexelSizeHalf,
        "g_Color4" => Feed::Color4,
        "g_Color" => Feed::Color,
        "g_Alpha" | "g_UserAlpha" => Feed::Alpha,
        "g_Brightness" => Feed::Brightness,
        "g_EyePosition" => Feed::Const(vec![0.0, 0.0, 1.0]),
        "g_LightAmbientColor" => Feed::Const(vec![1.0, 1.0, 1.0]),
        _ => return None,
    })
}

impl Shade {
    /// Compiles a pass: picks the combos, gets the program, and decides for every
    /// uniform the driver kept where its value will come from.
    pub(crate) fn compile(
        share: &mut Share,
        files: &Files,
        input: &PassInput<'_>,
        bindings: &mut Bindings,
    ) -> Result<Self, String> {
        let pair = share
            .shader(files, input.shader)
            .ok_or_else(|| format!("shader {} is not in the wallpaper or the assets", input.shader))?;

        // Resolve the textures first: which are bound decides combos.
        let slots =
            input.textures.len().max(pair.declared.samplers.iter().map(|s| s.slot + 1).max().unwrap_or(0));
        let mut textures: Vec<Option<TextureRef>> = Vec::with_capacity(slots);
        let mut bound = Vec::with_capacity(slots);
        let mut wants_background = false;
        for slot in 0..slots {
            let declared = pair.declared.samplers.iter().find(|s| s.slot == slot);
            let resolve =
                |share: &mut Share, name: &str, wants_background: &mut bool| -> Option<TextureRef> {
                    if name == "_rt_FullFrameBuffer" {
                        *wants_background = true;
                        return Some(TextureRef::Background);
                    }
                    if let Some(rest) = name.strip_prefix("_rt_imageLayerComposite_") {
                        // `<id>_a`: the picture of layer <id> after its effects.
                        return rest.split('_').next().and_then(|id| id.parse().ok()).map(TextureRef::Layer);
                    }
                    if name.starts_with("_rt_") || name.starts_with("_alias_") {
                        return input.buffers.iter().position(|b| b == name).map(TextureRef::Buffer);
                    }
                    match share.texture(files, name) {
                        Ok(Loaded::Picture(texture)) => Some(TextureRef::Shared(texture)),
                        Ok(Loaded::Video(video)) => Some(TextureRef::Video(video)),
                        Err(why) => {
                            log::warn!("{why}");
                            None
                        }
                    }
                };
            let chosen = match input.textures.get(slot).unwrap_or(&TextureChoice::Default) {
                TextureChoice::Previous => Some(TextureRef::Previous),
                TextureChoice::Buffer(index) => Some(TextureRef::Buffer(*index)),
                TextureChoice::Text(surface) => Some(TextureRef::Text(surface.clone())),
                TextureChoice::Named(name) => resolve(share, name, &mut wants_background),
                TextureChoice::Default => None,
            };
            bound.push(chosen.is_some());
            let chosen = chosen.or_else(|| {
                declared
                    .and_then(|d| d.default.as_deref())
                    .and_then(|name| resolve(share, name, &mut wants_background))
            });
            textures.push(chosen);
        }

        let mut combos = Combos::new();
        for combo in &pair.declared.combos {
            combos.insert(combo.name.clone(), combo.default);
        }
        for sampler in &pair.declared.samplers {
            if let Some(combo) = &sampler.combo
                && bound.get(sampler.slot).copied().unwrap_or(false)
            {
                combos.insert(combo.clone(), 1);
            }
        }
        for (slot, texture) in textures.iter().enumerate() {
            if let Some(TextureRef::Text(_)) = texture {
                // Rasterised text is one byte a pixel.
                combos.insert(format!("TEX{slot}FORMAT"), 9);
            }
            if let Some(TextureRef::Shared(texture)) = texture {
                combos.insert(format!("TEX{slot}FORMAT"), texture.format_code);
                if slot == 0 && texture.sprites.is_some() {
                    combos.insert("SPRITESHEET".to_owned(), 1);
                }
            }
        }
        for (name, value) in input.combos {
            combos.insert(name.clone(), *value);
        }
        // Shaders written before `g_Color4` take tint and alpha separately; both work.
        combos.entry("VERSION".to_owned()).or_insert(1);

        let program = share.program(&pair, &combos).map_err(|e| format!("shader {}: {e}", input.shader))?;

        let mut samplers = Vec::new();
        let mut feeds = Vec::new();
        let mut constants = Vec::new();
        let mut wants_audio = false;
        for uniform in &program.uniforms {
            if matches!(
                uniform.kind,
                glow::SAMPLER_2D | glow::SAMPLER_3D | glow::SAMPLER_CUBE | glow::SAMPLER_2D_SHADOW
            ) {
                let unit: u32 =
                    uniform.name.strip_prefix("g_Texture").and_then(|n| n.parse().ok()).unwrap_or(0);
                let texture = textures
                    .get_mut(unit as usize)
                    .and_then(Option::take)
                    .unwrap_or_else(|| TextureRef::Shared(share.white()));
                samplers.push((uniform.clone(), unit, texture));
                continue;
            }
            let count = components(uniform.kind) * uniform.count.max(1) as usize;
            let feed = if let Some(feed) = builtin(&uniform.name) {
                wants_audio |= matches!(feed, Feed::Audio { .. });
                feed
            } else {
                let declared = pair.declared.uniforms.iter().find(|u| u.name == uniform.name);
                if let Some(key) = declared.and_then(|d| d.material.as_deref()) {
                    constants.push((key.to_owned(), feeds.len()));
                }
                let constant = declared
                    .and_then(|d| d.material.as_deref())
                    .and_then(|key| input.constants.iter().find(|(name, _)| name == key));
                match (constant, declared) {
                    (Some((_, value)), _) if value.is_driven() => {
                        let slot = bindings.value(value_floats(&value.value, count));
                        bindings.link(value, Slot::Value(slot));
                        Feed::Value(slot)
                    }
                    (Some((_, value)), _) => Feed::Const(value_floats(&value.value, count)),
                    (None, Some(declared)) => Feed::Const(value_floats(&declared.default, count)),
                    (None, None) => Feed::Const(vec![0.0; count]),
                }
            };
            feeds.push((uniform.clone(), feed));
        }
        Ok(Self { program, samplers, feeds, constants, wants_background, wants_audio })
    }
}
