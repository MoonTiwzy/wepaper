//! What a script is attached to: the trait the scene implements and the values that
//! cross it.

use wepaper_json::Value;

/// A layer of the scene, as the host numbers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayerId(pub u32);

/// A value a script reads from the host or gives to it.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Datum {
    /// Nothing: the layer has no such field, or the script said `null`.
    #[default]
    Null,
    /// A flag, like `visible`.
    Bool(bool),
    /// A number, like `alpha`.
    Number(f64),
    /// A string, like `text`.
    Text(String),
    /// Two components, like `size`.
    Vec2([f64; 2]),
    /// Three components, like `origin` or a colour.
    Vec3([f64; 3]),
    /// Four components.
    Vec4([f64; 4]),
    /// Another layer, like `parent`.
    Layer(LayerId),
}

impl Datum {
    /// The components of a number or a vector.
    pub fn floats(&self) -> Option<&[f64]> {
        match self {
            Datum::Number(value) => Some(std::slice::from_ref(value)),
            Datum::Vec2(value) => Some(value),
            Datum::Vec3(value) => Some(value),
            Datum::Vec4(value) => Some(value),
            _ => None,
        }
    }
}

/// Something of a layer a script can read, write or be attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field<'a> {
    /// Position: a `Vec3`.
    Origin,
    /// Scale: a `Vec3`.
    Scale,
    /// Rotation in degrees: a `Vec3`.
    Angles,
    /// Whether the layer is drawn: a `Bool`.
    Visible,
    /// Opacity: a `Number`.
    Alpha,
    /// Tint: a `Vec3`.
    Color,
    /// A text layer's text: a `Text`.
    Text,
    /// Size in pixels: a `Vec2`.
    Size,
    /// The name given in the editor: a `Text`.
    Name,
    /// Parallax strength along x and y: a `Vec2`.
    ParallaxDepth,
    /// The parent layer: a `Layer`, or `Null` without one.
    Parent,
    /// Any other property of the layer, by its name in SceneScript and `scene.json`:
    /// `pointsize`, `font`, `horizontalalign`, `maxwidth`, `alignment`, `perspective`,
    /// `solid`, `volume`, ...
    Named(&'a str),
    /// Whether the layer's effect of this index runs: a `Bool`. `Null` when there is no
    /// such effect.
    EffectVisible(usize),
    /// The name of the layer's effect of this index: a `Text`. `Null` when there is no
    /// such effect, which is how scripts count them.
    EffectName(usize),
    /// A constant of the layer's effect of this index, by its key in the scene's
    /// `constantshadervalues`.
    EffectConstant(usize, &'a str),
    /// An instance override of a particle system: `size`, `rate`, `controlpoint1`, ...
    Instance(&'a str),
    /// A property of the layer's puppet animation layer of this index: `rate`, `blend`, ...
    AnimationLayer(usize, &'a str),
}

impl<'a> Field<'a> {
    /// The field as the prelude names it: a code, an index, a name. `scene.js` holds the
    /// same codes.
    pub(super) fn code(self) -> (i32, usize, Option<&'a str>) {
        match self {
            Field::Origin => (0, 0, None),
            Field::Scale => (1, 0, None),
            Field::Angles => (2, 0, None),
            Field::Visible => (3, 0, None),
            Field::Alpha => (4, 0, None),
            Field::Color => (5, 0, None),
            Field::Text => (6, 0, None),
            Field::Size => (7, 0, None),
            Field::Name => (8, 0, None),
            Field::ParallaxDepth => (9, 0, None),
            Field::Parent => (10, 0, None),
            Field::Named(name) => (20, 0, Some(name)),
            Field::EffectVisible(index) => (21, index, None),
            Field::EffectName(index) => (22, index, None),
            Field::EffectConstant(index, name) => (23, index, Some(name)),
            Field::Instance(name) => (24, 0, Some(name)),
            Field::AnimationLayer(index, name) => (25, index, Some(name)),
        }
    }

    /// The field a code, an index and a name from the prelude stand for.
    pub(super) fn from_code(code: i32, index: usize, name: &'a str) -> Option<Self> {
        Some(match code {
            0 => Field::Origin,
            1 => Field::Scale,
            2 => Field::Angles,
            3 => Field::Visible,
            4 => Field::Alpha,
            5 => Field::Color,
            6 => Field::Text,
            7 => Field::Size,
            8 => Field::Name,
            9 => Field::ParallaxDepth,
            10 => Field::Parent,
            20 => Field::Named(name),
            21 => Field::EffectVisible(index),
            22 => Field::EffectName(index),
            23 => Field::EffectConstant(index, name),
            24 => Field::Instance(name),
            25 => Field::AnimationLayer(index, name),
            _ => return None,
        })
    }
}

/// What scripts see of the world outside the layers, as it is for one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Environment {
    /// Seconds the scene has been running: `engine.runtime`, and the clock of the timers.
    pub time: f64,
    /// Seconds the last frame took: `engine.frametime`.
    pub frame_time: f64,
    /// The time of day from 0 at midnight to 1 at the next: `engine.timeOfDay`.
    pub time_of_day: f64,
    /// The size of the scene: `engine.canvasSize`.
    pub canvas_size: [f64; 2],
    /// The size of the output in pixels: `engine.screenResolution`.
    pub screen_size: [f64; 2],
    /// The cursor in scene coordinates: `input.cursorWorldPosition`.
    pub cursor_scene: [f64; 2],
    /// The cursor on the output in pixels: `input.cursorScreenPosition`.
    pub cursor_screen: [f64; 2],
    /// Whether the left button is held: `input.cursorLeftDown`.
    pub cursor_down: bool,
    /// The audio spectrum in 64 bands from low to high, left channel then right, each
    /// from 0 to 1. Scripts that ask for 32 or 16 bands get neighbours averaged.
    pub spectrum: [[f32; 64]; 2],
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            time: 0.0,
            frame_time: 0.0,
            time_of_day: 0.0,
            canvas_size: [0.0; 2],
            screen_size: [0.0; 2],
            cursor_scene: [0.0; 2],
            cursor_screen: [0.0; 2],
            cursor_down: false,
            spectrum: [[0.0; 64]; 2],
        }
    }
}

/// What a script is attached to and may touch. The scene implements this.
///
/// The methods with a body are the parts of SceneScript a host may leave out: scripts
/// that use them keep running and the runtime notes the member as unsupported.
pub trait Host {
    /// The layer with this name in the editor.
    fn layer_named(&self, name: &str) -> Option<LayerId>;

    /// The layer with this `id` in `scene.json`.
    fn layer_with_id(&self, id: i64) -> Option<LayerId>;

    /// Every layer, in the order they are drawn: what `thisScene.enumerateLayers` and
    /// `getLayerIndex` go by.
    fn layers(&self) -> Vec<LayerId>;

    /// The value of a layer's field, in the type the field's documentation names;
    /// `Null` when the layer has no such field.
    fn get(&self, layer: LayerId, field: Field<'_>) -> Datum;

    /// Changes a layer's field. Scripts assign what they like: a value of another type
    /// than the field's is the host's to convert or drop.
    fn set(&mut self, layer: LayerId, field: Field<'_>, value: Datum);

    /// The current value of a user property; `Null` when the wallpaper has none of this
    /// name. A colour is given as Wallpaper Engine stores it, `"r g b"`: scripts get a
    /// string of three numbers as a `Vec3`.
    fn user_property(&self, name: &str) -> Value;

    /// The environment of the frame being drawn.
    fn environment(&self) -> Environment;

    /// The source of a script file, `scripts/...`, from the wallpaper or the assets.
    fn read_script(&self, path: &str) -> Option<String>;

    /// A line from `console.log` (info), `console.error` (error), or from the runtime
    /// itself about a script it disabled (error) or a member it does not support (debug).
    fn log(&mut self, level: log::Level, message: &str);

    /// A value from `localStorage`. Keys are `screen:<key>` and `global:<key>`, and
    /// `screen#keys` and `global#keys` for the runtime's lists of them.
    fn storage_get(&self, key: &str) -> Option<String>;

    /// Stores a value for `localStorage`, or with `None` removes it.
    fn storage_set(&mut self, key: &str, value: Option<&str>);

    /// A property of the scene itself, by its name in SceneScript: `bloom`,
    /// `clearcolor`, `cameraparallaxamount`, ... `Null` for what the host does not have.
    fn scene_get(&self, name: &str) -> Datum {
        let _ = name;
        Datum::Null
    }

    /// Changes a property of the scene itself.
    fn scene_set(&mut self, name: &str, value: Datum) {
        let _ = (name, value);
    }

    /// Carries out a method a script called on a layer or one of its parts; `None` when
    /// the host does not support it, `Some(Datum::Null)` when it does and the method
    /// returns nothing.
    ///
    /// `method` is the name in the SceneScript reference: `play`, `pause`, `stop`,
    /// `isPlaying`, `emitParticles`, `setParent`, `rotateObjectSpace`, `lookAt`,
    /// `getBoneTransform`, ..., and `sortLayer` and `destroyLayer` for the scene's
    /// methods that act on one layer. Methods of the objects a layer hands out carry a
    /// prefix and get the object's name or index as their first argument (`Null` for the
    /// only one): `animation.play`, `animation.setFrame`, `textureAnimation.getFrame`,
    /// `videoTexture.setCurrentTime`, `animationLayer.stop`, and `<prefix>.get` and
    /// `<prefix>.set` with a property's name for `fps`, `frameCount`, `duration`, `name`,
    /// `rate`, `loop`, `blend` and `visible`.
    ///
    /// `field` is set for `animation.*` on the animation of the very property the
    /// calling script is bound to (`thisObject.getAnimation()` without a name).
    /// Layers among the arguments come as `Datum::Layer`'s number, matrices as a `Text`
    /// of their elements.
    fn call(
        &mut self,
        layer: LayerId,
        field: Option<Field<'_>>,
        method: &str,
        arguments: &[Datum],
    ) -> Option<Datum> {
        let _ = (layer, field, method, arguments);
        None
    }
}

impl std::fmt::Display for Field<'_> {
    /// The field as a script's author would name it, for messages.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Field::Origin => f.write_str("origin"),
            Field::Scale => f.write_str("scale"),
            Field::Angles => f.write_str("angles"),
            Field::Visible => f.write_str("visible"),
            Field::Alpha => f.write_str("alpha"),
            Field::Color => f.write_str("color"),
            Field::Text => f.write_str("text"),
            Field::Size => f.write_str("size"),
            Field::Name => f.write_str("name"),
            Field::ParallaxDepth => f.write_str("parallaxDepth"),
            Field::Parent => f.write_str("parent"),
            Field::Named(name) => f.write_str(name),
            Field::EffectVisible(index) => write!(f, "effect {index} visible"),
            Field::EffectName(index) => write!(f, "effect {index} name"),
            Field::EffectConstant(index, name) => write!(f, "effect {index} {name}"),
            Field::Instance(name) => write!(f, "instance {name}"),
            Field::AnimationLayer(index, name) => write!(f, "animation layer {index} {name}"),
        }
    }
}
