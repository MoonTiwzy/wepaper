//! The simulation: particles are born, age, are moved by operators and die.

use std::rc::Rc;

use wepaper_json::Value;

use super::def::{ChildKind, Instance, ParticleDef, Renderer};
use super::emitter::Emitter;
use super::initializer::Birth;
use super::operator::Frame;
use super::particle::{ControlPoint, Particle};
use super::rng::Rng;
use super::value::{Vec3, add, mul, scale};
use super::{Environment, def::ControlPointDef};

/// The longest step one `advance` simulates: after a pause the system resumes
/// instead of leaping.
const LONGEST_STEP: f32 = 0.1;
/// The step a system is simulated ahead with before its first frame.
const PREWARM_STEP: f32 = 1.0 / 30.0;
/// The most steps simulated ahead; longer start times use coarser steps.
const MOST_PREWARM_STEPS: f32 = 600.0;
/// The most particles a child system holds over all its instances.
const MOST_CHILD_PARTICLES: usize = 100_000;

/// What keeps a source going.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Lasts {
    /// The slot is not in use.
    Free,
    /// For as long as the system plays.
    Forever,
    /// While the parent particle with this id lives; the source moves with it.
    WithParticle(u64),
    /// Until its emitters have had their say.
    UntilSpent,
    /// It emits no more; its particles live out their lives.
    Ended,
}

/// One place a system emits from. A system on its own has a single source at its
/// origin; a child system started by events has one per event.
#[derive(Debug, Clone, Copy)]
struct Source {
    origin: Vec3,
    lasts: Lasts,
    /// Its particles that are alive.
    alive: u32,
    /// Particles it has made, which sequence initializers count by.
    made: u32,
    /// Seconds it has emitted for.
    age: f32,
}

impl Source {
    const FREE: Self = Self { origin: [0.0; 3], lasts: Lasts::Free, alive: 0, made: 0, age: 0.0 };

    fn emits(&self) -> bool {
        matches!(self.lasts, Lasts::Forever | Lasts::WithParticle(_) | Lasts::UntilSpent)
    }
}

/// Where one emitter of one source is in its schedule.
#[derive(Debug, Clone, Copy, Default)]
struct Emitting {
    elapsed: f32,
    /// The fraction of a particle carried over to the next step.
    pending: f32,
    burst_done: bool,
}

/// A child system and how the parent's particles start it.
struct Child {
    system: ParticleSystem,
    kind: ChildKind,
    origin: Vec3,
    probability: f32,
}

/// A live particle system.
pub struct ParticleSystem {
    pub(super) def: Rc<ParticleDef>,
    rng: Rng,
    /// The live particles, oldest first.
    pub(super) particles: Vec<Particle>,
    /// The most particles alive at once over all sources.
    capacity: usize,
    /// The most particles alive at once from one source.
    per_source: u32,
    sources: Vec<Source>,
    /// `sources.len() * def.emitters.len()` schedules, by source then emitter.
    emitting: Vec<Emitting>,
    next_id: u64,
    clock: f32,
    prewarmed: bool,
    /// The index of the first particle born in the last step.
    born_from: usize,
    /// Where particles died in the last step; filled only when a child wants it.
    died: Vec<Vec3>,
    records_deaths: bool,
    /// Rope trails: `trail_points` past positions per particle, newest first.
    pub(super) trail: Vec<Vec3>,
    pub(super) trail_points: usize,
    /// Seconds since the trails last recorded a point.
    pub(super) trail_clock: f32,
    /// Ropes over several sources: particle indices by source, then age. Empty when
    /// the particles are already in rope order.
    pub(super) order: Vec<u32>,
    /// What an emitter's reach and a particle's size are multiplied by (a child's scale).
    reach: Vec3,
    children: Vec<Child>,
}

impl ParticleSystem {
    /// A system playing `def`. The same seed gives the same particles.
    pub fn new(def: Rc<ParticleDef>, seed: u64) -> Self {
        let mut system = Self::with_sources(def, seed, 1, [1.0; 3]);
        system.sources[0].lasts = Lasts::Forever;
        system
    }

    /// A system with room for `sources` places to emit from, none of them in use.
    fn with_sources(def: Rc<ParticleDef>, seed: u64, sources: usize, reach: Vec3) -> Self {
        let per_source = def.max_count();
        let sources = sources.clamp(1, (MOST_CHILD_PARTICLES / per_source).max(1));
        let capacity = per_source * sources;
        let trail_points =
            if def.renderer() == Renderer::RopeTrail { def.renderer.segments as usize } else { 0 };
        Self {
            rng: Rng::new(seed),
            particles: Vec::with_capacity(capacity),
            capacity,
            per_source: per_source as u32,
            sources: vec![Source::FREE; sources],
            emitting: vec![Emitting::default(); sources * def.emitters.len()],
            next_id: 0,
            clock: 0.0,
            prewarmed: false,
            born_from: 0,
            died: Vec::new(),
            records_deaths: false,
            trail: vec![[0.0; 3]; capacity * trail_points],
            trail_points,
            trail_clock: 0.0,
            order: Vec::with_capacity(if def.renderer() == Renderer::Rope && sources > 1 {
                capacity
            } else {
                0
            }),
            reach,
            children: Vec::new(),
            def,
        }
    }

    /// Gives the system the child its document names at `index` of
    /// [`ParticleDef::children`]. The child inherits the instance factors of this
    /// system. Children are simulated by [`advance`](Self::advance) and drawn one
    /// by one through [`children`](Self::children).
    ///
    /// Returns the new child, for attaching its own children; `None` when the
    /// document has no child at `index`.
    pub fn attach_child(&mut self, index: usize, def: Rc<ParticleDef>) -> Option<&mut ParticleSystem> {
        let Some(child) = self.def.children().get(index) else {
            log::warn!("particles: no child {index} to attach to");
            return None;
        };
        let mut inherited = (*def).clone();
        inherited.instance = self.def.instance;
        let instances = match child.kind {
            ChildKind::Static => 1,
            _ if child.max_count > 0 => child.max_count.min(self.capacity),
            _ => self.capacity,
        };
        let mut system = Self::with_sources(
            Rc::new(inherited),
            u64::from(self.rng.next_u32()) << 32 | u64::from(self.rng.next_u32()),
            instances,
            child.scale,
        );
        // A child plays from the moment something starts it; simulating it ahead
        // would run it before its parent has any particles.
        system.prewarmed = true;
        if child.kind == ChildKind::Static {
            system.sources[0].lasts = Lasts::Forever;
            system.sources[0].origin = child.origin;
        }
        if child.kind == ChildKind::EventDeath && !self.records_deaths {
            self.records_deaths = true;
            self.died.reserve(self.capacity);
        }
        self.children.push(Child {
            system,
            kind: child.kind,
            origin: child.origin,
            probability: child.probability,
        });
        self.children.last_mut().map(|child| &mut child.system)
    }

    /// Applies a scene object's `instanceoverride` to the playing system and its
    /// children, as [`ParticleDef::apply_override`] does before one is made. The
    /// room for particles was set when the system was made and does not grow.
    pub fn apply_override(&mut self, instance_override: &Value) {
        Rc::make_mut(&mut self.def).apply_override(instance_override);
        let instance = self.def.instance;
        self.inherit(instance);
    }

    /// Hands the instance factors down to every child.
    fn inherit(&mut self, instance: Instance) {
        for child in &mut self.children {
            Rc::make_mut(&mut child.system.def).instance = instance;
            child.system.inherit(instance);
        }
    }

    /// The definition the system plays.
    pub fn def(&self) -> &Rc<ParticleDef> {
        &self.def
    }

    /// The attached child systems, each drawn with its own material.
    pub fn children(&self) -> impl Iterator<Item = &ParticleSystem> {
        self.children.iter().map(|child| &child.system)
    }

    /// How many particles are alive, not counting children.
    pub fn alive(&self) -> usize {
        self.particles.len()
    }

    /// The live particles, oldest first. Positions are as [`Particle::position`] says;
    /// [`position_of`](Self::position_of) gives where one is drawn.
    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    /// Where a particle of this system is, in the local space of the scene object.
    pub fn position_of(&self, particle: &Particle) -> Vec3 {
        add(self.anchor(particle), particle.position)
    }

    /// Simulates `dt` seconds: emits, ages and applies operators, here and in every
    /// child. The first call also simulates the document's start time ahead.
    pub fn advance(&mut self, dt: f32, env: &Environment) {
        let points = control_points(self.def.control_points(), env.pointer, Some(&env.control_points));
        if !self.prewarmed {
            self.prewarmed = true;
            let ahead = self.def.start_time();
            if ahead > 0.0 {
                let steps = (ahead / PREWARM_STEP).ceil().clamp(1.0, MOST_PREWARM_STEPS);
                for _ in 0..steps as usize {
                    self.step(ahead / steps, &points, env.pointer);
                }
            }
        }
        let dt = (dt * self.def.instance.rate).min(LONGEST_STEP);
        if dt > 0.0 {
            self.step(dt, &points, env.pointer);
        }
    }

    /// One step of the simulation.
    fn step(&mut self, dt: f32, points: &[ControlPoint; 8], pointer: Option<Vec3>) {
        self.clock += dt;
        self.expire(dt);
        self.emit(dt, points);
        self.operate(dt, points);
        self.record_trails(dt);
        self.order_rope();
        self.run_children(dt, pointer);
    }

    /// What a particle's position is relative to: nothing for particles that stay
    /// where they were born, their source for those that move with it.
    pub(super) fn anchor(&self, particle: &Particle) -> Vec3 {
        if self.def.world_space() {
            [0.0; 3]
        } else {
            self.sources.get(particle.source as usize).map_or([0.0; 3], |source| source.origin)
        }
    }

    /// Ages every particle and removes the dead, keeping the rest in birth order.
    fn expire(&mut self, dt: f32) {
        self.died.clear();
        let world_space = self.def.world_space();
        let stride = self.trail_points;
        let mut kept = 0;
        for index in 0..self.particles.len() {
            let particle = &mut self.particles[index];
            particle.age += dt;
            if particle.age >= particle.lifetime {
                let particle = *particle;
                if let Some(source) = self.sources.get_mut(particle.source as usize) {
                    source.alive = source.alive.saturating_sub(1);
                    if self.records_deaths && self.died.len() < self.died.capacity() {
                        self.died.push(if world_space {
                            particle.position
                        } else {
                            add(source.origin, particle.position)
                        });
                    }
                }
                continue;
            }
            if kept != index {
                self.particles[kept] = *particle;
                self.trail.copy_within(index * stride..(index + 1) * stride, kept * stride);
            }
            kept += 1;
        }
        self.particles.truncate(kept);
        for source in &mut self.sources {
            if source.lasts == Lasts::Ended && source.alive == 0 {
                *source = Source::FREE;
            }
        }
    }

    /// Lets every emitter of every source make the particles this step is due.
    fn emit(&mut self, dt: f32, points: &[ControlPoint; 8]) {
        self.born_from = self.particles.len();
        let def = Rc::clone(&self.def);
        let count = def.instance.count;
        let burst_only = def.emitters.iter().all(Emitter::is_burst_only);
        for source_index in 0..self.sources.len() {
            if !self.sources[source_index].emits() {
                continue;
            }
            for (emitter_index, emitter) in def.emitters.iter().enumerate() {
                let Some(schedule) = self.emitting.get_mut(source_index * def.emitters.len() + emitter_index)
                else {
                    continue;
                };
                schedule.elapsed += dt;
                if schedule.elapsed < emitter.delay {
                    continue;
                }
                let mut due = 0u32;
                if !schedule.burst_done {
                    schedule.burst_done = true;
                    if emitter.instantaneous > 0 && count > 0.0 {
                        due = ((emitter.instantaneous as f32 * count).round() as u32).max(1);
                    }
                }
                if emitter.duration <= 0.0 || schedule.elapsed - emitter.delay <= emitter.duration {
                    // Never more than fills the system: a document can ask for any rate.
                    schedule.pending =
                        (schedule.pending + emitter.rate * count * dt).min(self.capacity as f32 + 1.0);
                    let whole = schedule.pending.floor();
                    schedule.pending -= whole;
                    due = due.saturating_add(if emitter.one_per_frame {
                        whole.min(1.0) as u32
                    } else {
                        whole as u32
                    });
                }
                let source = &self.sources[source_index];
                let room = (self.per_source.saturating_sub(source.alive))
                    .min((self.capacity - self.particles.len()) as u32);
                for _ in 0..due.min(room) {
                    self.spawn(source_index, emitter, points, dt);
                }
            }
            let source = &mut self.sources[source_index];
            source.age += dt;
            if source.lasts == Lasts::UntilSpent && (burst_only || source.age > def.longest_life()) {
                source.lasts = Lasts::Ended;
            }
        }
    }

    /// Makes one particle at `source_index` from `emitter`, in a step of `dt` seconds.
    fn spawn(&mut self, source_index: usize, emitter: &Emitter, points: &[ControlPoint; 8], dt: f32) {
        let source = self.sources[source_index];
        let mut particle = Particle::newborn(self.rng.next_u32(), self.next_id, source_index as u32);
        self.next_id += 1;
        let (offset, velocity) = emitter.place(&mut self.rng);
        particle.position = add(points[emitter.control_point].position, mul(offset, self.reach));
        particle.velocity = velocity;
        let mut birth =
            Birth { rng: &mut self.rng, clock: self.clock, sequence: source.made, control_points: points };
        for initializer in &self.def.initializers {
            initializer.apply(&mut particle, &mut birth);
        }
        let instance = &self.def.instance;
        particle.lifetime = if particle.lifetime.is_finite() {
            (particle.lifetime * instance.lifetime).max(1e-3)
        } else {
            1.0
        };
        particle.size *= instance.size * (self.reach[0] + self.reach[1]) * 0.5;
        particle.velocity = scale(particle.velocity, instance.speed);
        if self.def.world_space() {
            particle.position = add(particle.position, source.origin);
        }
        // The operators move a newborn through the step it is born in, so it has
        // lived that long by the end of it.
        particle.age = dt;
        if self.trail_points > 0 {
            particle.trail = 1;
            if let Some(first) = self.trail.get_mut(self.particles.len() * self.trail_points) {
                *first = particle.position;
            }
        }
        if self.particles.len() < self.capacity {
            self.particles.push(particle);
            self.sources[source_index].alive += 1;
            self.sources[source_index].made = source.made.wrapping_add(1);
        }
    }

    /// Runs the operators and derives what is drawn this frame.
    fn operate(&mut self, dt: f32, points: &[ControlPoint; 8]) {
        for particle in &mut self.particles {
            particle.now_color = particle.color;
            particle.now_alpha = particle.alpha;
            particle.now_size = particle.size;
        }
        let frame = Frame { dt, clock: self.clock, control_points: points, force: self.def.instance.speed };
        for (index, operator) in self.def.operators.iter().enumerate() {
            operator.apply(index as u32 + 1, &mut self.particles, &frame);
        }
        for particle in &mut self.particles {
            // A document can drive a particle to infinity (runaway drag, a force at a
            // singular point). Such a particle is put out instead of poisoning the batch.
            let sum =
                particle.position.iter().chain(&particle.velocity).chain(&particle.rotation).sum::<f32>()
                    + particle.now_size
                    + particle.now_alpha
                    + particle.now_color.iter().sum::<f32>();
            if !sum.is_finite() {
                let (seed, id, source, trail) = (particle.seed, particle.id, particle.source, particle.trail);
                *particle = Particle {
                    age: 1.0,
                    lifetime: 0.0,
                    now_alpha: 0.0,
                    now_size: 0.0,
                    trail,
                    ..Particle::newborn(seed, id, source)
                };
            }
        }
    }

    /// Rope trails: every so often each particle remembers where it is.
    fn record_trails(&mut self, dt: f32) {
        let stride = self.trail_points;
        if stride == 0 {
            return;
        }
        let interval = self.def.renderer.length / stride as f32;
        self.trail_clock += dt;
        if self.trail_clock < interval {
            return;
        }
        self.trail_clock %= interval;
        for (particle, points) in self.particles.iter_mut().zip(self.trail.chunks_exact_mut(stride)) {
            points.copy_within(0..stride - 1, 1);
            points[0] = particle.position;
            particle.trail = (particle.trail + 1).min(stride as u32);
        }
    }

    /// Ropes over several sources: one rope per source needs the particles by source.
    fn order_rope(&mut self) {
        if self.def.renderer() != Renderer::Rope || self.sources.len() < 2 {
            return;
        }
        self.order.clear();
        self.order.extend(0..self.particles.len() as u32);
        let particles = &self.particles;
        self.order
            .sort_unstable_by_key(|&index| (particles[index as usize].source, particles[index as usize].id));
    }

    /// Starts child instances for this step's births and deaths, moves the ones that
    /// follow a particle, and steps every child.
    fn run_children(&mut self, dt: f32, pointer: Option<Vec3>) {
        let Self { children, particles, sources, died, def, born_from, rng, .. } = self;
        let world_space = def.world_space();
        let place = |particle: &Particle| {
            if world_space {
                particle.position
            } else {
                add(sources.get(particle.source as usize).map_or([0.0; 3], |s| s.origin), particle.position)
            }
        };
        for child in children.iter_mut() {
            let mut lucky = || child.probability >= 1.0 || rng.unit() < child.probability;
            match child.kind {
                ChildKind::Static => {
                    child.system.sources[0].origin = add(sources[0].origin, child.origin);
                }
                ChildKind::EventSpawn => {
                    for particle in &particles[(*born_from).min(particles.len())..] {
                        if lucky() {
                            child.system.open(add(place(particle), child.origin), Lasts::UntilSpent);
                        }
                    }
                }
                ChildKind::EventDeath => {
                    for at in died.iter() {
                        if lucky() {
                            child.system.open(add(*at, child.origin), Lasts::UntilSpent);
                        }
                    }
                }
                ChildKind::EventFollow => {
                    for particle in &particles[(*born_from).min(particles.len())..] {
                        if lucky() {
                            child
                                .system
                                .open(add(place(particle), child.origin), Lasts::WithParticle(particle.id));
                        }
                    }
                    for source in &mut child.system.sources {
                        let Lasts::WithParticle(id) = source.lasts else { continue };
                        match particles.binary_search_by_key(&id, |particle| particle.id) {
                            Ok(index) => source.origin = add(place(&particles[index]), child.origin),
                            Err(_) => source.lasts = Lasts::Ended,
                        }
                    }
                }
            }
            let points = control_points(child.system.def.control_points(), pointer, None);
            let dt = (dt * child.system.def.instance.rate / def.instance.rate.max(1e-6)).min(LONGEST_STEP);
            child.system.step(dt, &points, pointer);
        }
    }

    /// Starts emitting at `origin` from a free source, if there is one.
    fn open(&mut self, origin: Vec3, lasts: Lasts) {
        let Some(index) = self.sources.iter().position(|source| source.lasts == Lasts::Free) else { return };
        self.sources[index] = Source { origin, lasts, ..Source::FREE };
        let emitters = self.def.emitters.len();
        for schedule in self.emitting.iter_mut().skip(index * emitters).take(emitters) {
            *schedule = Emitting::default();
        }
    }
}

/// Where a system's control points are this frame.
///
/// `placed` holds the positions the scene keeps for a top-level system; a child
/// has none and rests on its document's offsets. A point locked to the pointer
/// sits on it, and rests inactive while there is no pointer.
fn control_points(
    defs: &[ControlPointDef; 8],
    pointer: Option<Vec3>,
    placed: Option<&[Vec3; 8]>,
) -> [ControlPoint; 8] {
    let mut points = [ControlPoint::default(); 8];
    for (index, (point, def)) in points.iter_mut().zip(defs).enumerate() {
        let rest = placed.map_or(def.offset, |placed| placed[index]);
        let rest = if rest.iter().all(|v| v.is_finite()) { rest } else { def.offset };
        *point = match (def.follows_pointer, pointer) {
            (true, Some(pointer)) if pointer.iter().all(|v| v.is_finite()) => {
                ControlPoint { position: add(pointer, def.offset), active: true }
            }
            (true, _) => ControlPoint { position: rest, active: false },
            (false, _) => ControlPoint { position: rest, active: true },
        };
    }
    points
}
