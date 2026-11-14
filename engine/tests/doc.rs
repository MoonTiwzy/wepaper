//! Documents: typed values out of loosely typed JSON.

use wepaper_engine::doc::{
    Alignment, Blending, Dyn, EffectDoc, Kind, LightKind, MaterialDoc, ObjectKind, Project, PropertyKind,
    Scene,
};
use wepaper_json::parse;

#[test]
fn vectors_from_strings_arrays_and_single_numbers() {
    let doc = parse(r#"{"a": "1 2.5 -3", "b": [4, 5], "c": 2, "d": "0.5"}"#).unwrap();
    assert_eq!(Dyn::<[f32; 3]>::read(doc.get("a"), [0.0; 3]).value, [1.0, 2.5, -3.0]);
    assert_eq!(Dyn::<[f32; 3]>::read(doc.get("b"), [9.0; 3]).value, [4.0, 5.0, 0.0]);
    assert_eq!(Dyn::<[f32; 3]>::read(doc.get("c"), [0.0; 3]).value, [2.0; 3]);
    assert_eq!(Dyn::<f32>::read(doc.get("d"), 0.0).value, 0.5);
    assert_eq!(Dyn::<f32>::read(doc.get("missing"), 7.0).value, 7.0);
}

#[test]
fn user_link_with_condition() {
    let doc = parse(r#"{"plain": {"user": "speed", "value": 2}, "cond": {"user": {"name": "mode", "condition": "2"}, "value": false},
                        "scripted": {"script": "export function update(v) { return v }", "scriptproperties": {"k": 1}, "value": "1 1 1"}}"#)
        .unwrap();
    let plain = Dyn::<f32>::read(doc.get("plain"), 0.0);
    assert_eq!((plain.value, plain.user.as_ref().unwrap().name.as_str()), (2.0, "speed"));
    assert!(plain.user.unwrap().condition.is_none() && plain.script.is_none());
    let cond = Dyn::<bool>::read(doc.get("cond"), true);
    assert!(!cond.value);
    assert_eq!(cond.user.unwrap().condition.as_deref(), Some("2"));
    let scripted = Dyn::<[f32; 3]>::read(doc.get("scripted"), [0.0; 3]);
    assert_eq!(scripted.value, [1.0; 3]);
    assert!(scripted.is_driven());
    assert_eq!(scripted.script.unwrap().properties.get("k").as_i64(), Some(1));
}

#[test]
fn image_object_defaults() {
    let scene = Scene::parse(
        &parse(r#"{"general": {"orthogonalprojection": {"width": 1920, "height": 1080}, "clearcolor": "0.1 0.2 0.3"},
                   "objects": [{"id": 7, "name": "bg", "image": "models/bg.json", "origin": "960 540 0"},
                               {"id": 8, "name": "fx", "image": null, "particle": "particles/rain.json", "parent": 7},
                               {"id": 9, "name": "label", "text": {"user": "caption", "value": "hi"}, "font": "fonts/a.ttf"},
                               {"id": 10, "name": "music", "sound": ["sounds/a.mp3"], "volume": 0.5},
                               {"id": 11, "name": "lamp", "light": "point"}, {"id": 12, "name": "folder"}]}"#)
            .unwrap(),
    );
    assert_eq!(scene.general.orthogonal, Some([1920.0, 1080.0]));
    assert_eq!(scene.general.clear_color.value, [0.1, 0.2, 0.3]);
    assert_eq!(scene.objects.len(), 6);
    let bg = &scene.objects[0];
    assert_eq!((bg.id, bg.name.as_str(), bg.parent), (7, "bg", None));
    assert_eq!((bg.origin.value, bg.scale.value, bg.visible.value), ([960.0, 540.0, 0.0], [1.0; 3], true));
    let ObjectKind::Image(image) = &bg.kind else { panic!("not an image") };
    assert_eq!((image.alpha.value, image.color.value, image.blend_mode), (1.0, [1.0; 3], 0));
    assert_eq!(image.alignment, Alignment::default());
    assert!(image.effects.is_empty() && image.size.is_none());
    assert!(matches!(&scene.objects[1].kind, ObjectKind::Particle(p) if p.file == "particles/rain.json"));
    assert_eq!(scene.objects[1].parent, Some(7));
    assert!(
        matches!(&scene.objects[2].kind, ObjectKind::Text(t) if t.text.value == "hi" && t.text.user.is_some())
    );
    assert!(
        matches!(&scene.objects[3].kind, ObjectKind::Sound(s) if s.volume.value == 0.5 && s.mode == "loop")
    );
    assert!(matches!(&scene.objects[4].kind, ObjectKind::Light(light) if light.kind == LightKind::Point));
    assert!(matches!(scene.objects[5].kind, ObjectKind::Group));
}

#[test]
fn a_perspective_scene_with_models_and_lights() {
    let scene = Scene::parse(
        &parse(r#"{"camera": {"center": "0 0.5 6", "eye": "0 0.5 7", "up": "0 1 0"},
                   "general": {"orthogonalprojection": null, "fov": 35, "nearz": 0.1, "farz": 1000,
                               "ambientcolor": "0.75 0.75 0.75", "skylightcolor": {"user": "sky", "value": "0.6 0.6 0.6"},
                               "hdr": true, "bloomhdrstrength": 0.8, "bloomhdrthreshold": 0.62, "bloomhdrfeather": 0.4,
                               "bloomhdrscatter": 3.0, "bloomhdriterations": 8, "bloomtint": "1 0.5 0.25"},
                   "objects": [
                     {"id": 23, "name": "Main Light", "light": "ldirectional", "color": "1 1 1", "intensity": 1.25,
                      "radius": 15.65, "exponent": 2.0, "castshadow": true, "angles": "0 0.9 -2.25"},
                     {"id": 24, "light": "point", "color": "0 0.5 0", "intensity": {"script": "x", "value": 6}, "radius": 1000},
                     {"id": 25, "light": "spot", "radius": 3, "innercone": 20, "outercone": 40},
                     {"id": 140, "name": "mario", "model": "models/mario/mario.mdl", "scale": "0.012 0.012 0.012",
                      "animationlayers": [{"animation": 139, "name": "Idle", "rate": 1.0}]},
                     {"id": 406, "name": "head", "model": "models/head/head.mdl", "parent": 140, "attachment": "Arm"},
                     {"id": 97, "name": "Cam", "camera": "default", "fov": 25.0, "zoom": 1.5, "origin": "0 3.25 13",
                      "angles": "-0.24 0 0", "path": "scripts/camera_paths_97.json", "queuemode": "random"}]}"#)
            .unwrap(),
    );
    let general = &scene.general;
    assert_eq!(general.orthogonal, None);
    assert_eq!((general.fov.value, general.near, general.far), (35.0, 0.1, 1000.0));
    assert_eq!(general.ambient.value, [0.75; 3]);
    assert_eq!(general.skylight.value, [0.6; 3]);
    assert!(general.skylight.user.is_some(), "the sky's colour can follow a property");
    assert!(general.hdr);
    assert_eq!(
        (
            general.bloom_hdr.strength.value,
            general.bloom_hdr.threshold.value,
            general.bloom_hdr.feather,
            general.bloom_hdr.scatter,
            general.bloom_hdr.iterations
        ),
        (0.8, 0.62, 0.4, 3.0, 8)
    );
    assert_eq!(general.bloom_hdr.tint, [1.0, 0.5, 0.25]);
    assert_eq!((scene.eye, scene.center), ([0.0, 0.5, 7.0], [0.0, 0.5, 6.0]));

    let ObjectKind::Light(sun) = &scene.objects[0].kind else { panic!("not a light") };
    assert_eq!(sun.kind, LightKind::Directional, "the editor writes `ldirectional`");
    assert_eq!(
        (sun.color.value, sun.intensity.value, sun.radius, sun.exponent),
        ([1.0; 3], 1.25, 15.65, 2.0)
    );
    assert!(sun.cast_shadow);
    let ObjectKind::Light(lamp) = &scene.objects[1].kind else { panic!("not a light") };
    assert_eq!((lamp.kind, lamp.intensity.value, lamp.radius), (LightKind::Point, 6.0, 1000.0));
    assert!(lamp.intensity.is_driven(), "a script drives the intensity");
    let ObjectKind::Light(spot) = &scene.objects[2].kind else { panic!("not a light") };
    assert_eq!((spot.kind, spot.inner_cone, spot.outer_cone), (LightKind::Spot, 20.0, 40.0));

    let ObjectKind::Model(mario) = &scene.objects[3].kind else { panic!("not a model") };
    assert_eq!(mario.file, "models/mario/mario.mdl");
    assert_eq!(mario.animation_layers.items().count(), 1);
    assert_eq!(scene.objects[3].attachment, None);
    assert!(matches!(scene.objects[4].kind, ObjectKind::Model(_)));
    assert_eq!(scene.objects[4].attachment.as_deref(), Some("Arm"), "it hangs from its parent's arm");

    let ObjectKind::Camera(camera) = &scene.objects[5].kind else { panic!("not a camera") };
    assert_eq!((camera.name.as_str(), camera.fov, camera.zoom), ("default", 25.0, 1.5));
    assert_eq!(camera.path.as_deref(), Some("scripts/camera_paths_97.json"));
    assert_eq!(scene.objects[5].origin.value, [0.0, 3.25, 13.0]);
}

#[test]
fn model_materials_are_solid_and_depth_tested_unless_they_say_otherwise() {
    let plain = MaterialDoc::parse_model(
        &parse(r#"{"passes": [{"shader": "generic4", "textures": ["models/a"]}]}"#).unwrap(),
    );
    let pass = &plain.passes[0];
    assert_eq!(pass.blending, Blending::Normal);
    assert!(pass.depth_test && pass.depth_write && pass.cull);
    let said = MaterialDoc::parse_model(
        &parse(r#"{"passes": [{"shader": "generic4", "blending": "translucent", "depthwrite": "disabled", "cullmode": "nocull"}]}"#)
            .unwrap(),
    );
    let pass = &said.passes[0];
    assert_eq!(pass.blending, Blending::Translucent);
    assert!(pass.depth_test && !pass.depth_write && !pass.cull);
    // An image's material keeps the defaults of a flat layer.
    let image = MaterialDoc::parse(&parse(r#"{"passes": [{"shader": "genericimage2"}]}"#).unwrap());
    assert!(!image.passes[0].depth_test && !image.passes[0].cull);
}

#[test]
fn effect_passes_and_fbos() {
    let effect = EffectDoc::parse(
        &parse(r#"{"name": "blur", "passes": [{"material": "m/down.json", "target": "_rt_Q1", "bind": [{"name": "previous", "index": 0}]},
                   {"command": "copy", "source": "_rt_Q1", "target": "_rt_Q2"}, {"material": "m/combine.json", "bind": [{"name": "_rt_Q1", "index": 0}, {"name": "previous", "index": 2}]}],
                   "fbos": [{"name": "_rt_Q1", "scale": 4, "format": "rgba_backbuffer"}, {"name": "_rt_Q2", "scale": 4}]}"#)
            .unwrap(),
    );
    assert_eq!(effect.passes.len(), 3);
    assert_eq!(effect.passes[0].target.as_deref(), Some("_rt_Q1"));
    assert_eq!((effect.passes[0].binds[0].name.as_str(), effect.passes[0].binds[0].slot), ("previous", 0));
    assert_eq!(effect.passes[1].command.as_deref(), Some("copy"));
    assert!(effect.passes[2].target.is_none());
    assert_eq!(effect.passes[2].binds[1].slot, 2);
    assert_eq!((effect.fbos[0].scale, effect.fbos[1].scale, effect.fbos.len()), (4.0, 4.0, 2));
}

#[test]
fn material_blending_modes() {
    let material = MaterialDoc::parse(
        &parse(r#"{"passes": [{"shader": "genericimage2", "blending": "additive", "textures": ["a", null, "c"], "combos": {"MASK": 1},
                   "constantshadervalues": {"ui_speed": 2}, "cullmode": "nocull", "depthtest": "disabled"}, {"shader": "flat"}]}"#)
            .unwrap(),
    );
    let pass = &material.passes[0];
    assert_eq!((pass.shader.as_str(), pass.blending), ("genericimage2", Blending::Additive));
    assert_eq!(pass.textures, [Some("a".to_owned()), None, Some("c".to_owned())]);
    assert_eq!(pass.combos.get("MASK"), Some(&1));
    assert_eq!(pass.constants[0].0, "ui_speed");
    assert!(!pass.cull && !pass.depth_test);
    assert_eq!(material.passes[1].blending, Blending::Translucent);
    assert_eq!(Blending::from_name("normal"), Blending::Normal);
}

#[test]
fn project_kinds_and_properties() {
    let project = Project::parse(
        &parse(r#"{"type": "Scene", "file": "scene.json", "title": "T", "workshopid": 123, "general": {"properties": {
                   "b": {"type": "bool", "text": "B", "value": true, "order": 2},
                   "a": {"type": "slider", "text": "A", "value": 1, "min": 0, "max": 5, "order": 1},
                   "c": {"type": "combo", "value": "x", "options": [{"label": "X", "value": "x"}], "order": 3},
                   "head": {"type": "text", "text": "Heading", "order": 0}}}}"#)
            .unwrap(),
    );
    assert_eq!(
        (project.kind, project.title.as_str(), project.workshop_id.as_deref()),
        (Kind::Scene, "T", Some("123"))
    );
    let names: Vec<&str> = project.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["head", "a", "b", "c"]);
    assert_eq!(project.properties[0].kind, PropertyKind::Group);
    assert!(
        matches!(project.properties[1].kind, PropertyKind::Slider { min, max, .. } if min == 0.0 && max == 5.0)
    );
    assert!(matches!(&project.properties[3].kind, PropertyKind::Combo(options) if options.len() == 1));
    assert_eq!(
        Project::parse(&parse(r#"{"dependency": "99", "preset": {"a": 1}}"#).unwrap()).kind,
        Kind::Preset
    );
    assert_eq!(Project::parse(&parse(r#"{"type": "video", "file": "a.mp4"}"#).unwrap()).kind, Kind::Video);
    assert_eq!(Project::parse(&parse("{}").unwrap()).kind, Kind::Scene);
}

#[test]
fn what_a_person_types_is_a_number_or_nothing() {
    use wepaper_engine::doc::{typed_flag, typed_floats};
    use wepaper_json::Value;
    assert_eq!(typed_floats(&Value::from(0.5)), Some(vec![0.5]));
    assert_eq!(typed_floats(&Value::from("1 0.5, 0")), Some(vec![1.0, 0.5, 0.0]));
    assert_eq!(typed_floats(&wepaper_json::parse("[1, 2]").unwrap()), Some(vec![1.0, 2.0]));
    // Text that is not numbers is not zero, as it would be in a wallpaper's document.
    assert_eq!(typed_floats(&Value::from("banana")), None);
    assert_eq!(typed_floats(&Value::from("1 two 3")), None);
    assert_eq!(typed_floats(&Value::from("")), None);
    assert_eq!(typed_floats(&Value::from("nan")), None);
    assert_eq!(typed_floats(&Value::Null), None);
    assert_eq!(typed_floats(&Value::from(true)), None);

    assert_eq!(typed_flag(&Value::from(true)), Some(true));
    assert_eq!(typed_flag(&Value::from("off")), Some(false));
    assert_eq!(typed_flag(&Value::from(1)), Some(true));
    assert_eq!(typed_flag(&Value::from(2)), None);
    assert_eq!(typed_flag(&Value::from("banana")), None);
    assert_eq!(typed_flag(&Value::Null), None);
}

#[test]
fn a_bind_to_a_slot_that_cannot_be_is_dropped() {
    use wepaper_engine::doc::EffectDoc;
    let effect = EffectDoc::parse(
        &wepaper_json::parse(
            r#"{"passes": [{"material": "m.json", "bind": [
                {"name": "previous", "index": 0},
                {"name": "_rt_a", "index": 1e30},
                {"name": "_rt_b", "index": 4294967295},
                {"name": "_rt_c", "index": "1e30 -1e30 nan"},
                {"name": "_rt_d", "index": 15}]}]}"#,
        )
        .unwrap(),
    );
    let slots: Vec<usize> = effect.passes[0].binds.iter().map(|bind| bind.slot).collect();
    assert_eq!(slots, [0, 15], "a number from a document never sizes anything");
}
