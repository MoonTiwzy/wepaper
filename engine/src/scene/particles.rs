//! Particle layers: a system from [`crate::particles`], drawn with its material.
//!
//! The simulation knows nothing of the scene; this is where a system gets its
//! place, its pointer and its property values, and where its vertices reach the GPU.

use std::collections::HashMap;
use std::rc::Rc;

use wepaper_json::{Map, Value};

use crate::Engine;
use crate::doc::{AsFloats, Blending, MaterialDoc, ParticleObject, Script};
use crate::gpu::{Blend, Gl, Mesh};
use crate::math::Mat4;
use crate::particles::{Environment, ParticleDef, ParticleSystem, Renderer, View};
use crate::read::Files;
use crate::share::Loaded;

use super::bindings::{Bindings, Scripted, Slot};
use super::shade::{PassInput, Shade};

/// How deep child systems may nest: a document that names itself stops here.
const DEEPEST_CHILD: usize = 4;
/// The longest stretch simulated to catch up with the clock, in seconds.
const LONGEST_CATCH_UP: f32 = 6.0;
/// The step that stretch is simulated in.
const CATCH_UP_STEP: f32 = 1.0 / 30.0;

/// One system of a layer (its own, or a child's) as it is drawn.
pub(crate) struct Part {
    pub(crate) shade: Shade,
    pub(crate) blend: Blend,
    /// This frame's vertices; made when the system first has any.
    pub(crate) mesh: Option<Mesh>,
    /// How many vertices this frame has.
    pub(crate) vertices: usize,
    /// `g_RenderVar0`: what the renderer says about its trails.
    pub(crate) trail: [f32; 4],
    /// `g_RenderVar1`: the sprite sheet's frame size and count.
    pub(crate) sheet: [f32; 4],
}

/// A particle layer.
pub(crate) struct ParticleLayer {
    system: ParticleSystem,
    environment: Environment,
    /// The layer's own system, then its children as [`visit`] walks them.
    pub(crate) parts: Vec<Part>,
    /// The object's `instanceoverride` as written: some of its values follow properties.
    overrides: Value,
    /// Whether any value in `overrides` follows a user property.
    follows_properties: bool,
    /// The values of the override a script computes: their names and what they are now.
    scripted: Vec<(String, Vec<f32>)>,
    scratch: Vec<f32>,
}

/// Walks a system and its children, parents first, in the one order parts are kept in.
fn visit<'a>(system: &'a ParticleSystem, each: &mut dyn FnMut(&'a ParticleSystem)) {
    each(system);
    for child in system.children() {
        visit(child, each);
    }
}

/// An `instanceoverride` with every value that follows a user property replaced by
/// the property's current value.
fn resolved(overrides: &Value, properties: &HashMap<String, Value>) -> Value {
    let Some(members) = overrides.as_object() else { return overrides.clone() };
    let mut out = Map::new();
    for (name, value) in members.iter() {
        let linked = match value.get("user") {
            Value::String(property) => Some(property.as_str()),
            link @ Value::Object(_) => link.get("name").as_str(),
            _ => None,
        };
        let current = match (linked.and_then(|property| properties.get(property)), value) {
            (Some(current), _) => current.clone(),
            // Linked, but the property has no value yet; or scripted: what is written holds.
            (None, Value::Object(_)) if !value.get("value").is_null() => value.get("value").clone(),
            _ => value.clone(),
        };
        out.insert(name, current);
    }
    Value::Object(out)
}

fn read_def(files: &Files, path: &str) -> Result<ParticleDef, String> {
    let text = files.read_text(path).map_err(|e| e.to_string())?;
    let document = wepaper_json::parse(&text).map_err(|e| format!("{path}: {e}"))?;
    ParticleDef::parse(&document).map_err(|e| format!("{path}: {e}"))
}

/// Gives `system` the children its document names, and those theirs.
fn attach_children(files: &Files, system: &mut ParticleSystem, depth: usize) {
    let def = system.def().clone();
    for (index, child) in def.children().iter().enumerate() {
        if depth >= DEEPEST_CHILD {
            log::warn!("particles: {} nests deeper than {DEEPEST_CHILD} systems", child.path);
            return;
        }
        match read_def(files, &child.path) {
            Ok(child_def) => {
                if let Some(attached) = system.attach_child(index, Rc::new(child_def)) {
                    attach_children(files, attached, depth + 1);
                }
            }
            Err(why) => log::warn!("particles: child system is left out: {why}"),
        }
    }
}

/// Compiles what draws one system.
fn part(
    engine: &mut Engine,
    files: &Files,
    bindings: &mut Bindings,
    system: &ParticleSystem,
) -> Result<Part, String> {
    let def = system.def();
    let path = def.material();
    let text = files.read_text(path).map_err(|e| e.to_string())?;
    let material = MaterialDoc::parse(&wepaper_json::parse(&text).map_err(|e| format!("{path}: {e}"))?);
    let pass = material.passes.first().ok_or("the particle material has no passes")?;
    // What the renderer needs is the same every frame: ask once, with nothing to draw yet.
    let batch = system.write_vertices(&View::default(), &mut Vec::new());

    let mut combos = pass.combos.clone();
    for (name, value) in batch.combos {
        combos.insert((*name).to_owned(), *value);
    }
    // The sprite sheet, when the system's texture is one.
    let texture = pass.textures.first().and_then(|name| name.as_deref()).and_then(|name| {
        match engine.share.texture(files, name) {
            Ok(Loaded::Picture(texture)) => Some(texture),
            _ => None,
        }
    });
    let sprites = matches!(batch.renderer, Renderer::Sprite | Renderer::SpriteTrail);
    let frame = texture
        .as_ref()
        .filter(|_| sprites)
        .and_then(|texture| Some((texture.sprites.as_ref()?.frames.first().copied()?, texture.clone())));
    let mut sheet = [0.0; 4];
    match &frame {
        Some((first, texture)) => {
            let [stored_w, stored_h, picture_w, _] = texture.resolution;
            let frames = texture.sprites.as_ref().map_or(1, |sheet| sheet.frames.len());
            let (width, height) = (first.width.abs().max(1.0), first.height.abs().max(1.0));
            sheet = [width / stored_w, height / stored_h, frames as f32, height / width];
            combos.insert("SPRITESHEET".to_owned(), 1);
            combos.insert("SPRITESHEETBLEND".to_owned(), i32::from(def.blends_frames()));
            combos.insert("SPRITESHEETBLENDNPOT".to_owned(), i32::from(stored_w != picture_w));
        }
        // A ribbon runs along its texture; frames in it are not for it.
        None => {
            combos.insert("SPRITESHEET".to_owned(), 0);
        }
    }

    let shade = Shade::compile(
        &mut engine.share,
        files,
        &PassInput {
            shader: batch.shader,
            textures: &super::build::choices(&pass.textures),
            combos: &combos,
            constants: &super::build::constants(pass),
            buffers: &[],
            slots: &[],
        },
        bindings,
    )?;
    Ok(Part {
        shade,
        blend: match pass.blending {
            Blending::Additive => Blend::Additive,
            Blending::Premultiplied => Blend::Premultiplied,
            // Particles are never opaque: a material that does not say is translucent.
            Blending::Translucent | Blending::Normal | Blending::Opaque => Blend::Alpha,
        },
        mesh: None,
        vertices: 0,
        trail: batch.render_var0,
        sheet,
    })
}

impl ParticleLayer {
    /// Reads the object's system and its children, and compiles what draws them.
    /// `seed` picks the particles: the same seed plays the same.
    pub(crate) fn build(
        engine: &mut Engine,
        files: &Files,
        bindings: &mut Bindings,
        object: &ParticleObject,
        index: usize,
        seed: u64,
    ) -> Result<Self, String> {
        let mut def = read_def(files, &object.file)?;
        def.apply_override(&resolved(&object.overrides, &bindings.properties));
        let def = Rc::new(def);
        let mut system = ParticleSystem::new(def.clone(), seed);
        attach_children(files, &mut system, 0);

        let mut systems = Vec::new();
        visit(&system, &mut |system| systems.push(system));
        let mut parts = Vec::with_capacity(systems.len());
        for system in systems {
            parts.push(part(engine, files, bindings, system)?);
        }
        let follows_properties = object.overrides.members().any(|(_, value)| !value.get("user").is_null());
        let mut scripted = Vec::new();
        for (name, value) in object.overrides.members() {
            let Some(source) = value.get("script").as_str() else { continue };
            let script =
                Script { source: source.to_owned(), properties: value.get("scriptproperties").clone() };
            let slot = Slot::Instance(index, scripted.len());
            bindings.scripted.push(Scripted {
                slot,
                script,
                layer: index,
                effect: None,
                name: Some(name.to_owned()),
            });
            // What the script starts from: the value written, or 1, which changes nothing.
            let written = value.get("value").as_floats();
            scripted.push((name.to_owned(), if written.is_empty() { vec![1.0] } else { written }));
        }
        Ok(Self {
            environment: Environment::of(&def),
            system,
            parts,
            overrides: object.overrides.clone(),
            follows_properties,
            scripted,
            scratch: Vec::new(),
        })
    }

    /// A user property changed: the values of the override that follow one are applied anew.
    pub(crate) fn properties_changed(&mut self, properties: &HashMap<String, Value>) {
        if self.follows_properties {
            self.system.apply_override(&resolved(&self.overrides, properties));
        }
    }

    /// What the override's scripted value `which` is now.
    #[cfg_attr(not(feature = "script"), allow(dead_code))]
    pub(crate) fn scripted(&self, which: usize) -> &[f32] {
        self.scripted.get(which).map_or(&[], |(_, value)| value.as_slice())
    }

    /// A script computed the override's value `which` anew: a control point moves,
    /// anything else is applied to the playing system.
    #[cfg_attr(not(feature = "script"), allow(dead_code))]
    pub(crate) fn set_scripted(&mut self, which: usize, floats: &[f32]) {
        let Some((name, value)) = self.scripted.get_mut(which) else { return };
        if value.as_slice() == floats {
            return;
        }
        *value = floats.to_vec();
        let point = name.strip_prefix("controlpoint").and_then(|number| number.parse::<usize>().ok());
        match point {
            Some(point) => {
                if let Some(placed) = self.environment.control_points.get_mut(point) {
                    for (to, from) in placed.iter_mut().zip(floats) {
                        *to = *from;
                    }
                }
            }
            None => {
                let written = match floats {
                    [one] => Value::from(*one),
                    many => Value::from(many.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(" ")),
                };
                let mut change = Map::new();
                change.insert(name.as_str(), written);
                self.system.apply_override(&Value::Object(change));
            }
        }
    }

    /// Plays the system on by `seconds` and puts this frame's vertices on the GPU.
    ///
    /// `world` is the layer's place in the scene and `pointer` the pointer in scene
    /// units, when it is over the wallpaper.
    pub(crate) fn update(
        &mut self,
        gl: &Gl,
        seconds: f32,
        time: f32,
        world: &Mat4,
        pointer: Option<[f32; 2]>,
    ) {
        let to_local = world.inverse();
        self.environment.time = time;
        self.environment.pointer = pointer.map(|at| to_local.transform_point([at[0], at[1], 0.0]));
        let def = self.system.def().clone();
        for (placed, point) in self.environment.control_points.iter_mut().zip(def.control_points()) {
            // A point given in scene space stays put there whatever the layer does.
            if point.world_space {
                *placed = to_local.transform_point(point.offset);
            }
        }
        // A long gap (the first frame of a wallpaper shown mid-flight, a stall) is
        // simulated in steps the simulation takes whole.
        let mut left = seconds.clamp(0.0, LONGEST_CATCH_UP);
        while left > 0.0 {
            let step = left.min(CATCH_UP_STEP);
            self.system.advance(step, &self.environment);
            left -= step;
        }

        let view = View::default();
        let mut index = 0;
        let (parts, scratch) = (&mut self.parts, &mut self.scratch);
        visit(&self.system, &mut |system| {
            let Some(part) = parts.get_mut(index) else { return };
            index += 1;
            scratch.clear();
            let batch = system.write_vertices(&view, scratch);
            part.vertices = batch.vertices;
            part.trail = batch.render_var0;
            if batch.vertices == 0 {
                return;
            }
            match &mut part.mesh {
                Some(mesh) => mesh.update(scratch),
                None => match Mesh::new(gl, batch.layout, scratch, None, true) {
                    Ok(mesh) => part.mesh = Some(mesh),
                    Err(error) => {
                        log::warn!("particles: {error}");
                        part.vertices = 0;
                    }
                },
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ribbon_attributes_sit_where_the_simulation_lays_them_out() {
        let at =
            |name: &str| crate::gpu::ATTRIBUTES.iter().find(|(known, _)| *known == name).map(|(_, at)| *at);
        assert_eq!(at("a_TexCoordVec4C3"), Some(crate::particles::A_TEXCOORDVEC4C3));
        assert_eq!(at("a_TexCoordC4"), Some(crate::particles::A_TEXCOORDC4));
        assert_eq!(at("a_PositionVec4"), Some(12));
    }

    #[test]
    fn override_values_follow_their_properties() {
        let overrides = wepaper_json::parse(
            r#"{"count": {"user": "amount", "value": 1}, "rate": {"user": {"name": "speed"}, "value": 2},
                "size": 3, "alpha": {"script": "...", "value": 0.5}, "id": 7}"#,
        )
        .unwrap();
        let mut properties = HashMap::new();
        let plain = resolved(&overrides, &properties);
        assert_eq!(plain.to_json(), r#"{"count":1,"rate":2,"size":3,"alpha":0.5,"id":7}"#);
        properties.insert("amount".to_owned(), Value::from(4));
        properties.insert("speed".to_owned(), Value::from(0.25));
        let driven = resolved(&overrides, &properties);
        assert_eq!(driven.to_json(), r#"{"count":4,"rate":0.25,"size":3,"alpha":0.5,"id":7}"#);
        assert_eq!(resolved(&Value::Null, &properties), Value::Null);
    }
}
