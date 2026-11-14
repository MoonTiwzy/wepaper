//! SceneScript in a scene: the scene as the host scripts read and write.
//!
//! The runtime ([`crate::script`]) knows nothing of scenes. Here every script the
//! scene's values carry is attached to it, the scene answers what scripts ask
//! (layers, user properties, the cursor), and what scripts return is written into
//! the same slots user properties and animations write.

use std::collections::HashMap;

use wepaper_json::Value;

use crate::Engine;
use crate::audio::Playback;
use crate::read::Files;
use crate::script::{
    Binding, CursorEvent, Datum, Environment, Field, Handler, Host, LayerId, MediaEvent, Runtime, ScriptId,
};

use super::bindings::{Change, PuppetField, SceneField, Scripted, Slot};
use super::{FrameInput, LayerKind, SceneInstance};

/// A script and the value it drives.
struct Attached {
    id: ScriptId,
    slot: Slot,
}

/// The scripts of a scene.
pub(crate) struct Scripts {
    runtime: Runtime,
    attached: Vec<Attached>,
    /// What scripts keep in `localStorage`, for as long as the scene plays.
    storage: HashMap<String, String>,
    /// Where the pointer was last frame, and whether its button was down.
    pointer: ([f32; 2], bool),
    /// The layer the pointer is over, and the one its button went down on.
    over: Option<usize>,
    pressed: Option<usize>,
    /// Whether any script handles events that need the layer under the pointer.
    watches_layers: bool,
    /// Whether any script handles what the system's media player does.
    hears_media: bool,
    /// The state of the media player the scripts were last told: its serial.
    media_told: u64,
}

impl Scripts {
    /// Whether any script wants to hear what the system's media player is on.
    pub(crate) fn wants_media(&self) -> bool {
        self.hears_media
    }

    /// Whether a script asked for the audio spectrum.
    pub(crate) fn wants_audio(&self) -> bool {
        self.runtime.wants_audio()
    }
}

/// The scene, for the time scripts run.
struct SceneHost<'a> {
    scene: &'a mut SceneInstance,
    storage: &'a mut HashMap<String, String>,
    /// The wallpaper's files, while the scene is being built: scripts import from them.
    files: Option<&'a Files>,
    environment: Environment,
}

/// A number or a vector as a script sees it.
fn datum(floats: &[f32]) -> Datum {
    let at = |index: usize| f64::from(floats.get(index).copied().unwrap_or(0.0));
    match floats.len() {
        0 => Datum::Null,
        1 => Datum::Number(at(0)),
        2 => Datum::Vec2([at(0), at(1)]),
        3 => Datum::Vec3([at(0), at(1), at(2)]),
        _ => Datum::Vec4([at(0), at(1), at(2), at(3)]),
    }
}

/// The field of its layer a script's slot is, as scripts name it.
fn field<'a>(scripted: &'a Scripted) -> Field<'a> {
    let name = scripted.name.as_deref().unwrap_or("");
    match scripted.slot {
        Slot::LayerOrigin(_) => Field::Origin,
        Slot::LayerScale(_) => Field::Scale,
        Slot::LayerAngles(_) => Field::Angles,
        Slot::LayerVisible(_) => Field::Visible,
        Slot::LayerAlpha(_) => Field::Alpha,
        Slot::LayerColor(_) => Field::Color,
        Slot::LayerText(_) => Field::Text,
        Slot::LayerVolume(_) => Field::Named("volume"),
        Slot::EffectVisible(_, effect) => Field::EffectVisible(effect),
        Slot::Value(_) => match scripted.effect {
            Some(effect) => Field::EffectConstant(effect, name),
            None => Field::Named(name),
        },
        Slot::Scene(_) => Field::Named("scene"),
        Slot::Puppet(_, layer, PuppetField::Rate) => Field::AnimationLayer(layer, "rate"),
        Slot::Puppet(_, layer, PuppetField::Blend) => Field::AnimationLayer(layer, "blend"),
        Slot::Puppet(_, layer, PuppetField::Visible) => Field::AnimationLayer(layer, "visible"),
        Slot::Instance(..) => Field::Instance(name),
    }
}

/// The names scripts know the scene's own values by.
const SCENE_NAMES: [(&str, SceneField); 11] = [
    ("camerazoom", SceneField::Zoom),
    ("clearcolor", SceneField::Clear),
    ("bloom", SceneField::Bloom),
    ("cameraparallax", SceneField::Parallax),
    ("cameraparallaxamount", SceneField::ParallaxAmount),
    ("cameraparallaxdelay", SceneField::ParallaxDelay),
    ("cameraparallaxmouseinfluence", SceneField::ParallaxMouse),
    ("camerashake", SceneField::Shake),
    ("camerashakeamplitude", SceneField::ShakeAmplitude),
    ("camerashakeroughness", SceneField::ShakeRoughness),
    ("camerashakespeed", SceneField::ShakeSpeed),
];

impl SceneInstance {
    /// The slot a layer's field is kept in, for the fields that have one.
    fn slot_of(&self, layer: usize, field: Field<'_>) -> Option<Slot> {
        Some(match field {
            Field::Origin => Slot::LayerOrigin(layer),
            Field::Scale => Slot::LayerScale(layer),
            Field::Angles => Slot::LayerAngles(layer),
            Field::Visible => Slot::LayerVisible(layer),
            Field::Alpha => Slot::LayerAlpha(layer),
            Field::Color => Slot::LayerColor(layer),
            Field::Text => Slot::LayerText(layer),
            Field::Named("volume") if matches!(self.layers.get(layer)?.kind, LayerKind::Sound(_)) => {
                Slot::LayerVolume(layer)
            }
            Field::EffectVisible(effect) => Slot::EffectVisible(layer, effect),
            Field::AnimationLayer(which, name) => Slot::Puppet(
                layer,
                which,
                match name {
                    "rate" => PuppetField::Rate,
                    "blend" => PuppetField::Blend,
                    "visible" => PuppetField::Visible,
                    _ => return None,
                },
            ),
            _ => return None,
        })
    }

    /// What a slot holds now, as a script sees it. Angles are degrees to scripts.
    fn read_slot(&self, slot: Slot) -> Datum {
        let layer = |index: usize| self.layers.get(index);
        let image = |index: usize| match &layer(index)?.kind {
            LayerKind::Image(image) => Some(image),
            _ => None,
        };
        match slot {
            Slot::Value(index) => self.bindings.values.get(index).map_or(Datum::Null, |value| datum(value)),
            Slot::LayerVisible(index) => layer(index).map_or(Datum::Null, |layer| Datum::Bool(layer.visible)),
            Slot::LayerOrigin(index) => layer(index).map_or(Datum::Null, |layer| datum(&layer.origin)),
            Slot::LayerScale(index) => layer(index).map_or(Datum::Null, |layer| datum(&layer.scale)),
            Slot::LayerAngles(index) => {
                layer(index).map_or(Datum::Null, |layer| datum(&layer.angles.map(f32::to_degrees)))
            }
            Slot::LayerAlpha(index) => image(index).map_or(Datum::Null, |image| datum(&[image.alpha])),
            Slot::LayerColor(index) => image(index).map_or(Datum::Null, |image| datum(&image.color)),
            Slot::LayerVolume(index) => match layer(index).map(|layer| &layer.kind) {
                Some(LayerKind::Sound(sound)) => datum(&[sound.volume]),
                _ => Datum::Null,
            },
            Slot::LayerText(index) => image(index)
                .and_then(|image| image.text.as_ref())
                .map_or(Datum::Null, |text| Datum::Text(text.text.clone())),
            Slot::EffectVisible(index, effect) => image(index)
                .and_then(|image| image.effects.get(effect))
                .map_or(Datum::Null, |effect| Datum::Bool(effect.visible)),
            Slot::Scene(field) => match self.scene_value(field) {
                Value::Bool(on) => Datum::Bool(on),
                Value::Number(number) => Datum::Number(number),
                other => datum(&super::shade::value_floats(&other, 3)),
            },
            Slot::Puppet(index, which, field) => image(index)
                .and_then(|image| image.puppet.as_ref()?.layers.get(which))
                .map_or(Datum::Null, |animation| match field {
                    PuppetField::Rate => datum(&[animation.rate]),
                    PuppetField::Blend => datum(&[animation.blend]),
                    PuppetField::Visible => Datum::Bool(animation.visible),
                }),
            Slot::Instance(index, which) => match layer(index).map(|layer| &layer.kind) {
                Some(LayerKind::Particles(particles)) => datum(particles.scripted(which)),
                _ => Datum::Null,
            },
        }
    }

    /// Writes what a script gave for a slot. A value of a kind the slot cannot take is dropped.
    fn write_slot(&mut self, slot: Slot, value: &Datum) {
        if let Slot::Instance(layer, which) = slot {
            if let Some(LayerKind::Particles(particles)) =
                self.layers.get_mut(layer).map(|layer| &mut layer.kind)
                && let Some(floats) = value.floats()
                && floats.iter().all(|number| number.is_finite())
            {
                let floats: Vec<f32> = floats.iter().map(|number| *number as f32).collect();
                particles.set_scripted(which, &floats);
            }
            return;
        }
        let change = if slot.is_flag() {
            match value {
                Datum::Bool(on) => Change::Flag(slot, *on),
                Datum::Number(number) => Change::Flag(slot, *number != 0.0),
                _ => return,
            }
        } else if matches!(slot, Slot::LayerText(_)) {
            match value {
                Datum::Text(text) => Change::Text(slot, text.clone()),
                Datum::Number(number) => Change::Text(slot, number.to_string()),
                _ => return,
            }
        } else {
            let Some(floats) = value.floats() else { return };
            let mut floats: Vec<f32> = floats.iter().map(|number| *number as f32).collect();
            if floats.iter().any(|number| !number.is_finite()) {
                return;
            }
            match slot {
                Slot::LayerAngles(_) => floats.iter_mut().for_each(|angle| *angle = angle.to_radians()),
                // A shader constant keeps its width whatever a script returns.
                Slot::Value(index) => {
                    let Some(width) = self.bindings.values.get(index).map(Vec::len) else { return };
                    floats.resize(width, floats.last().copied().unwrap_or(0.0));
                }
                _ => {}
            }
            Change::Floats(slot, floats)
        };
        self.apply(change);
    }

    /// The layer on top under a point of the scene, among those `wanted` picks.
    fn layer_at(&self, point: [f32; 2], wanted: impl Fn(usize) -> bool) -> Option<usize> {
        self.order.iter().rev().copied().find(|&index| {
            let layer = &self.layers[index];
            let LayerKind::Image(image) = &layer.kind else { return false };
            if !layer.shown || !wanted(index) {
                return false;
            }
            let local = layer.world.inverse().transform_point([point[0], point[1], 0.0]);
            local[0].abs() <= image.size[0] / 2.0 && local[1].abs() <= image.size[1] / 2.0
        })
    }

    /// Attaches every script the scene's values carry, runs their `init` and tells
    /// them the user properties, as Wallpaper Engine does when a wallpaper loads.
    pub(crate) fn attach_scripts(&mut self, files: &Files) {
        let scripted = std::mem::take(&mut self.bindings.scripted);
        if scripted.is_empty() {
            return;
        }
        let runtime = match Runtime::new() {
            Ok(runtime) => runtime,
            Err(error) => {
                log::warn!("scripts are left out: {error}");
                self.skipped.push(format!("scripts: {error}"));
                return;
            }
        };
        let mut scripts = Scripts {
            runtime,
            attached: Vec::with_capacity(scripted.len()),
            storage: HashMap::new(),
            pointer: ([0.5, 0.5], false),
            over: None,
            pressed: None,
            watches_layers: false,
            hears_media: false,
            media_told: 0,
        };
        let environment = self.script_environment(&FrameInput::default(), 0.0, None);
        let mut failed = 0;
        {
            let Scripts { runtime, attached, storage, .. } = &mut scripts;
            let mut host = SceneHost { scene: self, storage, files: Some(files), environment };
            for scripted in &scripted {
                let binding = Binding {
                    layer: LayerId(scripted.layer as u32),
                    field: field(scripted),
                    value: host.scene.read_slot(scripted.slot),
                    properties: &scripted.script.properties,
                };
                match runtime.attach(&scripted.script.source, binding, &mut host) {
                    Ok(id) => attached.push(Attached { id, slot: scripted.slot }),
                    // The runtime said why through the host.
                    Err(_) => failed += 1,
                }
            }
            for (id, value) in runtime.init(&mut host) {
                if let Some(slot) =
                    attached.iter().find(|attached| attached.id == id).map(|attached| attached.slot)
                {
                    host.scene.write_slot(slot, &value);
                }
            }
            let properties: Vec<(String, Value)> =
                host.scene.properties().into_iter().map(|property| (property.name, property.value)).collect();
            runtime.apply_user_properties(&properties, &mut host);
        }
        if failed > 0 {
            self.skipped.push(format!("{failed} of {} scripts could not be loaded", scripted.len()));
        }
        scripts.watches_layers = [
            Handler::CursorEnter,
            Handler::CursorLeave,
            Handler::CursorDown,
            Handler::CursorUp,
            Handler::CursorClick,
        ]
        .into_iter()
        .any(|handler| scripts.runtime.any_handles(handler));
        scripts.hears_media = [Handler::MediaStatus, Handler::MediaPlayback, Handler::MediaProperties]
            .into_iter()
            .any(|handler| scripts.runtime.any_handles(handler));
        self.scripts = Some(Box::new(scripts));
    }

    /// What scripts see of the frame being drawn.
    fn script_environment(&self, input: &FrameInput, seconds: f32, engine: Option<&Engine>) -> Environment {
        let screen = [f64::from(self.viewport.width), f64::from(self.viewport.height)];
        let mut spectrum = [[0.0; 64]; 2];
        if let Some(engine) = engine {
            let mut bands = Vec::with_capacity(64);
            for (side, right) in spectrum.iter_mut().zip([false, true]) {
                engine.listener.spectrum().bands(64, right, &mut bands);
                for (to, from) in side.iter_mut().zip(&bands) {
                    *to = *from;
                }
            }
        }
        Environment {
            time: input.time,
            frame_time: f64::from(seconds),
            time_of_day: f64::from(input.daytime),
            canvas_size: [f64::from(self.scene_size[0]), f64::from(self.scene_size[1])],
            screen_size: screen,
            cursor_scene: self.pointer_in_scene(input.pointer).map(f64::from),
            cursor_screen: [f64::from(input.pointer[0]) * screen[0], f64::from(input.pointer[1]) * screen[1]],
            cursor_down: input.pointer_down,
            spectrum,
        }
    }

    /// Runs the scripts for a frame: timers, the cursor's events, then every
    /// `update`, each result written to the value its script drives.
    ///
    /// `passed` are the events keyframe animations passed this frame: each goes to
    /// the script on the animation's own value.
    pub(crate) fn run_scripts(
        &mut self,
        engine: &Engine,
        input: &FrameInput,
        seconds: f32,
        passed: &[(usize, String, f32)],
    ) {
        let Some(mut scripts) = self.scripts.take() else { return };
        let environment = self.script_environment(input, seconds, Some(engine));
        let (was_at, was_down) = scripts.pointer;
        let moved = was_at != input.pointer;
        let under = if scripts.watches_layers && (moved || was_down != input.pointer_down) {
            let point = [environment.cursor_scene[0] as f32, environment.cursor_scene[1] as f32];
            let watched: Vec<usize> = scripts.attached.iter().filter_map(|a| self.layer_of(a.slot)).collect();
            self.layer_at(point, |index| watched.contains(&index))
        } else {
            scripts.over
        };
        let (playing, heard) = engine.media.now();
        let news = (scripts.hears_media && heard != scripts.media_told).then(|| playing.clone());
        {
            let Scripts { runtime, attached, storage, over, pressed, media_told, .. } = &mut *scripts;
            let mut host = SceneHost { scene: self, storage, files: None, environment };
            runtime.begin_frame(&mut host);

            // The media player moved on: its state, then its track.
            if let Some(playing) = news {
                if *media_told == 0 {
                    runtime.media(MediaEvent::Status { enabled: true }, &mut host);
                }
                *media_told = heard;
                let state = match playing.playback {
                    Playback::Stopped => 0,
                    Playback::Playing => 1,
                    Playback::Paused => 2,
                };
                runtime.media(MediaEvent::Playback { state }, &mut host);
                runtime.media(
                    MediaEvent::Properties {
                        title: &playing.title,
                        artist: &playing.artist,
                        sub_title: "",
                        album_title: &playing.album,
                        album_artist: &playing.artist,
                        genres: "",
                        content_type: "audio",
                    },
                    &mut host,
                );
            }

            let target = |layer: usize| Some(LayerId(layer as u32));
            if moved {
                runtime.cursor(CursorEvent::Move, None, &mut host);
            }
            if under != *over {
                if let Some(left) = *over {
                    runtime.cursor(CursorEvent::Leave, target(left), &mut host);
                }
                if let Some(entered) = under {
                    runtime.cursor(CursorEvent::Enter, target(entered), &mut host);
                }
                *over = under;
            }
            if input.pointer_down && !was_down {
                *pressed = under;
                if let Some(layer) = under {
                    runtime.cursor(CursorEvent::Down, target(layer), &mut host);
                }
            }
            if !input.pointer_down && was_down {
                if let Some(layer) = under {
                    runtime.cursor(CursorEvent::Up, target(layer), &mut host);
                    if *pressed == under {
                        runtime.cursor(CursorEvent::Click, target(layer), &mut host);
                    }
                }
                *pressed = None;
            }

            for (animation, name, frame) in passed {
                let Some(slot) = host.scene.bindings.animations.get(*animation).map(|playing| playing.slot)
                else {
                    continue;
                };
                for attached in attached.iter().filter(|attached| attached.slot == slot) {
                    let value = host.scene.read_slot(slot);
                    if let Some(new) =
                        runtime.animation_event(attached.id, name, f64::from(*frame), value, &mut host)
                    {
                        host.scene.write_slot(slot, &new);
                    }
                }
            }

            for attached in attached.iter() {
                if !runtime.has_update(attached.id) {
                    continue;
                }
                let value = host.scene.read_slot(attached.slot);
                if let Some(new) = runtime.update(attached.id, value, &mut host) {
                    host.scene.write_slot(attached.slot, &new);
                }
            }
        }
        scripts.pointer = (input.pointer, input.pointer_down);
        self.scripts = Some(scripts);
    }

    /// Tells the scripts that a user property changed.
    pub(crate) fn scripts_hear_property(&mut self, name: &str, value: &Value) {
        let Some(mut scripts) = self.scripts.take() else { return };
        let environment = self.script_environment(&FrameInput::default(), 0.0, None);
        {
            let Scripts { runtime, storage, .. } = &mut *scripts;
            let mut host = SceneHost { scene: self, storage, files: None, environment };
            runtime.apply_user_properties(&[(name.to_owned(), value.clone())], &mut host);
        }
        self.scripts = Some(scripts);
    }

    /// The layer a slot belongs to, for the slots that name one.
    fn layer_of(&self, slot: Slot) -> Option<usize> {
        match slot {
            Slot::LayerVisible(layer)
            | Slot::LayerAlpha(layer)
            | Slot::LayerColor(layer)
            | Slot::LayerOrigin(layer)
            | Slot::LayerScale(layer)
            | Slot::LayerAngles(layer)
            | Slot::LayerText(layer)
            | Slot::LayerVolume(layer)
            | Slot::EffectVisible(layer, _)
            | Slot::Puppet(layer, ..)
            | Slot::Instance(layer, _) => Some(layer),
            Slot::Value(_) | Slot::Scene(_) => None,
        }
    }
}

impl SceneHost<'_> {
    fn layer(&self, layer: LayerId) -> Option<&super::Layer> {
        self.scene.layers.get(layer.0 as usize)
    }

    /// The keyframe animation a script means: the one on its own value, or the one
    /// of its layer with this name.
    fn animation(
        &mut self,
        layer: LayerId,
        field: Option<Field<'_>>,
        name: &Datum,
    ) -> Option<&mut super::bindings::Playing> {
        let index = layer.0 as usize;
        let own = field.and_then(|field| self.scene.slot_of(index, field));
        let name = match name {
            Datum::Text(name) => Some(name.as_str()),
            _ => None,
        };
        // A constant of an effect has no slot of its own kind: it is found by its name.
        let constant = match field {
            Some(Field::EffectConstant(effect, constant)) => Some((Some(effect), constant)),
            Some(Field::Named(constant)) => Some((None, constant)),
            _ => None,
        };
        self.scene.bindings.animations.iter_mut().find(|playing| match (name, own, constant) {
            (Some(name), ..) => playing.layer == index && playing.animation.name == name,
            (None, Some(slot), _) => playing.slot == slot,
            (None, None, Some((effect, constant))) => {
                playing.layer == index
                    && playing.effect == effect
                    && playing.name.as_deref() == Some(constant)
            }
            (None, None, None) => playing.layer == index,
        })
    }
}

impl Host for SceneHost<'_> {
    fn layer_named(&self, name: &str) -> Option<LayerId> {
        self.scene.layers.iter().position(|layer| layer.name == name).map(|index| LayerId(index as u32))
    }

    fn layer_with_id(&self, id: i64) -> Option<LayerId> {
        self.scene.layers.iter().position(|layer| layer.id == id).map(|index| LayerId(index as u32))
    }

    fn layers(&self) -> Vec<LayerId> {
        self.scene.order.iter().map(|index| LayerId(*index as u32)).collect()
    }

    fn get(&self, layer: LayerId, field: Field<'_>) -> Datum {
        let index = layer.0 as usize;
        let Some(found) = self.layer(layer) else { return Datum::Null };
        if let Some(slot) = self.scene.slot_of(index, field) {
            return self.scene.read_slot(slot);
        }
        let image = match &found.kind {
            LayerKind::Image(image) => Some(image),
            _ => None,
        };
        match field {
            Field::Name => Datum::Text(found.name.clone()),
            Field::ParallaxDepth => datum(&found.parallax_depth),
            Field::Parent => found.parent.map_or(Datum::Null, |parent| Datum::Layer(LayerId(parent as u32))),
            Field::Size => image.map_or(Datum::Null, |image| datum(&image.size)),
            Field::EffectName(effect) => image
                .and_then(|image| image.effects.get(effect))
                .map_or(Datum::Null, |effect| Datum::Text(effect.name.clone())),
            Field::EffectConstant(effect, name) => image
                .and_then(|image| image.effects.get(effect))
                .and_then(|effect| effect.constant(name, &self.scene.bindings.values))
                .map_or(Datum::Null, datum),
            Field::Named("brightness") => image.map_or(Datum::Null, |image| datum(&[image.brightness])),
            Field::Named("volume") => match &found.kind {
                LayerKind::Sound(sound) => datum(&[sound.volume]),
                _ => Datum::Null,
            },
            _ => Datum::Null,
        }
    }

    fn set(&mut self, layer: LayerId, field: Field<'_>, value: Datum) {
        let index = layer.0 as usize;
        if index >= self.scene.layers.len() {
            return;
        }
        if let Some(slot) = self.scene.slot_of(index, field) {
            self.scene.write_slot(slot, &value);
            return;
        }
        let Some(floats) =
            value.floats().map(|floats| floats.iter().map(|n| *n as f32).collect::<Vec<f32>>())
        else {
            return;
        };
        if floats.iter().any(|number| !number.is_finite()) {
            return;
        }
        let found = &mut self.scene.layers[index];
        match (field, &mut found.kind) {
            (Field::ParallaxDepth, _) if floats.len() >= 2 => found.parallax_depth = [floats[0], floats[1]],
            (Field::EffectConstant(effect, name), LayerKind::Image(image)) => {
                if let Some(effect) = image.effects.get_mut(effect) {
                    effect.set_constant(name, &floats, &mut self.scene.bindings.values);
                }
            }
            (Field::Named("brightness"), LayerKind::Image(image)) => image.brightness = floats[0],
            (Field::Named("volume"), LayerKind::Sound(sound)) => sound.volume = floats[0].clamp(0.0, 1.0),
            _ => {}
        }
    }

    fn user_property(&self, name: &str) -> Value {
        self.scene.bindings.properties.get(name).cloned().unwrap_or(Value::Null)
    }

    fn environment(&self) -> Environment {
        self.environment.clone()
    }

    fn read_script(&self, path: &str) -> Option<String> {
        self.files?.read_text(path).ok()
    }

    fn log(&mut self, level: log::Level, message: &str) {
        log::log!(level, "script: {message}");
    }

    fn storage_get(&self, key: &str) -> Option<String> {
        self.storage.get(key).cloned()
    }

    fn storage_set(&mut self, key: &str, value: Option<&str>) {
        match value {
            Some(value) => self.storage.insert(key.to_owned(), value.to_owned()),
            None => self.storage.remove(key),
        };
    }

    fn scene_get(&self, name: &str) -> Datum {
        match SCENE_NAMES.iter().find(|(known, _)| *known == name) {
            Some((_, field)) => self.scene.read_slot(Slot::Scene(*field)),
            None => Datum::Null,
        }
    }

    fn scene_set(&mut self, name: &str, value: Datum) {
        if let Some((_, field)) = SCENE_NAMES.iter().find(|(known, _)| *known == name) {
            self.scene.write_slot(Slot::Scene(*field), &value);
        }
    }

    fn call(
        &mut self,
        layer: LayerId,
        field: Option<Field<'_>>,
        method: &str,
        arguments: &[Datum],
    ) -> Option<Datum> {
        let index = layer.0 as usize;
        let number = |at: usize| match arguments.get(at) {
            Some(Datum::Number(number)) => Some(*number as f32),
            _ => None,
        };
        match method {
            // Moves the layer to another place in the order layers are drawn in.
            "sortLayer" => {
                let to = number(0)?.max(0.0) as usize;
                let order = &mut self.scene.order;
                let from = order.iter().position(|layer| *layer == index)?;
                let moved = order.remove(from);
                order.insert(to.min(order.len()), moved);
                Some(Datum::Null)
            }
            "getBoneCount" => match &self.layer(layer)?.kind {
                LayerKind::Image(image) => {
                    Some(datum(&[
                        image.puppet.as_ref().map_or(0, |puppet| puppet.shared.puppet.bones.len()) as f32
                    ]))
                }
                _ => Some(datum(&[0.0])),
            },
            "getAnimationLayerCount" => match &self.layer(layer)?.kind {
                LayerKind::Image(image) => {
                    Some(datum(&[image.puppet.as_ref().map_or(0, |puppet| puppet.layers.len()) as f32]))
                }
                _ => Some(datum(&[0.0])),
            },
            "play" | "pause" | "stop" | "isPlaying" => match &mut self.scene.layers.get_mut(index)?.kind {
                LayerKind::Sound(sound) => Some(match method {
                    "play" => {
                        sound.play();
                        Datum::Null
                    }
                    "pause" => {
                        sound.pause();
                        Datum::Null
                    }
                    "stop" => {
                        sound.stop();
                        Datum::Null
                    }
                    _ => Datum::Bool(sound.is_playing()),
                }),
                _ => None,
            },
            _ => {
                let action = method.strip_prefix("animation.")?;
                let name = arguments.first().cloned().unwrap_or(Datum::Null);
                let playing = self.animation(layer, field, &name)?;
                let length = playing.animation.length;
                Some(match action {
                    "play" => {
                        // From an end it cannot leave, it starts over at the other one.
                        if playing.rate >= 0.0 && playing.animation.finished(playing.played) {
                            playing.played = 0.0;
                        } else if playing.rate < 0.0 && playing.played <= 0.0 {
                            playing.played = length;
                        }
                        playing.running = true;
                        Datum::Null
                    }
                    "pause" => {
                        playing.running = false;
                        Datum::Null
                    }
                    "stop" => {
                        playing.running = false;
                        playing.played = 0.0;
                        playing.stale = true;
                        Datum::Null
                    }
                    "isPlaying" => Datum::Bool(playing.running),
                    "getFrame" => datum(&[playing.animation.frame_at(playing.played)]),
                    "setFrame" => {
                        playing.played = number(1)?.clamp(0.0, length);
                        playing.stale = true;
                        Datum::Null
                    }
                    "get" => match arguments.get(1) {
                        Some(Datum::Text(what)) => match what.as_str() {
                            "fps" => datum(&[playing.animation.fps]),
                            "frameCount" => datum(&[length]),
                            "duration" => datum(&[length / playing.animation.fps]),
                            "rate" => datum(&[playing.rate]),
                            "name" => Datum::Text(playing.animation.name.clone()),
                            _ => return None,
                        },
                        _ => return None,
                    },
                    "set" => match arguments.get(1) {
                        Some(Datum::Text(what)) if what == "rate" => {
                            playing.rate = number(2)?;
                            Datum::Null
                        }
                        _ => return None,
                    },
                    _ => return None,
                })
            }
        }
    }
}
