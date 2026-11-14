//! `scene.json`: the scene's settings and its objects.

use std::collections::BTreeMap;

use wepaper_json::Value;

use super::material::{combos, texture_slots};
use super::{Dyn, FromDoc};

/// The scene-wide settings.
#[derive(Debug, Clone, PartialEq)]
pub struct General {
    /// The colour behind everything.
    pub clear_color: Dyn<[f32; 3]>,
    /// The scene's size in its own units when it is drawn flat; `None` for a perspective scene.
    pub orthogonal: Option<[f32; 2]>,
    /// How far the flat camera is zoomed in.
    pub zoom: Dyn<f32>,
    /// Field of view of a perspective camera, in degrees.
    pub fov: Dyn<f32>,
    /// Near plane of a perspective camera.
    pub near: f32,
    /// Far plane of a perspective camera.
    pub far: f32,
    /// Bloom on or off.
    pub bloom: Dyn<bool>,
    /// How strong the bloom is.
    pub bloom_strength: Dyn<f32>,
    /// How bright a pixel must be to bloom.
    pub bloom_threshold: Dyn<f32>,
    /// Layers shift against the pointer.
    pub parallax: Dyn<bool>,
    /// How far they shift.
    pub parallax_amount: Dyn<f32>,
    /// How slowly the shift follows the pointer.
    pub parallax_delay: Dyn<f32>,
    /// How much the pointer matters.
    pub parallax_mouse_influence: Dyn<f32>,
    /// The camera shakes.
    pub shake: Dyn<bool>,
    /// How far it shakes.
    pub shake_amplitude: Dyn<f32>,
    /// How rough the shaking is.
    pub shake_roughness: Dyn<f32>,
    /// How fast it shakes.
    pub shake_speed: Dyn<f32>,
    /// The scene asks for a high dynamic range pipeline.
    pub hdr: bool,
    /// The light that reaches what faces up, everywhere.
    pub ambient: Dyn<[f32; 3]>,
    /// The light that reaches what faces down; surfaces in between get a mix.
    pub skylight: Dyn<[f32; 3]>,
    /// The bloom of the high dynamic range pipeline.
    pub bloom_hdr: BloomHdr,
}

/// The settings of the bloom a scene with `hdr` has instead of the plain one.
#[derive(Debug, Clone, PartialEq)]
pub struct BloomHdr {
    /// How much of the glow is added back.
    pub strength: Dyn<f32>,
    /// How bright a pixel must be to glow.
    pub threshold: Dyn<f32>,
    /// How softly the threshold cuts in.
    pub feather: f32,
    /// How far the glow spreads between two of its sizes.
    pub scatter: f32,
    /// How many times smaller the glow is taken, at most.
    pub iterations: u32,
}

/// What a light is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightKind {
    /// Shines all around from a point, fading to nothing at its radius.
    Point,
    /// A point that shines into a cone.
    Spot,
    /// A line that shines all around.
    Tube,
    /// Parallel light from far away, along the way the object faces.
    Directional,
}

impl LightKind {
    /// The kind a document names. The editor writes some with an `l` in front
    /// (`ldirectional`); both spellings are taken.
    pub fn from_name(name: &str) -> Option<Self> {
        let kind = |name: &str| match name {
            "point" => Some(LightKind::Point),
            "spot" => Some(LightKind::Spot),
            "tube" => Some(LightKind::Tube),
            "directional" => Some(LightKind::Directional),
            _ => None,
        };
        kind(name).or_else(|| name.strip_prefix('l').and_then(kind))
    }
}

/// A light.
#[derive(Debug, Clone, PartialEq)]
pub struct LightObject {
    /// What it is.
    pub kind: LightKind,
    /// Its colour.
    pub color: Dyn<[f32; 3]>,
    /// How bright it is.
    pub intensity: Dyn<f32>,
    /// How far it reaches.
    pub radius: f32,
    /// How its light falls off towards the radius: 0 not at all, 1 evenly, more is sharper.
    pub exponent: f32,
    /// A spot's cone where it is full, in degrees.
    pub inner_cone: f32,
    /// A spot's cone where it ends, in degrees.
    pub outer_cone: f32,
    /// Whether it casts shadows.
    pub cast_shadow: bool,
}

/// A camera: what a scene is seen through, from where the object is, looking along
/// its own -z with its y up.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraObject {
    /// Its name (`default` for the one a scene starts with).
    pub name: String,
    /// From the bottom of its view to the top, in degrees.
    pub fov: f32,
    /// How far it is zoomed in.
    pub zoom: f32,
    /// The file of the paths it can fly along.
    pub path: Option<String>,
}

/// A 3D model.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelObject {
    /// The `.mdl` file.
    pub file: String,
    /// The animation layers playing on its skeleton.
    pub animation_layers: Value,
}

/// Where a layer's origin sits on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Alignment {
    /// -1 left, 0 centre, 1 right.
    pub horizontal: i8,
    /// -1 bottom, 0 centre, 1 top.
    pub vertical: i8,
}

impl Alignment {
    /// The alignment a document names (`center`, `topleft`, `bottom`, ...).
    pub fn from_name(name: &str) -> Self {
        let horizontal = if name.contains("left") {
            -1
        } else if name.contains("right") {
            1
        } else {
            0
        };
        let vertical = if name.contains("top") {
            1
        } else if name.contains("bottom") {
            -1
        } else {
            0
        };
        Self { horizontal, vertical }
    }
}

/// What a layer changes in one pass of one of its effects.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PassOverride {
    /// Textures by slot.
    pub textures: Vec<Option<String>>,
    /// Combos.
    pub combos: BTreeMap<String, i32>,
    /// Constants by material name.
    pub constants: Vec<(String, Dyn<Value>)>,
}

/// An effect applied to a layer.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectUse {
    /// The effect document's path.
    pub file: String,
    /// The effect's id in the scene.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// Whether it runs.
    pub visible: Dyn<bool>,
    /// What the layer changes in each of its passes.
    pub passes: Vec<PassOverride>,
}

/// An image layer.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageObject {
    /// The model document's path.
    pub image: String,
    /// The size the scene gives the layer.
    pub size: Option<[f32; 2]>,
    /// Opacity.
    pub alpha: Dyn<f32>,
    /// Tint.
    pub color: Dyn<[f32; 3]>,
    /// Brightness.
    pub brightness: Dyn<f32>,
    /// Where the origin sits on the layer.
    pub alignment: Alignment,
    /// How the layer blends with what is under it (0 = by its material).
    pub blend_mode: i32,
    /// The layer starts from a copy of what is under it.
    pub copy_background: bool,
    /// Whether a compose layer hands its input through.
    pub passthrough: Option<bool>,
    /// The effects, in order.
    pub effects: Vec<EffectUse>,
    /// The puppet animation layers.
    pub animation_layers: Value,
}

/// A text layer.
#[derive(Debug, Clone, PartialEq)]
pub struct TextObject {
    /// The text.
    pub text: Dyn<String>,
    /// The font's path, or a system font's name.
    pub font: String,
    /// The size in points.
    pub point_size: Dyn<f32>,
    /// `left`, `center` or `right`.
    pub horizontal_align: String,
    /// `top`, `center` or `bottom`.
    pub vertical_align: String,
    /// Which corner of the box stays put as the text grows.
    pub anchor: String,
    /// Space around the text, in pixels.
    pub padding: f32,
    /// The box the text is laid out in.
    pub size: Option<[f32; 2]>,
    /// Text colour.
    pub color: Dyn<[f32; 3]>,
    /// Opacity.
    pub alpha: Dyn<f32>,
    /// Brightness.
    pub brightness: Dyn<f32>,
    /// The box behind the text is filled.
    pub opaque_background: bool,
    /// The colour it is filled with.
    pub background_color: [f32; 3],
    /// Wrap at `max_width`.
    pub limit_width: bool,
    /// The width to wrap at.
    pub max_width: Dyn<f32>,
    /// Cut off after `max_rows` lines.
    pub limit_rows: bool,
    /// The number of lines to keep.
    pub max_rows: i32,
    /// How the layer blends with what is under it.
    pub blend_mode: i32,
    /// The effects, in order.
    pub effects: Vec<EffectUse>,
}

/// A sound layer.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundObject {
    /// The sound files.
    pub files: Vec<String>,
    /// Volume, 0 to 1.
    pub volume: Dyn<f32>,
    /// `loop`, `random` or `single`.
    pub mode: String,
    /// Shortest pause between random plays, in seconds.
    pub min_time: f32,
    /// Longest pause between random plays.
    pub max_time: f32,
    /// Do not play until a script says so.
    pub start_silent: bool,
}

/// A particle system layer.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticleObject {
    /// The particle system document's path.
    pub file: String,
    /// What this layer changes in the system (`count`, `rate`, `size`, colours, ...).
    pub overrides: Value,
}

/// What an object is.
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectKind {
    /// A picture with effects.
    Image(Box<ImageObject>),
    /// Text.
    Text(Box<TextObject>),
    /// Particles.
    Particle(ParticleObject),
    /// Sound.
    Sound(SoundObject),
    /// A 3D model.
    Model(Box<ModelObject>),
    /// A light.
    Light(Box<LightObject>),
    /// A camera.
    Camera(Box<CameraObject>),
    /// Nothing drawn: a parent for other objects.
    Group,
    /// A kind this version does not draw (`light`, `model`, ...).
    Unsupported(&'static str),
}

/// One object of the scene.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    /// Its id, unique in the scene.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// The object it is attached to.
    pub parent: Option<i64>,
    /// The attachment point of its parent's model it hangs from, by name.
    pub attachment: Option<String>,
    /// Whether it shows.
    pub visible: Dyn<bool>,
    /// Position.
    pub origin: Dyn<[f32; 3]>,
    /// Scale.
    pub scale: Dyn<[f32; 3]>,
    /// Rotation around x, y and z, in radians.
    pub angles: Dyn<[f32; 3]>,
    /// How much parallax moves it.
    pub parallax_depth: Dyn<[f32; 2]>,
    /// Objects whose picture this one reads.
    pub dependencies: Vec<i64>,
    /// What it is.
    pub kind: ObjectKind,
}

/// A scene document.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    /// The settings.
    pub general: General,
    /// The camera's position.
    pub eye: [f32; 3],
    /// What the camera looks at.
    pub center: [f32; 3],
    /// Which way is up for the camera.
    pub up: [f32; 3],
    /// The objects, in drawing order.
    pub objects: Vec<Object>,
}

impl Scene {
    /// Reads a scene document.
    pub fn parse(doc: &Value) -> Self {
        let general = doc.get("general");
        let field = |key: &str, default: f32| Dyn::read(general.get(key), default);
        let flag = |key: &str, default: bool| Dyn::read(general.get(key), default);
        let projection = general.get("orthogonalprojection");
        let orthogonal =
            match (f32::from_doc(projection.get("width")), f32::from_doc(projection.get("height"))) {
                (Some(w), Some(h)) if w > 0.0 && h > 0.0 => Some([w, h]),
                // `auto` takes the size of the first image; the loader resolves it.
                _ if !projection.is_null() => Some([0.0, 0.0]),
                _ => None,
            };
        let vector = |doc: &Value, default: [f32; 3]| <[f32; 3]>::from_doc(doc).unwrap_or(default);
        Self {
            general: General {
                clear_color: Dyn::read(general.get("clearcolor"), [0.0; 3]),
                orthogonal,
                zoom: field("zoom", 1.0),
                fov: field("fov", 50.0),
                near: f32::from_doc(general.get("nearz")).unwrap_or(0.01),
                far: f32::from_doc(general.get("farz")).unwrap_or(10000.0),
                bloom: flag("bloom", false),
                bloom_strength: field("bloomstrength", 2.0),
                bloom_threshold: field("bloomthreshold", 0.65),
                parallax: flag("cameraparallax", false),
                parallax_amount: field("cameraparallaxamount", 0.5),
                parallax_delay: field("cameraparallaxdelay", 0.1),
                parallax_mouse_influence: field("cameraparallaxmouseinfluence", 0.5),
                shake: flag("camerashake", false),
                shake_amplitude: field("camerashakeamplitude", 0.5),
                shake_roughness: field("camerashakeroughness", 1.0),
                shake_speed: field("camerashakespeed", 3.0),
                hdr: bool::from_doc(general.get("hdr")).unwrap_or(false),
                ambient: Dyn::read(general.get("ambientcolor"), [0.2; 3]),
                skylight: Dyn::read(general.get("skylightcolor"), [0.3; 3]),
                bloom_hdr: BloomHdr {
                    strength: field("bloomhdrstrength", 2.0),
                    threshold: field("bloomhdrthreshold", 1.0),
                    feather: f32::from_doc(general.get("bloomhdrfeather")).unwrap_or(0.1),
                    scatter: f32::from_doc(general.get("bloomhdrscatter")).unwrap_or(1.619),
                    iterations: i32::from_doc(general.get("bloomhdriterations")).unwrap_or(8).clamp(1, 16)
                        as u32,
                },
            },
            eye: vector(doc.get("camera").get("eye"), [0.0, 0.0, 1.0]),
            center: vector(doc.get("camera").get("center"), [0.0; 3]),
            up: vector(doc.get("camera").get("up"), [0.0, 1.0, 0.0]),
            objects: doc.get("objects").items().map(Object::parse).collect(),
        }
    }
}

impl Object {
    fn parse(doc: &Value) -> Self {
        let text = |key: &str| doc.get(key).as_str().filter(|s| !s.is_empty());
        let kind = if let Some(image) = text("image") {
            ObjectKind::Image(Box::new(ImageObject::parse(doc, image)))
        } else if let Some(file) = text("particle") {
            ObjectKind::Particle(ParticleObject {
                file: file.to_owned(),
                overrides: doc.get("instanceoverride").clone(),
            })
        } else if !doc.get("text").is_null() {
            ObjectKind::Text(Box::new(TextObject::parse(doc)))
        } else if doc.get("sound").as_array().is_some() {
            ObjectKind::Sound(SoundObject {
                files: doc.get("sound").items().filter_map(|s| s.as_str().map(str::to_owned)).collect(),
                volume: Dyn::read(doc.get("volume"), 1.0),
                mode: text("playbackmode").unwrap_or("loop").to_owned(),
                min_time: f32::from_doc(doc.get("mintime")).unwrap_or(0.0),
                max_time: f32::from_doc(doc.get("maxtime")).unwrap_or(0.0),
                start_silent: bool::from_doc(doc.get("startsilent")).unwrap_or(false),
            })
        } else if let Some(light) = text("light") {
            match LightKind::from_name(light) {
                Some(kind) => ObjectKind::Light(Box::new(LightObject::parse(doc, kind))),
                None => ObjectKind::Unsupported("light"),
            }
        } else if let Some(name) = text("camera") {
            ObjectKind::Camera(Box::new(CameraObject {
                name: name.to_owned(),
                fov: f32::from_doc(doc.get("fov")).unwrap_or(50.0),
                zoom: f32::from_doc(doc.get("zoom")).unwrap_or(1.0),
                path: text("path").map(str::to_owned),
            }))
        } else if let Some(file) = text("model") {
            ObjectKind::Model(Box::new(ModelObject {
                file: file.to_owned(),
                animation_layers: doc.get("animationlayers").clone(),
            }))
        } else {
            ObjectKind::Group
        };
        Self {
            id: doc.get("id").as_i64().unwrap_or(-1),
            name: text("name").unwrap_or("").to_owned(),
            parent: doc.get("parent").as_i64(),
            attachment: text("attachment").map(str::to_owned),
            visible: Dyn::read(doc.get("visible"), true),
            origin: Dyn::read(doc.get("origin"), [0.0; 3]),
            scale: Dyn::read(doc.get("scale"), [1.0; 3]),
            angles: Dyn::read(doc.get("angles"), [0.0; 3]),
            parallax_depth: Dyn::read(doc.get("parallaxDepth"), [0.0; 2]),
            dependencies: doc.get("dependencies").items().filter_map(Value::as_i64).collect(),
            kind,
        }
    }
}

fn effects(doc: &Value) -> Vec<EffectUse> {
    doc.get("effects")
        .items()
        .filter_map(|effect| {
            Some(EffectUse {
                file: effect.get("file").as_str()?.to_owned(),
                id: effect.get("id").as_i64().unwrap_or(-1),
                name: effect.get("name").as_str().unwrap_or("").to_owned(),
                visible: Dyn::read(effect.get("visible"), true),
                passes: effect
                    .get("passes")
                    .items()
                    .map(|pass| PassOverride {
                        textures: texture_slots(pass.get("textures")),
                        combos: combos(pass.get("combos")),
                        constants: pass
                            .get("constantshadervalues")
                            .members()
                            .map(|(name, value)| (name.to_owned(), Dyn::read(value, Value::Null)))
                            .collect(),
                    })
                    .collect(),
            })
        })
        .collect()
}

fn size(doc: &Value) -> Option<[f32; 2]> {
    <[f32; 2]>::from_doc(doc.get("size")).filter(|s| s[0] > 0.0 && s[1] > 0.0)
}

impl LightObject {
    fn parse(doc: &Value, kind: LightKind) -> Self {
        let number = |key: &str, default: f32| f32::from_doc(doc.get(key)).unwrap_or(default);
        Self {
            kind,
            color: Dyn::read(doc.get("color"), [1.0; 3]),
            intensity: Dyn::read(doc.get("intensity"), 1.0),
            radius: number("radius", 1000.0),
            exponent: number("exponent", 1.0),
            inner_cone: number("innercone", 30.0),
            outer_cone: number("outercone", 45.0),
            cast_shadow: bool::from_doc(doc.get("castshadow")).unwrap_or(false),
        }
    }
}

impl ImageObject {
    fn parse(doc: &Value, image: &str) -> Self {
        Self {
            image: image.to_owned(),
            size: size(doc),
            alpha: Dyn::read(doc.get("alpha"), 1.0),
            color: Dyn::read(doc.get("color"), [1.0; 3]),
            brightness: Dyn::read(doc.get("brightness"), 1.0),
            alignment: Alignment::from_name(doc.get("alignment").as_str().unwrap_or("center")),
            blend_mode: i32::from_doc(doc.get("colorBlendMode")).unwrap_or(0),
            copy_background: bool::from_doc(doc.get("copybackground")).unwrap_or(true),
            passthrough: bool::from_doc(doc.get("config").get("passthrough")),
            effects: effects(doc),
            animation_layers: doc.get("animationlayers").clone(),
        }
    }
}

impl TextObject {
    fn parse(doc: &Value) -> Self {
        let text = |key: &str, default: &str| doc.get(key).as_str().unwrap_or(default).to_owned();
        let flag = |key: &str| bool::from_doc(doc.get(key)).unwrap_or(false);
        Self {
            text: Dyn::read(doc.get("text"), String::new()),
            font: text("font", ""),
            point_size: Dyn::read(doc.get("pointsize"), 32.0),
            horizontal_align: text("horizontalalign", "center"),
            vertical_align: text("verticalalign", "center"),
            anchor: text("anchor", "none"),
            padding: f32::from_doc(doc.get("padding")).unwrap_or(0.0),
            size: size(doc),
            color: Dyn::read(doc.get("color"), [1.0; 3]),
            alpha: Dyn::read(doc.get("alpha"), 1.0),
            brightness: Dyn::read(doc.get("brightness"), 1.0),
            opaque_background: flag("opaquebackground"),
            background_color: <[f32; 3]>::from_doc(doc.get("backgroundcolor")).unwrap_or([0.0; 3]),
            limit_width: flag("limitwidth"),
            max_width: Dyn::read(doc.get("maxwidth"), 0.0),
            limit_rows: flag("limitrows"),
            max_rows: i32::from_doc(doc.get("maxrows")).unwrap_or(0),
            blend_mode: i32::from_doc(doc.get("colorBlendMode")).unwrap_or(0),
            effects: effects(doc),
        }
    }
}
