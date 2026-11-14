//! Particle system documents: what `particles/*.json` says, typed.

use wepaper_json::Value;

use super::emitter::Emitter;
use super::initializer::Initializer;
use super::operator::Operator;
use super::value::{Vec3, extent, number, plain, scale, vec3};

/// The system flag "worldspace".
const FLAG_WORLD_SPACE: u32 = 1;
/// The system flag "sprite sheet: no frame blending".
const FLAG_NO_FRAME_BLENDING: u32 = 2;
/// The system flag "perspective rendering".
const FLAG_PERSPECTIVE: u32 = 4;

/// The most particles one system may hold, whatever its document asks for.
const MOST_PARTICLES: usize = 200_000;
/// The most pieces a rope is cut into between two particles.
const MOST_SUBDIVISIONS: u32 = 8;
/// The most points a rope trail remembers per particle.
const MOST_TRAIL_SEGMENTS: u32 = 64;

/// How a system's particles are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Renderer {
    /// One sprite per particle, facing the screen.
    #[default]
    Sprite,
    /// One sprite per particle, turned along its velocity and stretched by its speed.
    SpriteTrail,
    /// One ribbon through all particles, in birth order.
    Rope,
    /// One ribbon per particle, along the path it took.
    RopeTrail,
}

/// How a sprite sheet plays on a particle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimationMode {
    /// The sheet plays over the particle's life, `sequencemultiplier` times.
    #[default]
    Sequence,
    /// Each particle shows one frame picked at birth.
    RandomFrame,
    /// The sheet plays once and holds its last frame.
    Once,
}

/// A renderer and its settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RendererDef {
    pub(crate) kind: Renderer,
    /// Sprite trail: length per unit of speed. Rope trail: seconds of path kept.
    pub(crate) length: f32,
    /// Sprite trail: the longest stretch, in sprite heights.
    pub(crate) max_length: f32,
    /// Sprite trail: the shortest stretch, in sprite heights.
    pub(crate) min_length: f32,
    /// Rope: straight pieces between two particles.
    pub(crate) subdivision: u32,
    /// Rope trail: points of path kept per particle.
    pub(crate) segments: u32,
}

impl Default for RendererDef {
    fn default() -> Self {
        Self {
            kind: Renderer::Sprite,
            length: 0.05,
            max_length: 10.0,
            min_length: 0.0,
            subdivision: 1,
            segments: 1,
        }
    }
}

impl RendererDef {
    fn parse(doc: &Value) -> Option<Self> {
        let kind = match doc.get("name").as_str()? {
            "sprite" => Renderer::Sprite,
            "spritetrail" => Renderer::SpriteTrail,
            "rope" => Renderer::Rope,
            "ropetrail" => Renderer::RopeTrail,
            _ => return None,
        };
        let whole =
            |key: &str, default: f32, most: u32| (number(doc, key, default).max(1.0) as u32).min(most);
        Some(match kind {
            Renderer::Sprite => Self { kind, ..Self::default() },
            Renderer::SpriteTrail => Self {
                kind,
                length: number(doc, "length", 0.05),
                max_length: number(doc, "maxlength", 10.0),
                min_length: number(doc, "minlength", 0.0),
                ..Self::default()
            },
            Renderer::Rope => {
                Self { kind, subdivision: whole("subdivision", 3.0, MOST_SUBDIVISIONS), ..Self::default() }
            }
            Renderer::RopeTrail => Self {
                kind,
                length: number(doc, "length", 1.0).max(1e-3),
                segments: whole("segments", 8.0, MOST_TRAIL_SEGMENTS).max(2),
                ..Self::default()
            },
        })
    }
}

/// One control point of a system as its document sets it up.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ControlPointDef {
    /// Where it rests, relative to the system (or to the pointer, when it follows it).
    pub offset: Vec3,
    /// Whether it is locked to the pointer.
    pub follows_pointer: bool,
    /// Whether its offset is in scene space instead of relative to the system.
    pub world_space: bool,
}

/// When a child system plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChildKind {
    /// Once, at the parent's origin, for as long as the parent plays.
    #[default]
    Static,
    /// One instance per parent particle, moving with it.
    EventFollow,
    /// One instance where each parent particle is born.
    EventSpawn,
    /// One instance where each parent particle dies.
    EventDeath,
}

/// A child system a document names.
#[derive(Debug, Clone, PartialEq)]
pub struct ChildDef {
    /// The path of the child's particle document.
    pub path: String,
    /// When it plays.
    pub kind: ChildKind,
    /// Where it sits, relative to the parent's origin or to the particle that started it.
    pub origin: Vec3,
    /// Its rotation in radians (not applied yet).
    pub angles: Vec3,
    /// Its scale.
    pub scale: Vec3,
    /// The most instances alive at once; 0 leaves it to the parent's particle count.
    pub max_count: usize,
    /// The chance, 0 to 1, that an event starts an instance.
    pub probability: f32,
}

/// The factors a scene object lays over a system (`instanceoverride`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Instance {
    pub(crate) alpha: f32,
    pub(crate) color: Vec3,
    /// Scales emission and the room for it.
    pub(crate) count: f32,
    pub(crate) lifetime: f32,
    /// Scales time itself.
    pub(crate) rate: f32,
    pub(crate) size: f32,
    /// Scales initial velocities and forces.
    pub(crate) speed: f32,
}

impl Default for Instance {
    fn default() -> Self {
        Self { alpha: 1.0, color: [1.0; 3], count: 1.0, lifetime: 1.0, rate: 1.0, size: 1.0, speed: 1.0 }
    }
}

/// A parsed particle system document.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticleDef {
    material: String,
    max_count: usize,
    start_time: f32,
    flags: u32,
    animation: AnimationMode,
    sequence_multiplier: f32,
    pub(crate) emitters: Vec<Emitter>,
    pub(crate) initializers: Vec<Initializer>,
    pub(crate) operators: Vec<Operator>,
    pub(crate) renderer: RendererDef,
    control_points: [ControlPointDef; 8],
    children: Vec<ChildDef>,
    pub(crate) instance: Instance,
}

impl ParticleDef {
    /// Reads a particle system document. Unknown emitters/initializers/operators are
    /// skipped with one `log::warn` each.
    pub fn parse(doc: &Value) -> Result<Self, String> {
        if doc.as_object().is_none() {
            return Err("a particle system document must be an object".to_owned());
        }
        let material =
            doc.get("material").as_str().ok_or("the particle system names no material")?.to_owned();

        /// Reads one list of components, warning about each that is not understood.
        fn components<T>(doc: &Value, list: &str, parse: impl Fn(&Value) -> Option<T>) -> Vec<T> {
            let mut out = Vec::new();
            for item in doc.get(list).items() {
                match parse(item) {
                    Some(component) => out.push(component),
                    None => log::warn!(
                        "particles: skipping unknown {list} {:?}",
                        item.get("name").as_str().unwrap_or("?")
                    ),
                }
            }
            out
        }

        let mut renderers = components(doc, "renderer", RendererDef::parse);
        if renderers.len() > 1 {
            log::warn!("particles: {} renderers in one system, drawing with the first", renderers.len());
        }
        let renderer = if renderers.is_empty() { RendererDef::default() } else { renderers.swap_remove(0) };

        let mut control_points = [ControlPointDef::default(); 8];
        for point in doc.get("controlpoint").items() {
            let Some(slot) =
                point.get("id").as_i64().and_then(|id| control_points.get_mut(usize::try_from(id).ok()?))
            else {
                continue;
            };
            let flags = number(point, "flags", 0.0) as u32;
            *slot = ControlPointDef {
                offset: vec3(point, "offset", [0.0; 3]),
                follows_pointer: flags & 1 != 0,
                world_space: flags & 2 != 0,
            };
        }

        let children = doc
            .get("children")
            .items()
            .filter_map(|child| {
                let kind = match child.get("type").as_str().unwrap_or("static") {
                    "eventfollow" => ChildKind::EventFollow,
                    "eventspawn" => ChildKind::EventSpawn,
                    "eventdeath" => ChildKind::EventDeath,
                    _ => ChildKind::Static,
                };
                Some(ChildDef {
                    path: child.get("name").as_str()?.to_owned(),
                    kind,
                    origin: vec3(child, "origin", [0.0; 3]),
                    angles: vec3(child, "angles", [0.0; 3]),
                    scale: extent(child, "scale", [1.0; 3]),
                    max_count: number(child, "maxcount", 0.0).clamp(0.0, MOST_PARTICLES as f32) as usize,
                    probability: number(child, "probability", 1.0).clamp(0.0, 1.0),
                })
            })
            .collect();

        Ok(Self {
            material,
            max_count: (number(doc, "maxcount", 100.0).max(1.0) as usize).min(MOST_PARTICLES),
            start_time: number(doc, "starttime", 0.0).max(0.0),
            flags: number(doc, "flags", 0.0) as u32,
            animation: match doc.get("animationmode").as_str() {
                Some("randomframe") => AnimationMode::RandomFrame,
                Some("once") => AnimationMode::Once,
                _ => AnimationMode::Sequence,
            },
            sequence_multiplier: number(doc, "sequencemultiplier", 1.0),
            emitters: components(doc, "emitter", Emitter::parse),
            initializers: components(doc, "initializer", Initializer::parse),
            operators: components(doc, "operator", Operator::parse),
            renderer,
            control_points,
            children,
            instance: Instance::default(),
        })
    }

    /// Applies a scene object's `instanceoverride`: every factor it names is set,
    /// the rest keep what they had. Fields linked to a user property or a script
    /// take the value written in the document; apply again with the new value when
    /// one changes (on a playing system: [`ParticleSystem::apply_override`]).
    ///
    /// [`ParticleSystem::apply_override`]: super::ParticleSystem::apply_override
    pub fn apply_override(&mut self, instance_override: &Value) {
        let instance = &mut self.instance;
        for (key, factor) in [
            ("alpha", &mut instance.alpha),
            ("count", &mut instance.count),
            ("lifetime", &mut instance.lifetime),
            ("rate", &mut instance.rate),
            ("size", &mut instance.size),
            ("speed", &mut instance.speed),
        ] {
            *factor = number(instance_override, key, *factor).max(0.0);
        }
        if !instance_override.get("colorn").is_null() {
            instance.color = vec3(instance_override, "colorn", instance.color);
        } else if !instance_override.get("color").is_null() {
            instance.color =
                vec3(instance_override, "color", scale(instance.color, 255.0)).map(|c| c / 255.0);
        }
        for (index, point) in self.control_points.iter_mut().enumerate() {
            // A point driven by a script has no position to read here; it keeps its own.
            let field = instance_override.get(&format!("controlpoint{index}"));
            if matches!(plain(field), Value::String(_) | Value::Number(_)) {
                point.offset = vec3(instance_override, &format!("controlpoint{index}"), point.offset);
            }
        }
    }

    /// The path of the material document the system is drawn with.
    pub fn material(&self) -> &str {
        &self.material
    }

    /// The child systems the document names.
    pub fn children(&self) -> &[ChildDef] {
        &self.children
    }

    /// The most particles alive at once, with the instance's `count` factor applied.
    pub fn max_count(&self) -> usize {
        ((self.max_count as f32 * self.instance.count).ceil() as usize).clamp(1, MOST_PARTICLES)
    }

    /// Seconds the system is simulated ahead before its first frame.
    pub fn start_time(&self) -> f32 {
        self.start_time
    }

    /// How the particles are drawn.
    pub fn renderer(&self) -> Renderer {
        self.renderer.kind
    }

    /// The control points as the document (and the instance override) set them up.
    pub fn control_points(&self) -> &[ControlPointDef; 8] {
        &self.control_points
    }

    /// Whether particles stay where they were born when the system moves.
    pub fn world_space(&self) -> bool {
        self.flags & FLAG_WORLD_SPACE != 0
    }

    /// Whether the system asks for a perspective projection in a flat scene.
    pub fn perspective(&self) -> bool {
        self.flags & FLAG_PERSPECTIVE != 0
    }

    /// How a sprite sheet plays on the particles.
    pub fn animation(&self) -> AnimationMode {
        self.animation
    }

    /// Whether neighbouring sprite sheet frames are blended (the `SPRITESHEETBLEND`
    /// combo): only a sheet that plays can blend, and the document may turn it off.
    pub fn blends_frames(&self) -> bool {
        self.animation != AnimationMode::RandomFrame && self.flags & FLAG_NO_FRAME_BLENDING == 0
    }

    /// How many times a sprite sheet plays over a particle's life.
    pub(crate) fn sequence_multiplier(&self) -> f32 {
        self.sequence_multiplier
    }

    /// The longest a particle of this system can live, in seconds.
    pub(crate) fn longest_life(&self) -> f32 {
        self.initializers.iter().filter_map(Initializer::longest_life).next_back().unwrap_or(1.0).max(0.0)
            * self.instance.lifetime
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> ParticleDef {
        ParticleDef::parse(&wepaper_json::parse(json).unwrap()).unwrap()
    }

    #[test]
    fn a_document_without_a_material_is_refused_and_an_empty_one_plays_nothing() {
        assert!(ParticleDef::parse(&wepaper_json::parse("{}").unwrap()).is_err());
        assert!(ParticleDef::parse(&wepaper_json::parse("[1, 2]").unwrap()).is_err());
        let bare = parse(r#"{"material":"m.json"}"#);
        assert_eq!(
            (bare.material(), bare.max_count(), bare.renderer(), bare.children().len()),
            ("m.json", 100, Renderer::Sprite, 0)
        );
        assert!(bare.emitters.is_empty() && bare.blends_frames() && !bare.world_space());
    }

    #[test]
    fn unknown_components_are_dropped_and_the_rest_kept() {
        let def = parse(
            r#"{"material":"m.json","maxcount":"12","flags":7,"animationmode":"randomframe",
                "emitter":[{"name":"layerimage"},{"name":"boxrandom"}],
                "initializer":[{"name":"colorlist"},{"name":"sizerandom"},7],
                "operator":[{"name":"boids"},{"name":"movement"},{"name":"alphafade"}],
                "renderer":[{"name":"mesh"},{"name":"ropetrail","length":0.2}],
                "controlpoint":[{"id":1,"flags":1,"offset":"1 2 3"},{"id":2,"flags":2},{"id":40},{"flags":1}],
                "children":[{"name":"a.json","type":"eventdeath","maxcount":4,"scale":"2 2 1"},{"type":"static"},{"name":"b.json"}]}"#,
        );
        assert_eq!((def.emitters.len(), def.initializers.len(), def.operators.len()), (1, 1, 2));
        assert_eq!(
            (def.max_count(), def.renderer(), def.animation()),
            (12, Renderer::RopeTrail, AnimationMode::RandomFrame)
        );
        assert!(def.world_space() && def.perspective() && !def.blends_frames());
        assert_eq!(
            def.control_points()[1],
            ControlPointDef { offset: [1.0, 2.0, 3.0], follows_pointer: true, world_space: false }
        );
        assert!(def.control_points()[2].world_space && !def.control_points()[0].follows_pointer);
        assert_eq!(def.children().len(), 2);
        assert_eq!(
            (def.children()[0].kind, def.children()[0].max_count, def.children()[0].scale),
            (ChildKind::EventDeath, 4, [2.0, 2.0, 1.0])
        );
        assert_eq!((def.children()[1].kind, def.children()[1].probability), (ChildKind::Static, 1.0));
    }

    #[test]
    fn overrides_multiply_and_move_control_points() {
        let mut def = parse(
            r#"{"material":"m.json","maxcount":10,"initializer":[{"name":"lifetimerandom","min":1,"max":4}]}"#,
        );
        def.apply_override(
            &wepaper_json::parse(
                r#"{"alpha":0.5,"count":{"user":"density","value":2.5},"lifetime":2,"rate":0.5,"size":3,"speed":-1,
                    "colorn":"1 0.5 0","controlpoint1":"7 8 9","controlpoint2":{"script":"x"},"id":4}"#,
            )
            .unwrap(),
        );
        assert_eq!(
            def.instance,
            Instance {
                alpha: 0.5,
                color: [1.0, 0.5, 0.0],
                count: 2.5,
                lifetime: 2.0,
                rate: 0.5,
                size: 3.0,
                speed: 0.0
            }
        );
        assert_eq!((def.max_count(), def.longest_life()), (25, 8.0));
        assert_eq!(
            (def.control_points()[1].offset, def.control_points()[2].offset),
            ([7.0, 8.0, 9.0], [0.0; 3])
        );
        def.apply_override(&wepaper_json::parse(r#"{"color":"255 255 51","count":0.5}"#).unwrap());
        assert_eq!((def.instance.color, def.instance.count, def.instance.size), ([1.0, 1.0, 0.2], 0.5, 3.0));
        def.apply_override(&Value::Null);
        assert_eq!((def.instance.count, def.instance.alpha), (0.5, 0.5));
    }
}
