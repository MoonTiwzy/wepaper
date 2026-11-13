//! Documents: typed values out of loosely typed JSON.

use wepaper_engine::doc::{Alignment, Blending, Dyn, EffectDoc, Kind, MaterialDoc, ObjectKind, Project, PropertyKind, Scene};
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
    assert!(matches!(&scene.objects[2].kind, ObjectKind::Text(t) if t.text.value == "hi" && t.text.user.is_some()));
    assert!(matches!(&scene.objects[3].kind, ObjectKind::Sound(s) if s.volume.value == 0.5 && s.mode == "loop"));
    assert!(matches!(scene.objects[4].kind, ObjectKind::Unsupported("light")));
    assert!(matches!(scene.objects[5].kind, ObjectKind::Group));
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
    assert_eq!((project.kind, project.title.as_str(), project.workshop_id.as_deref()), (Kind::Scene, "T", Some("123")));
    let names: Vec<&str> = project.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["head", "a", "b", "c"]);
    assert_eq!(project.properties[0].kind, PropertyKind::Group);
    assert!(matches!(project.properties[1].kind, PropertyKind::Slider { min, max, .. } if min == 0.0 && max == 5.0));
    assert!(matches!(&project.properties[3].kind, PropertyKind::Combo(options) if options.len() == 1));
    assert_eq!(Project::parse(&parse(r#"{"dependency": "99", "preset": {"a": 1}}"#).unwrap()).kind, Kind::Preset);
    assert_eq!(Project::parse(&parse(r#"{"type": "video", "file": "a.mp4"}"#).unwrap()).kind, Kind::Video);
    assert_eq!(Project::parse(&parse("{}").unwrap()).kind, Kind::Scene);
}
