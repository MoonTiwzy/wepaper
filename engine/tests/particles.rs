//! Particle systems: what the simulation does to particles and what it hands the renderer.
//!
//! The corpus check at the end runs against a real library when these are set and
//! is skipped otherwise:
//!
//! - `WEPAPER_TEST_ASSETS`: Wallpaper Engine's `assets` folder
//! - `WEPAPER_TEST_WORKSHOP`: a folder of wallpapers (`steamapps/workshop/content/431960`)

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use wepaper_engine::particles::{
    AnimationMode, Batch, ChildKind, Environment, Particle, ParticleDef, ParticleSystem, Renderer, View,
};
use wepaper_engine::read::{Archive, Files};
use wepaper_json::Value;

const FRAME: f32 = 1.0 / 60.0;

fn def(json: &str) -> ParticleDef {
    ParticleDef::parse(&wepaper_json::parse(json).expect("test document parses"))
        .expect("test document is a particle system")
}

fn system(json: &str, seed: u64) -> ParticleSystem {
    ParticleSystem::new(Rc::new(def(json)), seed)
}

/// Advances `frames` frames of a sixtieth of a second in the system's own surroundings.
fn run(system: &mut ParticleSystem, frames: usize) {
    let env = Environment::of(system.def());
    for _ in 0..frames {
        system.advance(FRAME, &env);
    }
}

/// A system that makes exactly one particle at its origin and applies `operators` to it.
fn lone(initializers: &str, operators: &str) -> ParticleSystem {
    system(
        &format!(
            r#"{{"material":"m.json","maxcount":4,
                 "emitter":[{{"name":"sphererandom","rate":0,"instantaneous":1,"distancemax":0}}],
                 "initializer":[{{"name":"lifetimerandom","min":2,"max":2}},{{"name":"sizerandom","min":10,"max":10}}{initializers}],
                 "operator":[{operators}]}}"#
        ),
        1,
    )
}

fn only(system: &ParticleSystem) -> Particle {
    assert_eq!(system.alive(), 1);
    system.particles()[0]
}

fn close(a: f32, b: f32, slack: f32) -> bool {
    (a - b).abs() <= slack
}

fn vertices(system: &ParticleSystem) -> (Batch, Vec<f32>) {
    let mut out = Vec::new();
    let batch = system.write_vertices(&View::default(), &mut out);
    assert_eq!(out.len(), batch.vertices * batch.stride());
    (batch, out)
}

#[test]
fn emitters_make_their_rate_of_particles_per_second() {
    let mut steady = system(
        r#"{"material":"m.json","maxcount":500,"emitter":[{"name":"sphererandom","rate":20}],
            "initializer":[{"name":"lifetimerandom","min":100,"max":100}]}"#,
        1,
    );
    for second in 1..=5 {
        run(&mut steady, 60);
        assert!(steady.alive().abs_diff(second * 20) <= 1, "{} alive after {second} s", steady.alive());
    }

    let mut two = system(
        r#"{"material":"m.json","maxcount":500,
            "emitter":[{"name":"boxrandom","rate":7.5},{"name":"sphererandom","rate":2.5,"instantaneous":3}],
            "initializer":[{"name":"lifetimerandom","min":100,"max":100}]}"#,
        1,
    );
    run(&mut two, 240);
    assert!(two.alive().abs_diff(43) <= 2, "{} alive from two emitters and a burst", two.alive());
}

#[test]
fn the_rate_does_not_depend_on_the_frame_rate() {
    let document = r#"{"material":"m.json","maxcount":900,"emitter":[{"name":"sphererandom","rate":90}],
                       "initializer":[{"name":"lifetimerandom","min":100,"max":100}]}"#;
    for frames_per_second in [24usize, 60, 144] {
        let mut system = system(document, 1);
        let env = Environment::default();
        for _ in 0..frames_per_second * 2 {
            system.advance(1.0 / frames_per_second as f32, &env);
        }
        assert!(system.alive().abs_diff(180) <= 2, "{} alive at {frames_per_second} fps", system.alive());
    }
}

#[test]
fn an_emitter_can_wait_stop_and_be_held_to_one_particle_a_frame() {
    let lasting = r#""initializer":[{"name":"lifetimerandom","min":100,"max":100}]"#;
    let mut timed = system(
        &format!(
            r#"{{"material":"m.json","maxcount":900,"emitter":[{{"name":"sphererandom","rate":60,"delay":1,"duration":0.5}}],{lasting}}}"#
        ),
        1,
    );
    run(&mut timed, 55);
    assert_eq!(timed.alive(), 0, "nothing before the delay is over");
    run(&mut timed, 300);
    assert!(timed.alive().abs_diff(30) <= 2, "{} from half a second of emission", timed.alive());

    let mut held = system(
        &format!(
            r#"{{"material":"m.json","maxcount":900,"emitter":[{{"name":"sphererandom","rate":600,"flags":2}}],{lasting}}}"#
        ),
        1,
    );
    run(&mut held, 30);
    assert_eq!(held.alive(), 30);
}

#[test]
fn particles_die_when_their_life_is_over() {
    let mut burst = system(
        r#"{"material":"m.json","maxcount":50,"emitter":[{"name":"sphererandom","rate":0,"instantaneous":10}],
            "initializer":[{"name":"lifetimerandom","min":1,"max":2}]}"#,
        1,
    );
    run(&mut burst, 57);
    assert_eq!(burst.alive(), 10, "all alive before the shortest life is over");
    assert!(
        burst
            .particles()
            .iter()
            .all(|p| close(p.age, 57.0 * FRAME, 1e-3) && (1.0..=2.0).contains(&p.lifetime))
    );
    run(&mut burst, 33);
    assert!(
        burst.alive() > 0 && burst.alive() < 10,
        "{} alive between the shortest and the longest life",
        burst.alive()
    );
    run(&mut burst, 40);
    assert_eq!(burst.alive(), 0, "none alive after the longest life");
    let (batch, out) = vertices(&burst);
    assert_eq!((batch.vertices, out.len()), (0, 0));
}

#[test]
fn survivors_stay_in_birth_order_when_others_die() {
    let mut mixed = system(
        r#"{"material":"m.json","maxcount":400,"emitter":[{"name":"sphererandom","rate":120}],
            "initializer":[{"name":"lifetimerandom","min":0.1,"max":2}]}"#,
        5,
    );
    run(&mut mixed, 300);
    assert!(mixed.alive() > 50);
    // Younger particles were born later, so in birth order ages never grow.
    assert!(mixed.particles().windows(2).all(|pair| pair[0].age >= pair[1].age));
}

#[test]
fn the_system_never_holds_more_than_its_max_count() {
    let mut crowded = system(
        r#"{"material":"m.json","maxcount":50,"emitter":[{"name":"sphererandom","rate":1000,"instantaneous":500}],
            "initializer":[{"name":"lifetimerandom","min":0.5,"max":0.5}]}"#,
        1,
    );
    let env = Environment::default();
    for _ in 0..200 {
        crowded.advance(FRAME, &env);
        assert!(crowded.alive() <= 50);
    }
    assert_eq!(crowded.alive(), 50, "the room freed by the dead is refilled");
    assert_eq!(crowded.def().max_count(), 50);
}

#[test]
fn movement_applies_velocity_gravity_and_drag() {
    let moving = r#",{"name":"velocityrandom","min":"30 0 0","max":"30 0 0"}"#;
    let mut still = lone(moving, "");
    run(&mut still, 60);
    assert_eq!(only(&still).position, [0.0; 3], "without a movement operator nothing moves");

    let mut coasting = lone(moving, r#"{"name":"movement"}"#);
    run(&mut coasting, 60);
    assert!(close(only(&coasting).position[0], 30.0, 0.01), "{:?}", only(&coasting).position);

    let mut falling = lone(moving, r#"{"name":"movement","gravity":"0 -100 0"}"#);
    run(&mut falling, 60);
    let fallen = only(&falling);
    assert!(
        close(fallen.velocity[1], -100.0, 0.01)
            && close(fallen.position[1], -50.0, 1.0)
            && close(fallen.position[0], 30.0, 0.01),
        "{fallen:?}"
    );

    let mut dragged = lone(moving, r#"{"name":"movement","drag":1}"#);
    run(&mut dragged, 60);
    let slowed = only(&dragged);
    assert!(close(slowed.velocity[0], 30.0 * (-1f32).exp(), 0.05), "{:?}", slowed.velocity);
    assert!(close(slowed.position[0], 30.0 * (1.0 - (-1f32).exp()), 0.3), "{:?}", slowed.position);
}

#[test]
fn angular_movement_turns_the_particle() {
    let spinning = r#",{"name":"angularvelocityrandom","min":"0 0 2","max":"0 0 2"},{"name":"rotationrandom","min":"0 0 1","max":"0 0 1"}"#;
    let mut fixed = lone(spinning, "");
    run(&mut fixed, 60);
    assert_eq!(only(&fixed).rotation, [0.0, 0.0, 1.0]);
    let mut turning = lone(spinning, r#"{"name":"angularmovement"}"#);
    run(&mut turning, 60);
    assert!(close(only(&turning).rotation[2], 3.0, 0.01));
    let mut pushed = lone(spinning, r#"{"name":"angularmovement","force":"0 0 -2"}"#);
    run(&mut pushed, 60);
    assert!(close(only(&pushed).angular_velocity[2], 0.0, 0.01));
}

#[test]
fn alpha_fades_in_and_out_over_the_life() {
    let mut fading = lone("", r#"{"name":"alphafade","fadeintime":0.25,"fadeouttime":0.75}"#);
    run(&mut fading, 15);
    assert!(close(only(&fading).now_alpha, 0.5, 0.01), "halfway through the fade in");
    run(&mut fading, 45);
    assert_eq!(only(&fading).now_alpha, 1.0);
    run(&mut fading, 45);
    assert!(close(only(&fading).now_alpha, 0.5, 0.01), "halfway through the fade out");
    assert_eq!(only(&fading).alpha, 1.0, "the opacity it was born with is kept");
    let (_, out) = vertices(&fading);
    assert!(close(out[10], 0.5, 0.01), "the faded opacity is what is drawn");
}

#[test]
fn size_alpha_and_colour_change_over_the_life() {
    let mut changing = lone(
        "",
        r#"{"name":"sizechange","startvalue":1,"endvalue":3},
           {"name":"alphachange","starttime":0.5,"endtime":1,"startvalue":1,"endvalue":0},
           {"name":"colorchange","startvalue":"1 1 1","endvalue":"0 0.5 1","endtime":0.5}"#,
    );
    run(&mut changing, 30);
    let early = only(&changing);
    assert!(
        close(early.now_size, 15.0, 0.01) && early.now_alpha == 1.0 && close(early.now_color[0], 0.5, 0.01),
        "{early:?}"
    );
    run(&mut changing, 60);
    let late = only(&changing);
    assert!(close(late.now_size, 25.0, 0.01) && close(late.now_alpha, 0.5, 0.01), "{late:?}");
    assert!(
        close(late.now_color[0], 0.0, 1e-4)
            && close(late.now_color[1], 0.5, 1e-4)
            && close(late.now_color[2], 1.0, 1e-4)
    );
    assert_eq!((late.size, late.color), (10.0, [1.0; 3]));
}

#[test]
fn oscillators_swing_size_alpha_and_position() {
    let mut swinging = lone(
        "",
        r#"{"name":"oscillatesize","frequencymin":8,"frequencymax":8,"scalemin":0.5,"scalemax":1.5},
           {"name":"oscillatealpha","frequencymin":8,"frequencymax":8,"scalemin":0.2,"scalemax":0.6},
           {"name":"oscillateposition","frequencymin":4,"frequencymax":4,"scalemin":30,"scalemax":30,"mask":"1 0 0"}"#,
    );
    let (mut sizes, mut alphas, mut reach) =
        ((f32::MAX, f32::MIN), (f32::MAX, f32::MIN), (f32::MAX, f32::MIN));
    let env = Environment::default();
    for _ in 0..110 {
        swinging.advance(FRAME, &env);
        let particle = only(&swinging);
        sizes = (sizes.0.min(particle.now_size), sizes.1.max(particle.now_size));
        alphas = (alphas.0.min(particle.now_alpha), alphas.1.max(particle.now_alpha));
        reach = (reach.0.min(particle.position[0]), reach.1.max(particle.position[0]));
        assert_eq!((particle.position[1], particle.position[2]), (0.0, 0.0), "the mask keeps the sway on x");
    }
    assert!(close(sizes.0, 5.0, 0.3) && close(sizes.1, 15.0, 0.3), "{sizes:?}");
    assert!(close(alphas.0, 0.2, 0.02) && close(alphas.1, 0.6, 0.02), "{alphas:?}");
    assert!(close(reach.1 - reach.0, 60.0, 2.0), "the sway spans twice its scale: {reach:?}");
}

#[test]
fn turbulence_pushes_particles_apart_along_the_mask() {
    let mut gusty = system(
        r#"{"material":"m.json","maxcount":64,"emitter":[{"name":"boxrandom","rate":0,"instantaneous":64,"distancemax":"300 300 0"}],
            "initializer":[{"name":"lifetimerandom","min":9,"max":9}],
            "operator":[{"name":"movement"},{"name":"turbulence","scale":0.01,"speedmin":200,"speedmax":400,"mask":"1 0 0"}]}"#,
        2,
    );
    run(&mut gusty, 120);
    let particles = gusty.particles();
    assert!(
        particles.iter().all(|p| p.velocity[1] == 0.0 && p.velocity[2] == 0.0),
        "the mask lets only x through"
    );
    assert!(particles.iter().all(|p| p.velocity[0].abs() <= 800.0), "two seconds of at most 400 a second");
    let (left, right) = (
        particles.iter().filter(|p| p.velocity[0] < -5.0).count(),
        particles.iter().filter(|p| p.velocity[0] > 5.0).count(),
    );
    assert!(left > 5 && right > 5, "the field pushes both ways: {left} left, {right} right");
}

#[test]
fn a_vortex_carries_particles_around_its_centre() {
    let mut whirl = system(
        r#"{"material":"m.json","maxcount":1,"emitter":[{"name":"boxrandom","rate":0,"instantaneous":1,"distancemin":"100 0 0","distancemax":"100 0 0","directions":"1 0 0"}],
            "initializer":[{"name":"lifetimerandom","min":60,"max":60}],
            "operator":[{"name":"movement","drag":2},{"name":"vortex","distanceinner":0,"distanceouter":0,"speedouter":400,"offset":"0 0 0"}]}"#,
        3,
    );
    run(&mut whirl, 1);
    let start = only(&whirl).position;
    assert!(close(start[0].abs(), 100.0, 1.0) && close(start[1], 0.0, 1.0));
    run(&mut whirl, 120);
    let later = only(&whirl).position;
    let turn = start[0] * later[1] - start[1] * later[0];
    assert!(turn > 1000.0, "it went counter-clockwise around the axis: from {start:?} to {later:?}");
}

#[test]
fn control_points_pull_push_and_follow_the_pointer() {
    let document = |strength: i32, flags: u32| {
        format!(
            r#"{{"material":"m.json","maxcount":1,"controlpoint":[{{"id":1,"flags":{flags},"offset":"50 0 0"}}],
                 "emitter":[{{"name":"sphererandom","rate":0,"instantaneous":1,"distancemax":0}}],
                 "initializer":[{{"name":"lifetimerandom","min":60,"max":60}}],
                 "operator":[{{"name":"movement"}},{{"name":"controlpointattract","controlpoint":1,"scale":{strength},"threshold":200}}]}}"#
        )
    };
    let mut pulled = system(&document(100, 0), 1);
    run(&mut pulled, 30);
    assert!(
        only(&pulled).velocity[0] > 40.0 && only(&pulled).position[0] > 0.0,
        "pulled towards the point at +x"
    );

    let mut pushed = system(&document(-100, 0), 1);
    run(&mut pushed, 30);
    assert!(only(&pushed).velocity[0] < -40.0);

    let mut moved = system(&document(100, 0), 1);
    let mut env = Environment::of(moved.def());
    assert_eq!(env.control_points[1], [50.0, 0.0, 0.0]);
    env.control_points[1] = [0.0, -50.0, 0.0];
    for _ in 0..30 {
        moved.advance(FRAME, &env);
    }
    assert!(
        only(&moved).velocity[1] < -40.0 && only(&moved).velocity[0] == 0.0,
        "the scene moved the point below"
    );

    let mut pointed = system(&document(100, 1), 1);
    let mut env = Environment::of(pointed.def());
    for _ in 0..30 {
        pointed.advance(FRAME, &env);
    }
    assert_eq!(only(&pointed).velocity, [0.0; 3], "no pointer: the force rests");
    env.pointer = Some([-150.0, 0.0, 0.0]);
    for _ in 0..30 {
        pointed.advance(FRAME, &env);
    }
    assert!(only(&pointed).velocity[0] < -40.0, "pulled to the pointer plus the point's offset, at -100");
}

#[test]
fn an_emitter_follows_its_control_point() {
    let mut trailing = system(
        r#"{"material":"m.json","maxcount":100,"controlpoint":[{"id":0,"flags":1}],
            "emitter":[{"name":"sphererandom","rate":60,"distancemax":0}],
            "initializer":[{"name":"lifetimerandom","min":60,"max":60}]}"#,
        1,
    );
    let mut env = Environment::of(trailing.def());
    for frame in 0..20 {
        env.pointer = Some([frame as f32 * 10.0, 5.0, 0.0]);
        trailing.advance(FRAME, &env);
    }
    let particles = trailing.particles();
    assert_eq!(particles.len(), 20);
    assert!(particles.iter().enumerate().all(|(frame, p)| p.position == [frame as f32 * 10.0, 5.0, 0.0]));
}

#[test]
fn instance_overrides_scale_what_they_name() {
    let document = r#"{"material":"m.json","maxcount":100,"emitter":[{"name":"sphererandom","rate":20,"distancemax":0}],
        "initializer":[{"name":"lifetimerandom","min":4,"max":4},{"name":"sizerandom","min":10,"max":10},
                       {"name":"velocityrandom","min":"10 0 0","max":"10 0 0"},{"name":"colorrandom","min":"255 255 255"},
                       {"name":"alpharandom","min":0.8,"max":0.8}],
        "operator":[{"name":"movement","gravity":"0 -10 0"}]}"#;
    let mut plain = system(document, 1);
    run(&mut plain, 60);

    let mut scaled = def(document);
    scaled.apply_override(
        &wepaper_json::parse(
            r#"{"alpha":0.5,"colorn":"1 0.5 0.25","count":2,"lifetime":0.5,"size":3,"speed":2,"id":7}"#,
        )
        .unwrap(),
    );
    assert_eq!(scaled.max_count(), 200);
    let mut scaled = ParticleSystem::new(Rc::new(scaled), 1);
    run(&mut scaled, 60);

    assert!(
        plain.alive().abs_diff(20) <= 1 && scaled.alive().abs_diff(40) <= 1,
        "count doubles the emission"
    );
    let (was, is) = (plain.particles()[0], scaled.particles()[0]);
    assert_eq!((was.lifetime, is.lifetime), (4.0, 2.0));
    assert_eq!((was.size, is.size), (10.0, 30.0));
    assert!(close(is.velocity[0], 2.0 * was.velocity[0], 1e-3), "speed doubles the initial velocity");
    // The doubled emission bore its first particle a frame earlier, hence the slack.
    assert!(
        close(is.velocity[1], 2.0 * was.velocity[1], 0.5),
        "and the forces: {} against {}",
        is.velocity[1],
        was.velocity[1]
    );
    let (_, out) = vertices(&scaled);
    assert!(
        close(out[7], 1.0, 1e-5)
            && close(out[8], 0.5, 1e-5)
            && close(out[9], 0.25, 1e-5)
            && close(out[10], 0.4, 1e-5),
        "tint and opacity are laid over what is drawn"
    );
    assert_eq!(out[6], 15.0, "a size of 30 is drawn 15 across");

    let mut slowed = def(document);
    slowed.apply_override(&wepaper_json::parse(r#"{"rate":{"user":"speed","value":0.5}}"#).unwrap());
    let mut slowed = ParticleSystem::new(Rc::new(slowed), 1);
    run(&mut slowed, 60);
    assert!(slowed.alive().abs_diff(10) <= 1, "rate slows time itself");
    assert!(
        close(slowed.particles()[0].age, 0.46, 0.03),
        "the first was born a twentieth of a second in, half a second ago in its own time"
    );
}

#[test]
fn start_time_fills_the_system_before_the_first_frame() {
    let document = |start: u32| {
        format!(
            r#"{{"material":"m.json","maxcount":500,"starttime":{start},"emitter":[{{"name":"sphererandom","rate":10}}],
                 "initializer":[{{"name":"lifetimerandom","min":3,"max":3}}],"operator":[{{"name":"movement","gravity":"0 -10 0"}}]}}"#
        )
    };
    let mut cold = system(&document(0), 1);
    run(&mut cold, 1);
    assert!(cold.alive() <= 1);
    let mut warm = system(&document(15), 1);
    assert_eq!(warm.def().start_time(), 15.0);
    run(&mut warm, 1);
    assert!(warm.alive().abs_diff(30) <= 2, "{} alive: a steady three seconds' worth", warm.alive());
    assert!(
        warm.particles()[0].age > 2.5 && warm.particles()[0].position[1] < -20.0,
        "they have lived and moved"
    );
}

#[test]
fn a_seed_repeats_the_same_system() {
    let document = r#"{"material":"m.json","maxcount":300,"emitter":[{"name":"sphererandom","rate":100}],
        "initializer":[{"name":"lifetimerandom","min":0.5,"max":1.5},{"name":"sizerandom","min":5,"max":50},{"name":"velocityrandom"},
                       {"name":"rotationrandom"},{"name":"colorrandom","min":"0 0 0"},{"name":"turbulentvelocityrandom"}],
        "operator":[{"name":"movement","drag":0.5},{"name":"alphafade"},{"name":"turbulence"},{"name":"oscillateposition"},{"name":"oscillatealpha"}]}"#;
    let frame = |seed: u64| {
        let mut system = system(document, seed);
        run(&mut system, 200);
        vertices(&system).1
    };
    let (first, again, other) = (frame(9), frame(9), frame(10));
    assert!(first.len() > 6 * 17 * 50);
    assert_eq!(first, again);
    assert_ne!(first, other);
}

#[test]
fn sprites_are_six_vertices_with_the_thick_layout() {
    let mut sprites = system(
        r#"{"material":"materials/particle/halo.json","maxcount":40,"animationmode":"once","sequencemultiplier":2,
            "emitter":[{"name":"sphererandom","rate":0,"instantaneous":25,"distancemin":10,"distancemax":90}],
            "initializer":[{"name":"lifetimerandom","min":4,"max":4},{"name":"sizerandom","min":8,"max":8},
                           {"name":"rotationrandom","min":"0.1 0.2 0.3","max":"0.1 0.2 0.3"},{"name":"velocityrandom","min":"1 2 3","max":"1 2 3"},
                           {"name":"colorrandom","min":"255 0 0","max":"255 0 0"}],
            "renderer":[{"name":"sprite"}]}"#,
        1,
    );
    run(&mut sprites, 30);
    let (batch, out) = vertices(&sprites);
    assert_eq!((batch.renderer, batch.shader, batch.vertices), (Renderer::Sprite, "genericparticle", 25 * 6));
    assert_eq!(batch.layout, &[(0, 3), (3, 4), (2, 4), (4, 4), (6, 2)]);
    assert_eq!(batch.combos, &[("GS_ENABLED", 0), ("THICKFORMAT", 1)]);
    assert_eq!((batch.stride(), batch.render_var0), (17, [0.0; 4]));
    assert_eq!(
        (sprites.def().material(), sprites.def().animation()),
        ("materials/particle/halo.json", AnimationMode::Once)
    );

    let quad: Vec<&[f32; 17]> = out.as_chunks::<17>().0.iter().take(6).collect();
    let centre = sprites.position_of(&sprites.particles()[0]);
    let corners: Vec<(f32, f32)> = quad.iter().map(|v| (v[3], v[4])).collect();
    assert_eq!(
        corners,
        [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
        "two triangles covering the quad"
    );
    for vertex in quad {
        assert_eq!(&vertex[..3], &centre, "every corner carries the centre; the shader spreads them");
        assert_eq!(
            (vertex[5], vertex[6]),
            (0.3, 4.0),
            "rotation z, and half the size: how far across it is drawn"
        );
        assert_eq!(&vertex[7..11], &[1.0, 0.0, 0.0, 1.0], "colour");
        assert_eq!(&vertex[11..14], &[1.0, 2.0, 3.0], "velocity");
        assert!(close(vertex[14], 0.25, 1e-3), "an eighth of the life, played twice over it");
        assert_eq!(&vertex[15..], &[0.1, 0.2], "rotation x and y");
    }
}

#[test]
fn sprite_sheet_modes_pick_the_frame() {
    let document = |mode: &str| {
        format!(
            r#"{{"material":"m.json","maxcount":64,{mode}"emitter":[{{"name":"sphererandom","rate":0,"instantaneous":64}}],
                 "initializer":[{{"name":"lifetimerandom","min":2,"max":2}}]}}"#
        )
    };
    let sheet_positions = |mode: &str, frames: usize| {
        let mut system = system(&document(mode), 4);
        run(&mut system, frames);
        vertices(&system).1.as_chunks::<{ 17 * 6 }>().0.iter().map(|quad| quad[14]).collect::<Vec<f32>>()
    };
    assert!(
        sheet_positions("", 60).iter().all(|&at| close(at, 0.5, 1e-3)),
        "sequence is the default and follows the life"
    );
    assert!(
        sheet_positions(r#""sequencemultiplier":3,"#, 60).iter().all(|&at| close(at, 0.5, 1e-3)),
        "three loops: halfway through the second"
    );
    assert!(
        sheet_positions(r#""animationmode":"once","sequencemultiplier":3,"#, 60)
            .iter()
            .all(|&at| at > 0.99 && at < 1.0),
        "once holds the last frame"
    );
    let (early, late) = (
        sheet_positions(r#""animationmode":"randomframe","#, 10),
        sheet_positions(r#""animationmode":"randomframe","#, 100),
    );
    assert_eq!(early, late, "a random frame is kept for life");
    assert!(early.iter().all(|at| (0.0..1.0).contains(at)));
    let mut distinct = early.clone();
    distinct.sort_by(f32::total_cmp);
    distinct.dedup();
    assert!(distinct.len() > 50, "and differs between particles");
    assert!(def(&document("")).blends_frames() && !def(&document(r#""flags":2,"#)).blends_frames());
    assert!(!def(&document(r#""animationmode":"randomframe","#)).blends_frames());
}

#[test]
fn sprite_trails_carry_a_velocity_the_shader_can_stretch_along() {
    let mut trails = system(
        r#"{"material":"m.json","maxcount":10,"emitter":[{"name":"sphererandom","rate":0,"instantaneous":10}],
            "initializer":[{"name":"lifetimerandom","min":4,"max":4}],
            "renderer":[{"name":"spritetrail","length":0.02,"maxlength":5,"minlength":0.5}]}"#,
        1,
    );
    run(&mut trails, 2);
    let (batch, out) = vertices(&trails);
    assert_eq!(
        (batch.renderer, batch.shader, batch.vertices),
        (Renderer::SpriteTrail, "genericparticle", 60)
    );
    assert_eq!(batch.combos, &[("GS_ENABLED", 0), ("THICKFORMAT", 1), ("TRAILRENDERER", 1)]);
    assert_eq!(batch.render_var0, [0.02, 5.0, 0.5, 0.0]);
    assert_eq!(batch.stride(), 17);
    for vertex in out.as_chunks::<17>().0 {
        let speed = (vertex[11] * vertex[11] + vertex[12] * vertex[12] + vertex[13] * vertex[13]).sqrt();
        assert!(speed > 0.0, "a particle at rest still has a direction to face");
    }
}

/// What the stock rope vertex shader computes for one ribbon vertex of the thick layout.
fn rope_shader_position(vertex: &[f32], forward: [f32; 3]) -> [f32; 3] {
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
    };
    let unit = |a: [f32; 3]| {
        let length = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
        assert!(length > 1e-6, "the shader would divide by zero");
        a.map(|v| v / length)
    };
    let (start, end) = ([vertex[0], vertex[1], vertex[2]], [vertex[4], vertex[5], vertex[6]]);
    let (size_start, size_end) = (vertex[3], vertex[15]);
    let (across, along) = (vertex[20], vertex[21]);
    let right_start = unit(cross(forward, [0, 1, 2].map(|i| end[i] - start[i] + start[i] - vertex[8 + i])))
        .map(|v| v * size_start);
    let right_end = unit(cross(forward, [0, 1, 2].map(|i| end[i] - start[i] - (end[i] - vertex[12 + i]))))
        .map(|v| v * size_end);
    [0, 1, 2].map(|i| {
        start[i]
            + (end[i] - start[i]) * along
            + (right_start[i] + (right_end[i] - right_start[i]) * along) * across * 2.0
            - 1.0
    })
}

#[test]
fn a_rope_is_one_ribbon_through_the_particles_in_birth_order() {
    let document = |subdivision: u32| {
        format!(
            r#"{{"material":"m.json","maxcount":64,"controlpoint":[{{"id":0,"flags":1}}],
                 "emitter":[{{"name":"sphererandom","rate":60,"distancemax":0,"flags":2}}],
                 "initializer":[{{"name":"lifetimerandom","min":60,"max":60}},{{"name":"sizerandom","min":6,"max":6}}],
                 "renderer":[{{"name":"rope","subdivision":{subdivision}}}]}}"#
        )
    };
    let draw = |subdivision: u32| {
        let mut rope = system(&document(subdivision), 1);
        let mut env = Environment::of(rope.def());
        for frame in 0..9 {
            env.pointer = Some([frame as f32 * 20.0, (frame as f32 * 0.9).sin() * 40.0, 0.0]);
            rope.advance(FRAME, &env);
        }
        assert_eq!(rope.alive(), 9);
        (vertices(&rope), rope)
    };

    let ((batch, out), rope) = draw(1);
    assert_eq!(
        (batch.renderer, batch.shader, batch.vertices),
        (Renderer::Rope, "genericropeparticle", 8 * 6)
    );
    assert_eq!(batch.layout, &[(12, 4), (3, 4), (4, 4), (5, 4), (13, 4), (14, 2), (2, 4)]);
    assert_eq!((batch.stride(), batch.combos), (26, &[("GS_ENABLED", 0), ("THICKFORMAT", 1)][..]));
    let forward = View::default().forward;
    for (piece, quad) in out.as_chunks::<{ 26 * 6 }>().0.iter().enumerate() {
        let (from, to) = (rope.particles()[piece].position, rope.particles()[piece + 1].position);
        let corner = |index: usize| rope_shader_position(&quad[index * 26..(index + 1) * 26], forward);
        // Corners 0 and 1 are the two sides at the start, 2 and 4 at the end.
        for (near, far, centre) in [(corner(0), corner(1), from), (corner(2), corner(4), to)] {
            let middle = [0, 1, 2].map(|i| (near[i] + far[i]) * 0.5);
            let width = ((near[0] - far[0]).powi(2) + (near[1] - far[1]).powi(2)).sqrt();
            assert!(
                (0..3).all(|i| close(middle[i], centre[i], 1e-2)),
                "piece {piece}: the ribbon is centred on its particles"
            );
            assert!(close(width, 6.0, 1e-2), "piece {piece}: as wide as the particle's size, not {width}");
        }
        assert!(
            quad.as_chunks::<26>().0.iter().all(|v| v[7] == 9.0 && v[11] == piece as f32),
            "the shader maps the texture by piece number over the length"
        );
    }

    let ((smooth, _), _) = draw(4);
    assert_eq!(smooth.vertices, 8 * 4 * 6, "each stretch is cut into its subdivisions");
    let ((capped, out), _) = draw(100);
    assert_eq!(capped.vertices, 8 * 8 * 6, "a huge subdivision is capped");
    assert!(out.iter().all(|v| v.is_finite()));
}

#[test]
fn a_rope_trail_follows_each_particle_along_its_path() {
    let mut trails = system(
        r#"{"material":"m.json","maxcount":8,"emitter":[{"name":"sphererandom","rate":0,"instantaneous":3,"distancemin":50,"distancemax":90}],
            "initializer":[{"name":"lifetimerandom","min":60,"max":60},{"name":"velocityrandom","min":"100 20 0","max":"140 40 0"}],
            "operator":[{"name":"movement","gravity":"0 -80 0"}],
            "renderer":[{"name":"ropetrail","length":0.5,"segments":5}]}"#,
        1,
    );
    run(&mut trails, 1);
    let (first, _) = vertices(&trails);
    assert_eq!((first.renderer, first.shader), (Renderer::RopeTrail, "genericropeparticle"));
    assert_eq!(first.combos, &[("GS_ENABLED", 0), ("THICKFORMAT", 1), ("TRAILRENDERER", 1)]);
    assert_eq!((first.render_var0[0], first.render_var0[3]), (5.0, 5.0));

    let mut most = 0;
    let env = Environment::default();
    for _ in 0..120 {
        trails.advance(FRAME, &env);
        let (batch, out) = vertices(&trails);
        assert!(batch.vertices <= 3 * 5 * 6, "at most its segments per particle");
        assert!((0.0..=1.0).contains(&batch.render_var0[2]));
        assert!(out.iter().all(|v| v.is_finite()));
        most = most.max(batch.vertices);
    }
    assert_eq!(most, 3 * 5 * 6, "a full trail has all its segments");
    let (_, out) = vertices(&trails);
    let head = trails.particles()[0].position;
    let forward = View::default().forward;
    let start: Vec<[f32; 3]> =
        (0..2).map(|corner| rope_shader_position(&out[corner * 26..(corner + 1) * 26], forward)).collect();
    assert!(
        (0..3).all(|i| close((start[0][i] + start[1][i]) * 0.5, head[i], 1e-2)),
        "the trail starts at the particle"
    );
    let tail = &out[4 * 6 * 26..5 * 6 * 26];
    let tail_end = [tail[4] - tail[0] + tail[0], tail[5], tail[6]];
    let behind = (tail_end[0] - head[0]).abs();
    assert!(behind > 20.0 && behind < 80.0, "and reaches back about half a second of travel: {behind}");
}

#[test]
fn children_play_with_the_particles_that_start_them() {
    let parent = |kind: &str| {
        format!(
            r#"{{"material":"parent.json","maxcount":8,"flags":1,
                 "children":[{{"name":"particles/child.json","type":"{kind}","maxcount":6}}],
                 "emitter":[{{"name":"boxrandom","rate":2,"distancemin":"200 0 0","distancemax":"300 0 0","directions":"1 0 0"}}],
                 "initializer":[{{"name":"lifetimerandom","min":1,"max":1}},{{"name":"velocityrandom","min":"0 100 0","max":"0 100 0"}}],
                 "operator":[{{"name":"movement"}}]}}"#
        )
    };
    let child = r#"{"material":"child.json","maxcount":5,"emitter":[{"name":"sphererandom","rate":0,"instantaneous":5,"distancemax":0}],
                    "initializer":[{"name":"lifetimerandom","min":0.4,"max":0.4}]}"#;
    let build = |kind: &str| {
        let parent = Rc::new(def(&parent(kind)));
        assert_eq!(parent.children().len(), 1);
        assert_eq!(parent.children()[0].path, "particles/child.json");
        let mut system = ParticleSystem::new(parent, 1);
        assert!(system.attach_child(0, Rc::new(def(child))).is_some());
        assert!(system.attach_child(3, Rc::new(def(child))).is_none());
        system
    };
    let child_of = |system: &ParticleSystem| {
        let child = system.children().next().expect("the child is attached");
        assert_eq!(child.def().material(), "child.json");
        (child.alive(), child.particles().first().map(|p| child.position_of(p)))
    };

    // A burst child at each birth: the first particle is born after half a second.
    let mut spawn = build("eventspawn");
    assert_eq!(spawn.def().children()[0].kind, ChildKind::EventSpawn);
    run(&mut spawn, 29);
    assert_eq!((spawn.alive(), child_of(&spawn).0), (0, 0));
    run(&mut spawn, 2);
    let born_at = spawn.particles()[0].position;
    let (alive, started_at) = child_of(&spawn);
    let started_at = started_at.expect("the child has particles");
    assert_eq!((alive, started_at[0]), (5, born_at[0]), "five particles where the parent's was born");
    assert!(started_at[1] < 4.0);
    run(&mut spawn, 15);
    assert_eq!(child_of(&spawn).1, Some(started_at), "they stay there while the parent's moves on");
    assert!(spawn.particles()[0].position[1] > 20.0);
    run(&mut spawn, 12);
    assert_eq!(child_of(&spawn).0, 0, "and live their own lives");

    // The same at each death, one second after the birth, where the particle got to.
    let mut death = build("eventdeath");
    run(&mut death, 85);
    assert_eq!(child_of(&death).0, 0);
    run(&mut death, 8);
    let (alive, at) = child_of(&death);
    assert_eq!(alive, 5);
    assert!(close(at.unwrap()[1], 100.0, 4.0) && at.unwrap()[0].abs() >= 200.0, "{at:?}");

    // A following child moves with its particle.
    let mut follow = build("eventfollow");
    run(&mut follow, 40);
    let (alive, at) = child_of(&follow);
    let leader = follow.particles()[0].position;
    assert_eq!((alive, at), (5, Some(leader)));
    assert!(leader[1] > 10.0);

    // A static child plays once at the origin, without any parent particle.
    let mut fixed = build("static");
    run(&mut fixed, 1);
    assert_eq!(child_of(&fixed), (5, Some([0.0; 3])));
    assert_eq!(fixed.alive(), 0);
}

#[test]
fn child_instances_are_limited_and_their_room_is_reused() {
    let parent = Rc::new(def(
        r#"{"material":"parent.json","maxcount":50,"children":[{"name":"c.json","type":"eventspawn","maxcount":3}],
            "emitter":[{"name":"sphererandom","rate":30}],"initializer":[{"name":"lifetimerandom","min":0.2,"max":0.2}]}"#,
    ));
    let child = Rc::new(def(
        r#"{"material":"child.json","maxcount":4,"emitter":[{"name":"sphererandom","rate":0,"instantaneous":4}],
            "initializer":[{"name":"lifetimerandom","min":0.5,"max":0.5}]}"#,
    ));
    let mut system = ParticleSystem::new(parent, 1);
    system.attach_child(0, child);
    let env = Environment::default();
    let mut total = 0;
    for _ in 0..600 {
        system.advance(FRAME, &env);
        let alive = system.children().next().map_or(0, ParticleSystem::alive);
        assert!(alive <= 3 * 4, "three instances of four particles at most, not {alive}");
        total += alive;
    }
    assert!(total > 600 * 8, "the instances keep being restarted: {total}");
}

#[test]
fn children_inherit_the_instance_factors() {
    let mut parent = def(
        r#"{"material":"parent.json","children":[{"name":"c.json"}],"emitter":[{"name":"sphererandom","rate":1}]}"#,
    );
    parent.apply_override(&wepaper_json::parse(r#"{"size":2,"alpha":0.5,"colorn":"0 1 0"}"#).unwrap());
    let mut system = ParticleSystem::new(Rc::new(parent), 1);
    system.attach_child(
        0,
        Rc::new(def(
            r#"{"material":"child.json","emitter":[{"name":"sphererandom","rate":0,"instantaneous":1}],
                "initializer":[{"name":"sizerandom","min":10,"max":10},{"name":"lifetimerandom","min":5,"max":5}]}"#,
        )),
    );
    run(&mut system, 2);
    let child = system.children().next().unwrap();
    assert_eq!(child.particles()[0].size, 20.0);
    let (_, out) = vertices(child);
    assert_eq!(&out[7..11], &[0.0, 1.0, 0.0, 0.5]);
}

#[test]
fn an_override_can_change_while_the_system_plays() {
    let shared = Rc::new(def(r#"{"material":"parent.json","maxcount":100,"children":[{"name":"c.json"}],
            "emitter":[{"name":"sphererandom","rate":30}],
            "initializer":[{"name":"lifetimerandom","min":50,"max":50},{"name":"sizerandom","min":10,"max":10}]}"#));
    let mut system = ParticleSystem::new(shared.clone(), 1);
    system.attach_child(0, shared.clone());
    run(&mut system, 60);
    assert!(system.alive().abs_diff(30) <= 1);
    system.apply_override(&wepaper_json::parse(r#"{"count":0,"size":2,"alpha":0.25}"#).unwrap());
    run(&mut system, 60);
    assert!(system.alive().abs_diff(30) <= 1, "a count of nothing stops the emission");
    system.apply_override(&wepaper_json::parse(r#"{"count":{"script":"x","value":2}}"#).unwrap());
    run(&mut system, 30);
    assert!(
        system.alive().abs_diff(60) <= 2,
        "{} alive after half a second at twice the rate",
        system.alive()
    );
    let newest = system.particles()[system.alive() - 1];
    assert_eq!(
        (system.particles()[0].size, newest.size),
        (10.0, 20.0),
        "size applies to particles born from then on"
    );
    let child = system.children().next().unwrap();
    assert_eq!(child.particles()[child.alive() - 1].size, 20.0, "children follow");
    assert_eq!(vertices(&system).1[10], 0.25, "opacity applies to every particle at once");
    assert_eq!(shared.max_count(), 100, "the definition others hold is left alone");
}

#[test]
fn no_document_makes_the_system_panic() {
    let documents = [
        r#"{"material":"m.json"}"#,
        r#"{"material":"m.json","maxcount":-5,"starttime":-3,"emitter":7,"initializer":"x","operator":{},"renderer":[null],"children":[1,{}],"controlpoint":[3]}"#,
        r#"{"material":"m.json","maxcount":1e30,"starttime":1e30,"emitter":[{"name":"sphererandom","rate":0,"instantaneous":1e30,"duration":1e-9}],
            "initializer":[{"name":"lifetimerandom","min":1e-6,"max":1e-6}],"operator":[{"name":"movement"}]}"#,
        r#"{"material":"m.json","maxcount":2000,"starttime":3,"emitter":[{"name":"sphererandom","rate":1e30,"instantaneous":1e30,"distancemax":1e30,"speedmax":1e38}],
            "initializer":[{"name":"lifetimerandom","min":-1,"max":-1},{"name":"sizerandom","min":1e38,"max":1e38},{"name":"velocityrandom","min":"1e38 1e38 1e38","max":"1e38 1e38 1e38"}],
            "operator":[{"name":"movement","drag":-1e30,"gravity":"1e38 1e38 1e38"},{"name":"sizechange","endvalue":1e38},{"name":"turbulence","scale":1e30,"speedmax":1e38},
                        {"name":"vortex","speedinner":1e38,"speedouter":1e38},{"name":"controlpointattract","scale":1e38,"threshold":1e38},{"name":"oscillateposition","frequencymax":1e38,"scalemax":1e38}],
            "renderer":[{"name":"rope","subdivision":1e9}]}"#,
        r#"{"material":"m.json","maxcount":20,"emitter":[{"name":"boxrandom","rate":500,"distancemax":0}],
            "initializer":[{"name":"lifetimerandom","min":0,"max":0},{"name":"mapsequencearoundcontrolpoint","count":0,"axis":"0 0 0","controlpoint":50},
                           {"name":"mapsequencebetweencontrolpoints","count":-4,"controlpointend":-9},{"name":"turbulentvelocityrandom","timescale":1e30,"scale":1e30}],
            "operator":[{"name":"alphafade","fadeintime":0,"fadeouttime":1},{"name":"sizechange","starttime":1,"endtime":0},{"name":"colorchange","starttime":0.5,"endtime":0.5}],
            "renderer":[{"name":"ropetrail","length":0,"segments":0}]}"#,
        r#"{"material":"m.json","maxcount":30,"emitter":[{"name":"sphererandom","rate":300,"distancemax":0}],
            "initializer":[{"name":"lifetimerandom","min":1,"max":1}],"renderer":[{"name":"ropetrail","length":-2,"segments":1e9}]}"#,
        r#"{"material":"m.json","maxcount":30,"emitter":[{"name":"sphererandom","rate":300,"distancemax":0,"directions":"0 0 0"}],
            "initializer":[{"name":"lifetimerandom","min":1,"max":1}],"renderer":[{"name":"spritetrail","length":"x","maxlength":-1}]}"#,
    ];
    for (index, document) in documents.iter().enumerate() {
        let parsed = wepaper_json::parse(document).unwrap_or_else(|e| panic!("document {index}: {e}"));
        let mut def = ParticleDef::parse(&parsed).unwrap_or_else(|e| panic!("document {index}: {e}"));
        def.apply_override(&wepaper_json::parse(r#"{"count":3,"size":-4,"speed":"fast","lifetime":[],"colorn":7,"controlpoint3":"1","controlpoint9":"2 2 2"}"#).unwrap());
        let def = Rc::new(def);
        let mut system = ParticleSystem::new(def.clone(), index as u64);
        for child in 0..def.children().len() {
            system.attach_child(child, def.clone());
        }
        let mut env = Environment::of(&def);
        env.control_points[2] = [f32::NAN, f32::INFINITY, 0.0];
        let mut out = Vec::new();
        for frame in 0..90 {
            env.pointer = match frame % 3 {
                0 => None,
                1 => Some([frame as f32, -3.0, 0.0]),
                _ => Some([f32::NAN, f32::INFINITY, 0.0]),
            };
            system.advance(
                if frame == 40 {
                    f32::NAN
                } else if frame == 41 {
                    -1.0
                } else if frame == 42 {
                    1e9
                } else {
                    FRAME
                },
                &env,
            );
            out.clear();
            let batch = system.write_vertices(&View::default(), &mut out);
            assert_eq!(out.len(), batch.vertices * batch.stride(), "document {index}");
            assert!(
                out.iter().all(|v| v.is_finite()),
                "document {index}, frame {frame}: a vertex is not a number"
            );
            let flat = View { forward: [0.0; 3], ..View::default() };
            out.clear();
            system.write_vertices(&flat, &mut out);
            assert!(out.iter().all(|v| v.is_finite()), "document {index}: a view without a direction");
        }
    }
    let mut swollen = def(r#"{"material":"m.json","maxcount":1e30}"#);
    swollen.apply_override(&wepaper_json::parse(r#"{"count":1e30}"#).unwrap());
    assert_eq!(swollen.max_count(), 200_000, "no document gets more room than the engine's limit");
    for refused in ["null", "[]", "3", r#"{"maxcount":5}"#, r#"{"material":7}"#] {
        assert!(ParticleDef::parse(&wepaper_json::parse(refused).unwrap()).is_err(), "{refused}");
    }
}

#[test]
fn ten_thousand_particles_for_six_hundred_frames() {
    let mut busy = system(
        r#"{"material":"m.json","maxcount":10000,"starttime":4,
            "emitter":[{"name":"boxrandom","rate":4000,"distancemax":"960 540 0"}],
            "initializer":[{"name":"lifetimerandom","min":2.4,"max":2.6},{"name":"sizerandom","min":4,"max":30,"exponent":2},
                           {"name":"velocityrandom","min":"-60 -60 0","max":"60 60 0"},{"name":"colorrandom","min":"255 180 90"},
                           {"name":"alpharandom","min":0.3,"max":1},{"name":"rotationrandom"},{"name":"angularvelocityrandom"}],
            "operator":[{"name":"movement","gravity":"0 -30 0","drag":0.2},{"name":"angularmovement"},{"name":"alphafade"},
                        {"name":"sizechange","startvalue":0.5,"endvalue":1.5},{"name":"colorchange","endvalue":"1 0.4 0.2"},
                        {"name":"oscillateposition","frequencymax":2,"scalemax":20},{"name":"oscillatealpha","scalemin":0.5},
                        {"name":"turbulence","speedmin":50,"speedmax":150},{"name":"controlpointattract","scale":-400,"threshold":200}]}"#,
        1,
    );
    let env = Environment::default();
    let view = View::default();
    let mut out = Vec::new();
    busy.advance(FRAME, &env);
    let (mut simulating, mut writing, mut alive, mut written) = (0.0f64, 0.0f64, 0usize, 0usize);
    for _ in 0..600 {
        let started = Instant::now();
        busy.advance(FRAME, &env);
        simulating += started.elapsed().as_secs_f64();
        let started = Instant::now();
        out.clear();
        written += busy.write_vertices(&view, &mut out).vertices;
        writing += started.elapsed().as_secs_f64();
        alive += busy.alive();
    }
    assert!(alive / 600 > 9000, "the system is meant to run full: {} alive on average", alive / 600);
    assert_eq!(written, alive * 6);
    eprintln!(
        "{} particles on average, 9 operators: {:.3} ms a frame to simulate, {:.3} ms to write vertices ({} frames in {:.1} ms)",
        alive / 600,
        simulating * 1000.0 / 600.0,
        writing * 1000.0 / 600.0,
        600,
        (simulating + writing) * 1000.0
    );
}

fn env_dir(name: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os(name)?);
    dir.is_dir().then_some(dir)
}

/// A child a document names: its index among the children and its path.
type NamedChild = (usize, String);

/// Loads a particle document and the children it names, as the scene will.
fn load(
    files: &Files,
    path: &str,
    instance_override: &Value,
    depth: usize,
) -> Result<(Rc<ParticleDef>, Vec<NamedChild>), String> {
    let text = files.read_text(path).map_err(|e| format!("{path}: {e}"))?;
    let document = wepaper_json::parse(&text).map_err(|e| format!("{path}: {e}"))?;
    let mut def = ParticleDef::parse(&document).map_err(|e| format!("{path}: {e}"))?;
    def.apply_override(instance_override);
    let children = if depth < 4 {
        def.children().iter().enumerate().map(|(index, child)| (index, child.path.clone())).collect()
    } else {
        Vec::new()
    };
    Ok((Rc::new(def), children))
}

fn attach(
    files: &Files,
    system: &mut ParticleSystem,
    children: &[NamedChild],
    depth: usize,
    tally: &mut BTreeMap<String, usize>,
) -> Result<(), String> {
    for (index, path) in children {
        let (def, grandchildren) = load(files, path, &Value::Null, depth)?;
        *tally.entry(format!("child {:?}", system.def().children()[*index].kind)).or_default() += 1;
        *tally.entry(format!("renderer {:?}", def.renderer())).or_default() += 1;
        let child = system.attach_child(*index, def).ok_or_else(|| format!("{path}: not attached"))?;
        attach(files, child, &grandchildren, depth + 1, tally)?;
    }
    Ok(())
}

/// Writes a system and its children, checking every batch; returns the vertices written.
fn check_batches(
    system: &ParticleSystem,
    view: &View,
    out: &mut Vec<f32>,
    what: &str,
) -> Result<usize, String> {
    out.clear();
    let batch = system.write_vertices(view, out);
    if out.len() != batch.vertices * batch.stride() {
        return Err(format!(
            "{what}: {} floats for {} vertices of {}",
            out.len(),
            batch.vertices,
            batch.stride()
        ));
    }
    if let Some(at) = out.iter().position(|v| !v.is_finite()) {
        return Err(format!("{what}: float {at} of the batch is not a number"));
    }
    if system.particles().iter().any(|p| system.position_of(p).iter().any(|v| !v.is_finite())) {
        return Err(format!("{what}: a particle is nowhere"));
    }
    if system.alive() > system.def().max_count().max(100_000) {
        return Err(format!("{what}: {} particles alive", system.alive()));
    }
    let expected = match batch.renderer {
        Renderer::Sprite | Renderer::SpriteTrail => batch.vertices == system.alive() * 6,
        // Ribbons leave out pieces without a length; they never have more than a piece per subdivision or segment.
        Renderer::Rope => batch.vertices <= system.alive() * 8 * 6,
        Renderer::RopeTrail => batch.vertices <= system.alive() * 64 * 6,
    };
    if !expected || !batch.vertices.is_multiple_of(6) {
        return Err(format!(
            "{what}: {} vertices for {} particles drawn as {:?}",
            batch.vertices,
            system.alive(),
            batch.renderer
        ));
    }
    let mut total = batch.vertices;
    for child in system.children() {
        total += check_batches(child, view, out, what)?;
    }
    Ok(total)
}

#[test]
fn every_particle_system_of_every_wallpaper_simulates() {
    let (Some(assets), Some(workshop)) = (env_dir("WEPAPER_TEST_ASSETS"), env_dir("WEPAPER_TEST_WORKSHOP"))
    else {
        eprintln!("skipped: WEPAPER_TEST_ASSETS / WEPAPER_TEST_WORKSHOP are not set");
        return;
    };
    let mut tally: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut idle = Vec::new();
    let (mut scenes, mut objects, mut drawn, mut vertices) = (0, 0, 0, 0usize);
    let mut wallpapers: Vec<PathBuf> = std::fs::read_dir(&workshop)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    wallpapers.sort();
    let view = View::default();
    let mut out = Vec::new();
    for dir in wallpapers {
        let mut files = Files::new();
        files.mount_dir(assets.clone());
        files.mount_dir(dir.clone());
        let package = dir.join("scene.pkg");
        if package.is_file() {
            match Archive::open(&package) {
                Ok(archive) => files.mount_archive(archive),
                Err(e) => {
                    failures.push(format!("{}: {e}", package.display()));
                    continue;
                }
            }
        }
        let Ok(text) = files.read_text("scene.json") else { continue };
        let scene = match wepaper_json::parse(&text) {
            Ok(scene) => scene,
            Err(e) => {
                failures.push(format!("{}: scene.json: {e}", dir.display()));
                continue;
            }
        };
        let mut has_particles = false;
        for object in scene.get("objects").items() {
            let Some(path) = object.get("particle").as_str() else { continue };
            has_particles = true;
            objects += 1;
            let what = format!("{}: {path}", dir.file_name().unwrap_or_default().to_string_lossy());
            let checked = (|| {
                let (def, children) = load(&files, path, object.get("instanceoverride"), 0)?;
                *tally.entry(format!("renderer {:?}", def.renderer())).or_default() += 1;
                let mut system = ParticleSystem::new(def.clone(), objects as u64);
                attach(&files, &mut system, &children, 1, &mut tally)?;
                let mut env = Environment::of(&def);
                let mut most = 0;
                for frame in 0..120 {
                    env.time += FRAME;
                    // The pointer sweeps across, leaves, and comes back.
                    env.pointer = (frame % 60 < 45).then(|| {
                        [(frame as f32 * 13.0) % 400.0 - 200.0, (frame as f32 * 7.0) % 300.0 - 150.0, 0.0]
                    });
                    system.advance(FRAME, &env);
                    most = most.max(check_batches(&system, &view, &mut out, &what)?);
                }
                Ok::<usize, String>(most)
            })();
            match checked {
                Ok(most) => {
                    drawn += usize::from(most > 0);
                    vertices += most;
                    if most == 0 {
                        idle.push(what);
                    }
                }
                Err(why) => failures.push(why),
            }
        }
        scenes += usize::from(has_particles);
    }
    eprintln!(
        "{objects} particle objects in {scenes} scenes; {drawn} drew something within two seconds; {vertices} vertices at their fullest; {tally:?}"
    );
    eprintln!("drew nothing: {idle:?}");
    assert!(
        failures.is_empty(),
        "{} failures, first ones:\n{}",
        failures.len(),
        failures.iter().take(15).cloned().collect::<Vec<_>>().join("\n")
    );
    assert!(objects > 0, "nothing was checked");
}
