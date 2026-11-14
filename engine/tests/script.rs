//! SceneScript against a host that is a table of layers.
//!
//! The corpus test at the end runs when these are set and is skipped otherwise:
//!
//! - `WEPAPER_TEST_ASSETS`: Wallpaper Engine's `assets` folder
//! - `WEPAPER_TEST_WORKSHOP`: a folder of wallpapers (`steamapps/workshop/content/431960`)
#![cfg(feature = "script")]

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use log::Level;
use wepaper_engine::read::{Archive, Files};
use wepaper_engine::script::{
    Binding, CursorEvent, Datum, Environment, ErrorKind, Field, Handler, Host, LayerId, Limits, MediaEvent,
    Runtime, ScriptId,
};
use wepaper_json::Value;

#[derive(Default)]
struct Layer {
    name: String,
    id: i64,
    /// By the field's `Display`.
    fields: HashMap<String, Datum>,
}

/// A scene that is only its values.
#[derive(Default)]
struct Mock {
    layers: Vec<Layer>,
    user: HashMap<String, Value>,
    environment: Environment,
    files: Option<Files>,
    storage: HashMap<String, String>,
    logs: Vec<(Level, String)>,
    /// Methods scripts called, as `layer:method(arguments)`.
    calls: Vec<String>,
    /// Whether `call` carries methods out.
    answers_calls: bool,
}

impl Mock {
    fn with_layers(names: &[&str]) -> Self {
        let mut mock = Mock::default();
        for (i, name) in names.iter().enumerate() {
            mock.layer(name, 100 + i as i64);
        }
        mock.environment.canvas_size = [1920.0, 1080.0];
        mock.environment.screen_size = [2560.0, 1440.0];
        mock
    }

    fn layer(&mut self, name: &str, id: i64) -> LayerId {
        let mut fields = HashMap::new();
        fields.insert(Field::Name.to_string(), Datum::Text(name.to_owned()));
        self.layers.push(Layer { name: name.to_owned(), id, fields });
        LayerId(self.layers.len() as u32 - 1)
    }

    fn field(&self, layer: u32, field: Field<'_>) -> Datum {
        self.get(LayerId(layer), field)
    }

    fn put(&mut self, layer: u32, field: Field<'_>, value: Datum) {
        self.set(LayerId(layer), field, value);
    }

    fn logged(&self, level: Level) -> Vec<&str> {
        self.logs.iter().filter(|(l, _)| *l == level).map(|(_, message)| message.as_str()).collect()
    }
}

impl Host for Mock {
    fn layer_named(&self, name: &str) -> Option<LayerId> {
        self.layers.iter().position(|layer| layer.name == name).map(|i| LayerId(i as u32))
    }

    fn layer_with_id(&self, id: i64) -> Option<LayerId> {
        self.layers.iter().position(|layer| layer.id == id).map(|i| LayerId(i as u32))
    }

    fn layers(&self) -> Vec<LayerId> {
        (0..self.layers.len() as u32).map(LayerId).collect()
    }

    fn get(&self, layer: LayerId, field: Field<'_>) -> Datum {
        self.layers
            .get(layer.0 as usize)
            .and_then(|layer| layer.fields.get(&field.to_string()))
            .cloned()
            .unwrap_or_default()
    }

    fn set(&mut self, layer: LayerId, field: Field<'_>, value: Datum) {
        if let Some(layer) = self.layers.get_mut(layer.0 as usize) {
            layer.fields.insert(field.to_string(), value);
        }
    }

    fn user_property(&self, name: &str) -> Value {
        self.user.get(name).cloned().unwrap_or(Value::Null)
    }

    fn environment(&self) -> Environment {
        self.environment.clone()
    }

    fn read_script(&self, path: &str) -> Option<String> {
        self.files.as_ref()?.read_text(path).ok()
    }

    fn log(&mut self, level: Level, message: &str) {
        self.logs.push((level, message.to_owned()));
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

    fn call(
        &mut self,
        layer: LayerId,
        field: Option<Field<'_>>,
        method: &str,
        arguments: &[Datum],
    ) -> Option<Datum> {
        if !self.answers_calls {
            return None;
        }
        let bound = field.map(|field| format!(" on {field}")).unwrap_or_default();
        self.calls.push(format!("{}:{method}({arguments:?}){bound}", layer.0));
        Some(match method {
            "animation.getFrame" => Datum::Number(7.0),
            "sortLayer" => Datum::Bool(true),
            _ => Datum::Null,
        })
    }
}

fn binding(layer: u32, field: Field<'static>, value: Datum) -> Binding<'static> {
    static NONE: Value = Value::Null;
    Binding { layer: LayerId(layer), field, value, properties: &NONE }
}

/// Attaches `source` to layer 0's origin and panics when it does not load.
fn attach(runtime: &mut Runtime, mock: &mut Mock, source: &str) -> ScriptId {
    runtime
        .attach(source, binding(0, Field::Origin, Datum::Vec3([0.0; 3])), mock)
        .unwrap_or_else(|error| panic!("{error}"))
}

/// Runs `expression` in a script of its own and returns what it evaluates to.
fn eval(mock: &mut Mock, imports: &str, expression: &str) -> Datum {
    let mut runtime = Runtime::new().unwrap();
    let script = attach(
        &mut runtime,
        mock,
        &format!("{imports}\nexport function update(value) {{ return {expression}; }}"),
    );
    let value = runtime.update(script, Datum::Number(0.0), mock);
    assert_eq!(runtime.failure(script), None, "{expression}");
    value.unwrap_or_else(|| panic!("{expression} returned nothing"))
}

fn close(value: &Datum, expected: &[f64]) -> bool {
    value.floats().is_some_and(|floats| {
        floats.len() == expected.len() && floats.iter().zip(expected).all(|(a, b)| (a - b).abs() < 1e-9)
    })
}

#[test]
fn update_returns_the_new_value_in_the_type_the_script_chose() {
    let mut mock = Mock::with_layers(&["a"]);
    let mut runtime = Runtime::new().unwrap();
    let moves = attach(
        &mut runtime,
        &mut mock,
        "'use strict';\nexport function update(value) { value.x += 1; return value.multiply(2); }",
    );
    let text = attach(&mut runtime, &mut mock, "export function update(value) { return value + '!'; }");
    let flag = attach(&mut runtime, &mut mock, "export function update(value) { return !value; }");
    let silent = attach(&mut runtime, &mut mock, "export function update(value) { value.x = 5; }");
    let events_only = attach(&mut runtime, &mut mock, "export function cursorClick(event) {}");

    assert_eq!(
        runtime.update(moves, Datum::Vec3([1.0, 2.0, 3.0]), &mut mock),
        Some(Datum::Vec3([4.0, 4.0, 6.0]))
    );
    assert_eq!(runtime.update(text, Datum::Text("hi".into()), &mut mock), Some(Datum::Text("hi!".into())));
    assert_eq!(runtime.update(flag, Datum::Bool(true), &mut mock), Some(Datum::Bool(false)));
    assert_eq!(runtime.update(silent, Datum::Vec3([0.0; 3]), &mut mock), None);
    assert!(runtime.has_update(moves) && !runtime.has_update(events_only));
    assert_eq!(runtime.update(events_only, Datum::Number(1.0), &mut mock), None);
    assert!(mock.logs.is_empty(), "{:?}", mock.logs);
}

#[test]
fn init_gets_the_attached_value_and_its_return_is_reported() {
    let mut mock = Mock::with_layers(&["a"]);
    let mut runtime = Runtime::new().unwrap();
    let source = "let start;\nexport function init(value) { start = value.x; return new Vec3(start + 1, 0, 0); }\nexport function update() { return start; }";
    let script =
        runtime.attach(source, binding(0, Field::Origin, Datum::Vec3([41.0, 0.0, 0.0])), &mut mock).unwrap();
    let quiet = attach(&mut runtime, &mut mock, "export function init(value) {}");
    assert_eq!(runtime.init(&mut mock), [(script, Datum::Vec3([42.0, 0.0, 0.0]))]);
    assert_eq!(runtime.update(script, Datum::Null, &mut mock), Some(Datum::Number(41.0)));
    assert_eq!(runtime.failure(quiet), None);
}

#[test]
fn script_properties_take_the_bindings_values_and_follow_user_properties() {
    let mut mock = Mock::with_layers(&["a"]);
    mock.user.insert("speed".into(), Value::from(7.0));
    mock.user.insert("tint".into(), Value::from("1 0.5 0"));
    let properties = wepaper_json::parse(
        r#"{"plain": 3, "linked": {"user": "speed", "value": 1}, "nested": {"user": "missing", "value": {"user": "speed", "value": 2}},
            "fallback": {"user": "missing", "value": 9}, "color": {"user": "tint", "value": "0 0 0"}, "label": "from the scene",
            "flag": true, "undeclared": 5}"#,
    )
    .unwrap();
    let source = "export var scriptProperties = createScriptProperties()
        .addSlider({name: 'plain', label: 'Plain', value: 1, min: 0, max: 10, integer: false})
        .addSlider({name: 'linked', value: 0}).addSlider({name: 'nested', value: 0}).addSlider({name: 'fallback', value: 0})
        .addColor({name: 'color', value: new Vec3(1, 1, 1)}).addText({name: 'label', value: 'default'})
        .addCheckbox({name: 'flag', value: false}).addCheckbox({name: 'untouched', value: true})
        .addCombo({name: 'mode', options: [{label: 'A', value: 'a'}, {label: 'B', value: 'b'}]})
        .addFile({name: 'file', value: ''}).addDirectory({name: 'folder', value: ''})
        .finish();
    const p = scriptProperties;
    export function update() {
        return [p.plain, p.linked, p.nested, p.fallback, p.color instanceof Vec3, p.color.y, p.label, p.flag, p.untouched, p.mode, 'undeclared' in p].join('|');
    }";
    let mut runtime = Runtime::new().unwrap();
    let script = runtime
        .attach(
            source,
            Binding { properties: &properties, ..binding(0, Field::Text, Datum::Text(String::new())) },
            &mut mock,
        )
        .unwrap();
    assert_eq!(
        runtime.update(script, Datum::Null, &mut mock),
        Some(Datum::Text("3|7|7|9|true|0.5|from the scene|true|true|a|false".into()))
    );

    mock.user.insert("speed".into(), Value::from(8.0));
    runtime.apply_user_properties(&[("speed".into(), Value::from(8.0))], &mut mock);
    assert_eq!(
        runtime.update(script, Datum::Null, &mut mock),
        Some(Datum::Text("3|8|8|9|true|0.5|from the scene|true|true|a|false".into()))
    );
}

#[test]
fn a_script_that_throws_is_disabled_alone_and_reported_once() {
    let mut mock = Mock::with_layers(&["Clock", "Sun"]);
    let mut runtime = Runtime::new().unwrap();
    let good = attach(&mut runtime, &mut mock, "let n = 0;\nexport function update() { return ++n; }");
    let bad = runtime
        .attach(
            "let n = 0;\nexport function update(value) {\n  if (++n > 1) missing.call();\n  return n;\n}",
            binding(1, Field::Alpha, Datum::Number(1.0)),
            &mut mock,
        )
        .unwrap();

    assert_eq!(runtime.update(bad, Datum::Number(1.0), &mut mock), Some(Datum::Number(1.0)));
    for frame in 1..=3 {
        assert_eq!(runtime.update(good, Datum::Null, &mut mock), Some(Datum::Number(f64::from(frame))));
        assert_eq!(runtime.update(bad, Datum::Number(1.0), &mut mock), None);
    }
    assert!(!runtime.has_update(bad) && runtime.has_update(good));
    let failure = runtime.failure(bad).unwrap();
    assert_eq!(failure.kind, ErrorKind::Threw);
    assert!(failure.message.contains("missing"), "{failure}");
    assert!(failure.at.as_deref().is_some_and(|at| at.contains(":3:")), "{failure:?}");
    let errors = mock.logged(Level::Error);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("\"Sun\" (alpha)") && errors[0].contains("missing"), "{errors:?}");
}

#[test]
fn a_script_that_does_not_compile_is_an_error_and_the_others_run() {
    let mut mock = Mock::with_layers(&["Clock"]);
    let mut runtime = Runtime::new().unwrap();
    let good = attach(&mut runtime, &mut mock, "export function update() { return 1; }");
    let broken = runtime
        .attach("export function update( { return 1; }", binding(0, Field::Scale, Datum::Null), &mut mock)
        .unwrap_err();
    let throws = runtime
        .attach("null.x;\nexport function update() {}", binding(0, Field::Scale, Datum::Null), &mut mock)
        .unwrap_err();
    let imports = runtime
        .attach("import * as m from 'nowhere';", binding(0, Field::Scale, Datum::Null), &mut mock)
        .unwrap_err();

    assert_eq!(
        (broken.kind, throws.kind, imports.kind),
        (ErrorKind::Compile, ErrorKind::Threw, ErrorKind::Compile)
    );
    assert!(imports.message.contains("nowhere"), "{imports:?}");
    assert_eq!(runtime.update(good, Datum::Null, &mut mock), Some(Datum::Number(1.0)));
    let errors = mock.logged(Level::Error);
    assert_eq!(errors.len(), 3, "{errors:?}");
    assert!(errors.iter().all(|error| error.contains("\"Clock\" (scale)")), "{errors:?}");
}

#[test]
fn an_endless_loop_is_stopped_and_only_its_script_is_disabled() {
    let mut mock = Mock::with_layers(&["a"]);
    let limits = Limits { call_budget: Duration::from_millis(30), ..Limits::default() };
    let mut runtime = Runtime::with_limits(limits).unwrap();
    let good = attach(&mut runtime, &mut mock, "export function update() { return 1; }");
    let spins = attach(
        &mut runtime,
        &mut mock,
        "export function update() { for (;;) { try { while (true) {} } catch (e) {} } }",
    );
    let spins_loading =
        runtime.attach("while (true) {}", binding(0, Field::Scale, Datum::Null), &mut mock).unwrap_err();

    let started = std::time::Instant::now();
    assert_eq!(runtime.update(spins, Datum::Null, &mut mock), None);
    assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());
    assert_eq!(runtime.failure(spins).map(|failure| failure.kind), Some(ErrorKind::TimedOut));
    assert_eq!(spins_loading.kind, ErrorKind::TimedOut);
    assert_eq!(runtime.update(good, Datum::Null, &mut mock), Some(Datum::Number(1.0)));
    assert_eq!(runtime.update(spins, Datum::Null, &mut mock), None);
    assert_eq!(mock.logged(Level::Error).len(), 2);
}

#[test]
fn a_script_that_hoards_memory_is_disabled() {
    let mut mock = Mock::with_layers(&["a"]);
    let limits = Limits { memory: 16 << 20, call_budget: Duration::from_secs(5) };
    let mut runtime = Runtime::with_limits(limits).unwrap();
    let good = attach(&mut runtime, &mut mock, "export function update() { return 1; }");
    let hoards = attach(
        &mut runtime,
        &mut mock,
        "const kept = [];\nexport function update() { for (;;) kept.push(new Array(1024).fill(1)); }",
    );
    assert_eq!(runtime.update(hoards, Datum::Null, &mut mock), None);
    assert!(runtime.failure(hoards).is_some());
    assert_eq!(runtime.update(good, Datum::Null, &mut mock), Some(Datum::Number(1.0)));
}

#[test]
fn this_layer_reads_and_writes_reach_the_host() {
    let mut mock = Mock::with_layers(&["Sun", "Moon"]);
    mock.put(0, Field::Origin, Datum::Vec3([10.0, 20.0, 30.0]));
    mock.put(0, Field::Alpha, Datum::Number(0.25));
    mock.put(0, Field::Visible, Datum::Bool(false));
    mock.put(0, Field::Size, Datum::Vec2([64.0, 32.0]));
    mock.put(0, Field::Named("pointsize"), Datum::Number(24.0));
    mock.put(0, Field::Parent, Datum::Layer(LayerId(1)));
    let mut runtime = Runtime::new().unwrap();
    let source = "export function update(value) {
        const origin = thisLayer.origin;
        const read = [origin instanceof Vec3, origin.y, thisLayer.alpha, thisLayer.visible, thisLayer.name, thisLayer.size.x, thisLayer.pointsize,
            thisLayer.getParent().name, thisLayer.getParent().getChildren()[0] === thisLayer, thisLayer.scale.x, thisLayer.text, thisObject === thisLayer];
        thisLayer.origin = origin.add(new Vec3(1, 1, 1));
        thisLayer.alpha = 0.5;
        thisLayer.visible = true;
        thisLayer.scale = 2;
        thisLayer.angles = '0 0 90';
        thisLayer.color = {x: 1, y: 0, z: 0};
        thisLayer.text = 12;
        thisLayer.pointsize = 30;
        thisLayer.horizontalalign = 'left';
        thisLayer.parallaxDepth = new Vec2(0.5, 0.25);
        thisLayer.alpha = 0 / 0;
        thisLayer.custom = 'kept on the object';
        return read.join('|');
    }";
    let script = attach(&mut runtime, &mut mock, source);
    assert_eq!(
        runtime.update(script, Datum::Null, &mut mock),
        Some(Datum::Text("true|20|0.25|false|Sun|64|24|Moon|true|0||true".into()))
    );
    assert_eq!(mock.field(0, Field::Origin), Datum::Vec3([11.0, 21.0, 31.0]));
    assert_eq!(mock.field(0, Field::Alpha), Datum::Number(0.5), "a NaN must not reach the scene");
    assert_eq!(mock.field(0, Field::Visible), Datum::Bool(true));
    assert_eq!(mock.field(0, Field::Scale), Datum::Vec3([2.0, 2.0, 2.0]));
    assert_eq!(mock.field(0, Field::Angles), Datum::Vec3([0.0, 0.0, 90.0]));
    assert_eq!(mock.field(0, Field::Color), Datum::Vec3([1.0, 0.0, 0.0]));
    assert_eq!(mock.field(0, Field::Text), Datum::Text("12".into()));
    assert_eq!(mock.field(0, Field::Named("pointsize")), Datum::Number(30.0));
    assert_eq!(mock.field(0, Field::Named("horizontalalign")), Datum::Text("left".into()));
    assert_eq!(mock.field(0, Field::ParallaxDepth), Datum::Vec2([0.5, 0.25]));
    assert_eq!(mock.field(0, Field::Named("custom")), Datum::Null);
}

#[test]
fn this_object_is_the_part_of_the_layer_the_script_is_bound_to() {
    let mut mock = Mock::with_layers(&["Picture"]);
    mock.put(0, Field::EffectName(0), Datum::Text("blur".into()));
    mock.put(0, Field::EffectName(1), Datum::Text("tint".into()));
    mock.put(0, Field::EffectVisible(1), Datum::Bool(true));
    mock.put(0, Field::EffectConstant(1, "stage"), Datum::Number(3.0));
    mock.put(0, Field::EffectConstant(1, "Tint Color"), Datum::Vec3([1.0, 0.0, 0.0]));
    mock.put(0, Field::Instance("rate"), Datum::Number(2.0));
    let mut runtime = Runtime::new().unwrap();
    let on_effect = "export function update(value) {
        const read = [thisObject !== thisLayer, thisObject.stage, thisObject['Tint Color'] instanceof Vec3, thisObject.name, thisObject.visible,
            thisLayer.getEffectCount(), thisLayer.getEffect('blur').name, thisLayer.getEffect(1).stage, thisLayer.getEffect('none'), thisObject.nothing];
        thisObject.stage = value + 1;
        thisObject['Tint Color'] = new Vec3(0, 1, 0);
        thisLayer.getEffect(1).visible = false;
        thisObject.setMaterialProperty('strength', 0.5);
        return read.join('|');
    }";
    let effect = runtime
        .attach(on_effect, binding(0, Field::EffectConstant(1, "stage"), Datum::Number(3.0)), &mut mock)
        .unwrap();
    let on_instance = "export function update(value) { thisObject.size = thisObject.rate * 2; thisLayer.instance.count = 3; return thisObject.rate; }";
    let instance = runtime
        .attach(on_instance, binding(0, Field::Instance("size"), Datum::Number(1.0)), &mut mock)
        .unwrap();

    assert_eq!(
        runtime.update(effect, Datum::Number(3.0), &mut mock),
        Some(Datum::Text("true|3|true|tint|true|2|blur|3||".into()))
    );
    assert_eq!(mock.field(0, Field::EffectConstant(1, "stage")), Datum::Number(4.0));
    assert_eq!(mock.field(0, Field::EffectConstant(1, "Tint Color")), Datum::Vec3([0.0, 1.0, 0.0]));
    assert_eq!(mock.field(0, Field::EffectConstant(1, "strength")), Datum::Number(0.5));
    assert_eq!(mock.field(0, Field::EffectVisible(1)), Datum::Bool(false));
    assert_eq!(runtime.update(instance, Datum::Number(1.0), &mut mock), Some(Datum::Number(2.0)));
    assert_eq!(mock.field(0, Field::Instance("size")), Datum::Number(4.0));
    assert_eq!(mock.field(0, Field::Instance("count")), Datum::Number(3.0));
    assert_eq!(runtime.failure(effect).or(runtime.failure(instance)), None);
}

#[test]
fn this_scene_finds_layers_by_name_index_and_id() {
    let mut mock = Mock::with_layers(&["Sky", "Sun", "Sea"]);
    mock.put(1, Field::Origin, Datum::Vec3([5.0, 6.0, 7.0]));
    let source = "export function update() {
        const sun = thisScene.getLayer('Sun');
        sun.origin = sun.origin.add(1);
        return [sun.name, thisScene.getLayer(2).name, thisScene.getLayerByID(100).name, thisScene.getLayerByID('102').name,
            thisScene.getLayer('Nothing'), thisScene.getLayer(7), thisScene.getLayerByID(5), thisScene.getLayerCount(),
            thisScene.enumerateLayers().map((layer) => layer.name).join(','), thisScene.getLayerIndex(sun), thisScene.getLayerIndex('Sea'),
            thisScene.getLayerIndex('Nothing'), thisScene.getLayer('Sun') === sun, thisScene.getLayer('Sky') === thisLayer].join('|');
    }";
    let mut runtime = Runtime::new().unwrap();
    let script = attach(&mut runtime, &mut mock, source);
    assert_eq!(
        runtime.update(script, Datum::Null, &mut mock),
        Some(Datum::Text("Sun|Sea|Sky|Sea||||3|Sky,Sun,Sea|1|2|-1|true|true".into()))
    );
    assert_eq!(mock.field(1, Field::Origin), Datum::Vec3([6.0, 7.0, 8.0]));
}

#[test]
fn shared_is_common_to_scripts_and_this_layer_follows_the_running_one() {
    let mut mock = Mock::with_layers(&["First", "Second"]);
    let mut runtime = Runtime::new().unwrap();
    let first = attach(
        &mut runtime,
        &mut mock,
        "shared.count = 1;\nshared.who = () => thisLayer.name;\nexport function update() { return shared.count; }",
    );
    let second = runtime
        .attach(
            "export function update() { shared.count += 1; return shared.who(); }",
            binding(1, Field::Alpha, Datum::Number(1.0)),
            &mut mock,
        )
        .unwrap();
    // As in Wallpaper Engine, a function shared with another script sees the caller's layer.
    assert_eq!(runtime.update(second, Datum::Null, &mut mock), Some(Datum::Text("Second".into())));
    assert_eq!(runtime.update(first, Datum::Null, &mut mock), Some(Datum::Number(2.0)));

    let mut other_scene = Runtime::new().unwrap();
    let alone =
        attach(&mut other_scene, &mut mock, "export function update() { return typeof shared.count; }");
    assert_eq!(other_scene.update(alone, Datum::Null, &mut mock), Some(Datum::Text("undefined".into())));
}

#[test]
fn timers_fire_on_the_scenes_clock() {
    let mut mock = Mock::with_layers(&["Timed"]);
    let source = "const fired = [];
        const stop = engine.setInterval(() => fired.push('tick'), 100);
        engine.setTimeout(() => fired.push('once ' + thisLayer.name), 250);
        const never = setTimeout(() => fired.push('never'), 50);
        clearTimeout(never);
        engine.setTimeout(() => { throw new Error('in a timer'); }, 10);
        export function update(value) { if (value) stop(); return fired.join(','); }";
    let mut runtime = Runtime::new().unwrap();
    let script = attach(&mut runtime, &mut mock, source);
    let at = |time: f64, mock: &mut Mock, runtime: &mut Runtime| {
        mock.environment.time = time;
        runtime.begin_frame(mock);
        runtime.update(script, Datum::Bool(false), mock)
    };
    assert_eq!(at(0.05, &mut mock, &mut runtime), Some(Datum::Text(String::new())));
    assert_eq!(at(0.10, &mut mock, &mut runtime), Some(Datum::Text("tick".into())));
    assert_eq!(at(0.15, &mut mock, &mut runtime), Some(Datum::Text("tick".into())));
    assert_eq!(at(0.30, &mut mock, &mut runtime), Some(Datum::Text("tick,tick,once Timed".into())));
    assert_eq!(
        runtime.update(script, Datum::Bool(true), &mut mock),
        Some(Datum::Text("tick,tick,once Timed".into()))
    );
    assert_eq!(at(9.0, &mut mock, &mut runtime), Some(Datum::Text("tick,tick,once Timed".into())));
    // The timer that threw is gone and named; its script goes on.
    assert_eq!(runtime.failure(script), None);
    let errors = mock.logged(Level::Error);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("\"Timed\"") && errors[0].contains("in a timer"), "{errors:?}");
}

#[test]
fn cursor_events_reach_the_handlers_of_the_layer_under_the_cursor() {
    let mut mock = Mock::with_layers(&["Button", "Other"]);
    mock.put(0, Field::Origin, Datum::Vec3([100.0, 100.0, 0.0]));
    mock.put(0, Field::Scale, Datum::Vec3([1.0, 1.0, 1.0]));
    mock.environment.cursor_scene = [110.0, 95.0];
    mock.environment.cursor_screen = [220.0, 190.0];
    mock.environment.cursor_down = true;
    let source = "const seen = [];
        const at = (event) => event.worldPosition.x + ';' + event.worldPosition.y + ' local ' + event.localPosition.x + ';' + event.localPosition.y;
        export function cursorEnter(event) { seen.push('enter'); }
        export function cursorLeave(event) { seen.push('leave'); }
        export function cursorMove(event) { seen.push('move ' + at(event)); }
        export function cursorDown(event) { seen.push('down ' + input.cursorLeftDown + ' ' + input.cursorScreenPosition.x); }
        export function cursorUp(event) { seen.push('up'); }
        export function cursorClick(event) { seen.push('click ' + (input.cursorWorldPosition instanceof Vec3)); }
        export function update() { return seen.join(','); }";
    let mut runtime = Runtime::new().unwrap();
    let button = attach(&mut runtime, &mut mock, source);
    let other = runtime
        .attach(
            "let n = 0;\nexport function cursorClick() { n++; }\nexport function update() { return n; }",
            binding(1, Field::Alpha, Datum::Null),
            &mut mock,
        )
        .unwrap();

    assert!(runtime.handles(button, Handler::CursorEnter) && !runtime.handles(other, Handler::CursorEnter));
    for event in
        [CursorEvent::Enter, CursorEvent::Down, CursorEvent::Up, CursorEvent::Click, CursorEvent::Leave]
    {
        runtime.cursor(event, Some(LayerId(0)), &mut mock);
    }
    runtime.cursor(CursorEvent::Move, None, &mut mock);
    assert_eq!(
        runtime.update(button, Datum::Null, &mut mock),
        Some(Datum::Text("enter,down true 220,up,click true,leave,move 110;95 local 10;-5".into()))
    );
    assert_eq!(runtime.update(other, Datum::Null, &mut mock), Some(Datum::Number(0.0)));
    runtime.cursor(CursorEvent::Click, None, &mut mock);
    assert_eq!(runtime.update(other, Datum::Null, &mut mock), Some(Datum::Number(1.0)));
}

#[test]
fn vectors_compute_as_documented() {
    let mut mock = Mock::with_layers(&["a"]);
    let vec3 = |mock: &mut Mock, expression: &str, expected: [f64; 3]| {
        let value = eval(mock, "", expression);
        assert!(close(&value, &expected), "{expression} = {value:?}, not {expected:?}");
    };
    vec3(&mut mock, "new Vec3(1, 2, 3).add(new Vec3(1, 1, 1)).multiply(2)", [4.0, 6.0, 8.0]);
    vec3(&mut mock, "new Vec3(4, 6, 8).subtract(1).divide(new Vec3(3, 5, 7))", [1.0, 1.0, 1.0]);
    vec3(&mut mock, "new Vec3(1, 0, 0).cross(new Vec3(0, 1, 0))", [0.0, 0.0, 1.0]);
    vec3(&mut mock, "new Vec3(3, 0, 4).normalize()", [0.6, 0.0, 0.8]);
    vec3(&mut mock, "new Vec3(0, 0, 0).mix(new Vec3(10, 20, 30), 0.5)", [5.0, 10.0, 15.0]);
    vec3(&mut mock, "new Vec3('1.5 2.5 3.5')", [1.5, 2.5, 3.5]);
    vec3(&mut mock, "new Vec3(2)", [2.0, 2.0, 2.0]);
    vec3(&mut mock, "new Vec3(1, 2)", [1.0, 2.0, 0.0]);
    vec3(&mut mock, "new Vec3(new Vec2(7, 8))", [7.0, 8.0, 0.0]);
    vec3(&mut mock, "new Vec3(1, 2, 3).add(new Vec2(10, 10))", [11.0, 12.0, 3.0]);
    vec3(&mut mock, "new Vec3(-1.5, 2.5, 0).abs().floor()", [1.0, 2.0, 0.0]);
    vec3(&mut mock, "new Vec3(5, -5, 0.5).clamp(0, 1)", [1.0, 0.0, 0.5]);
    vec3(&mut mock, "new Vec3(1, 2, 3).copy().negate().min(new Vec3(0, -5, 0)).max(-4)", [-1.0, -4.0, -3.0]);
    vec3(
        &mut mock,
        "new Mat4().translate(new Vec3(1, 2, 3)).scale(2).transformPoint(new Vec3(1, 1, 1))",
        [3.0, 4.0, 5.0],
    );
    vec3(&mut mock, "Mat4.fromEuler(0, 0, 90).transformDirection(new Vec3(1, 0, 0))", [0.0, 1.0, 0.0]);
    vec3(
        &mut mock,
        "Mat4.compose(new Vec3(5, 0, 0), new Vec3(0, 0, 90), new Vec3(2, 2, 2)).inverse().transformPoint(new Vec3(5, 2, 0))",
        [1.0, 0.0, 0.0],
    );
    vec3(&mut mock, "Mat4.fromEuler(10, 20, 30).extractEuler()", [10.0, 20.0, 30.0]);

    assert_eq!(
        eval(&mut mock, "", "new Vec2(3, 4).length() + new Vec3(1, 2, 3).dot(new Vec3(1, 1, 1))"),
        Datum::Number(11.0)
    );
    assert_eq!(
        eval(&mut mock, "", "new Vec2(1, 2).add(new Vec2(3, 4)).multiply(new Vec2(2, 0.5))"),
        Datum::Vec2([8.0, 3.0])
    );
    assert_eq!(eval(&mut mock, "", "new Vec4(1, 2, 3, 4).add(1)"), Datum::Vec4([2.0, 3.0, 4.0, 5.0]));
    assert_eq!(
        eval(&mut mock, "", "new Vec3(1, 2, 3).toString() + ' / ' + new Vec2(1, 2)"),
        Datum::Text("1 2 3 / 1 2".into())
    );
    assert_eq!(
        eval(
            &mut mock,
            "",
            "new Vec3(1, 2, 3).equals(new Vec3(1, 2, 3.000001)) && !new Vec3(1, 2, 3).equals(new Vec2(1, 2))"
        ),
        Datum::Bool(true)
    );
    assert!(close(&eval(&mut mock, "", "new Vec2(1, 0).rotate(90).angle()"), &[90.0]));
    assert!(close(
        &eval(
            &mut mock,
            "",
            "new Mat4().rotate(45, new Vec3(0, 0, 1)).multiply(new Mat4().rotate(-45, new Vec3(0, 0, 1))).equals(Mat4.identity()) ? 1 : 0"
        ),
        &[1.0]
    ));
    assert!(close(
        &eval(
            &mut mock,
            "",
            "Mat3.fromRotation(90).transformPoint(new Vec2(1, 0)).y + Mat3.fromScale(3).determinant()"
        ),
        &[10.0]
    ));
    assert_eq!(eval(&mut mock, "", "MediaPlaybackEvent.PLAYBACK_PAUSED"), Datum::Number(2.0));
}

#[test]
fn the_modules_of_the_reference_import_and_compute() {
    let mut mock = Mock::with_layers(&["a"]);
    let math = "import * as WEMath from 'WEMath';";
    assert_eq!(
        eval(&mut mock, math, "WEMath.mix(10, 20, 0.25) + WEMath.smoothStep(0, 10, 5)"),
        Datum::Number(13.0)
    );
    assert!(close(
        &eval(&mut mock, math, "WEMath.deg2rad * 180 + WEMath.rad2deg * Math.PI"),
        &[std::f64::consts::PI + 180.0]
    ));
    let color = "import * as WEColor from 'WEColor';";
    assert!(close(&eval(&mut mock, color, "WEColor.rgb2hsv(new Vec3(0, 1, 0))"), &[1.0 / 3.0, 1.0, 1.0]));
    assert!(close(&eval(&mut mock, color, "WEColor.hsv2rgb(new Vec3(2 / 3, 1, 0.5))"), &[0.0, 0.0, 0.5]));
    assert!(close(
        &eval(&mut mock, color, "WEColor.hsv2rgb(WEColor.rgb2hsv(new Vec3(0.2, 0.6, 0.4)))"),
        &[0.2, 0.6, 0.4]
    ));
    assert!(close(
        &eval(
            &mut mock,
            color,
            "WEColor.expandColor(WEColor.normalizeColor(new Vec3(255, 51, 0))).subtract(new Vec3(0, 51, 0))"
        ),
        &[255.0, 0.0, 0.0]
    ));
    let vector = "import * as WEVector from 'WEVector';\nimport { mix } from 'WEMath';";
    assert!(close(&eval(&mut mock, vector, "WEVector.angleVector2(90)"), &[0.0, 1.0]));
    assert!(close(
        &eval(&mut mock, vector, "WEVector.vectorAngle2(new Vec2(-1, 0)) + mix(0, 1, 1)"),
        &[181.0]
    ));
}

#[test]
fn local_storage_keeps_values_in_the_host() {
    let mut mock = Mock::with_layers(&["a"]);
    let stored = eval(
        &mut mock,
        "",
        "(localStorage.set('n', 42), localStorage.set('o', {a: [1, 2], v: new Vec3(1, 2, 3)}), localStorage.set('g', 'everywhere', localStorage.LOCATION_GLOBAL),
          [localStorage.get('n'), localStorage.get('o').a[1], localStorage.get('o').v, localStorage.get('g'), localStorage.get('g', 'global'), localStorage.get('nothing')].join('|'))",
    );
    assert_eq!(stored, Datum::Text("42|2|1 2 3||everywhere|".into()));
    assert_eq!(mock.storage.get("screen:n").map(String::as_str), Some("42"));
    assert_eq!(mock.storage.get("global:g").map(String::as_str), Some("\"everywhere\""));

    // Another runtime, as after a restart: the host's storage is all there is.
    let later = eval(
        &mut mock,
        "",
        "[localStorage.get('n'), localStorage.delete('n'), localStorage.delete('n'), localStorage.get('n')].join('|')",
    );
    assert_eq!(later, Datum::Text("42|true|false|".into()));
    eval(&mut mock, "", "(localStorage.clear(), localStorage.get('o') === undefined)");
    let mut left: Vec<&str> = mock.storage.keys().map(String::as_str).collect();
    left.sort_unstable();
    assert_eq!(left, ["global#keys", "global:g"]);
}

#[test]
fn user_property_changes_reach_apply_user_properties() {
    let mut mock = Mock::with_layers(&["a"]);
    mock.user.insert("speed".into(), Value::from(2.0));
    mock.user.insert("tint".into(), Value::from("0.5 0.25 1"));
    mock.user.insert("title".into(), Value::from("hello"));
    let source = "const seen = [];
        export function applyUserProperties(changed) {
            seen.push(Object.keys(changed).join('+'));
            if (changed.hasOwnProperty('tint')) seen.push(changed.tint instanceof Vec3 ? 'vec ' + changed.tint.y : 'not a vector');
            if (changed.hasOwnProperty('speed')) seen.push(changed.speed === engine.userProperties.speed);
        }
        export function update() {
            const all = engine.userProperties;
            return seen.concat([all.speed, all.tint instanceof Vec3, all.title, all.nothing, 'speed' in all, all.hasOwnProperty('nothing')]).join('|');
        }";
    let mut runtime = Runtime::new().unwrap();
    let script = attach(&mut runtime, &mut mock, source);
    let deaf = attach(&mut runtime, &mut mock, "export function update() { return 1; }");
    mock.user.insert("speed".into(), Value::from(3.0));
    runtime.apply_user_properties(
        &[("speed".into(), Value::from(3.0)), ("tint".into(), Value::from("0.5 0.25 1"))],
        &mut mock,
    );
    runtime.apply_user_properties(&[("title".into(), Value::from("bye"))], &mut mock);
    assert_eq!(
        runtime.update(script, Datum::Null, &mut mock),
        Some(Datum::Text("speed+tint|vec 0.25|true|title|3|true|hello||true|false".into()))
    );
    assert_eq!(runtime.failure(deaf), None);
}

#[test]
fn engine_tells_the_environment_of_the_frame() {
    let mut mock = Mock::with_layers(&["a"]);
    mock.environment = Environment {
        time: 12.5,
        frame_time: 0.016,
        time_of_day: 0.75,
        canvas_size: [3840.0, 2160.0],
        screen_size: [1080.0, 1920.0],
        ..Default::default()
    };
    mock.environment.spectrum[0] = [0.5; 64];
    mock.environment.spectrum[1][..4].copy_from_slice(&[1.0, 0.0, 1.0, 0.0]);
    let source = "const top = engine.canvasSize.x;
        const audio = engine.registerAudioBuffers(engine.AUDIO_RESOLUTION_32);
        const resized = [];
        export function resizeScreen(size) { resized.push(size.x + 'x' + size.y); }
        export function update() {
            return [top, engine.runtime, engine.frametime, engine.timeOfDay, engine.screenResolution.y, engine.isPortrait(), engine.isLandscape(),
                engine.isDesktopDevice(), engine.isMobileDevice(), engine.isWallpaper(), engine.isScreensaver(), engine.isRunningInEditor(),
                audio.left.length, audio.left[3], audio.right[0], audio.right[1], audio.average[0], resized.join(',')].join('|');
        }";
    let mut runtime = Runtime::new().unwrap();
    let script = attach(&mut runtime, &mut mock, source);
    runtime.begin_frame(&mut mock);
    assert_eq!(
        runtime.update(script, Datum::Null, &mut mock),
        Some(Datum::Text(
            "3840|12.5|0.016|0.75|1920|true|false|true|false|true|false|false|32|0.5|0.5|0.5|0.5|".into()
        ))
    );
    mock.environment.screen_size = [1920.0, 1080.0];
    runtime.begin_frame(&mut mock);
    let Some(Datum::Text(after)) = runtime.update(script, Datum::Null, &mut mock) else { panic!("no value") };
    assert!(
        after.ends_with("|false|true|true|false|true|false|false|32|0.5|0.5|0.5|0.5|1920x1080"),
        "{after}"
    );
}

#[test]
fn media_and_animation_events_reach_their_handlers() {
    let mut mock = Mock::with_layers(&["Player"]);
    let source = "const seen = [];
        export function mediaStatusChanged(event) { seen.push('status ' + event.enabled); }
        export function mediaPlaybackChanged(event) { seen.push('playback ' + (event.state === MediaPlaybackEvent.PLAYBACK_PLAYING)); }
        export function mediaPropertiesChanged(event) { seen.push(event.title + ' by ' + event.artist + ' on ' + event.albumTitle); }
        export function mediaThumbnailChanged(event) { seen.push('cover ' + event.hasThumbnail + ' ' + (event.primaryColor instanceof Vec3) + ' ' + event.textColor.z); }
        export function mediaTimelineChanged(event) { seen.push(event.position + '/' + event.duration); }
        export function animationEvent(event, value) { seen.push(event.name + '@' + event.frame); return value + 1; }
        export function applyGeneralSettings(changed) { seen.push('language ' + changed.language); }
        export function destroy() { thisLayer.text = seen.join(','); }";
    let mut runtime = Runtime::new().unwrap();
    let script = attach(&mut runtime, &mut mock, source);
    runtime.media(MediaEvent::Status { enabled: true }, &mut mock);
    runtime.media(MediaEvent::Playback { state: 1 }, &mut mock);
    let track = MediaEvent::Properties {
        title: "Song",
        artist: "Band",
        sub_title: "",
        album_title: "Album",
        album_artist: "",
        genres: "",
        content_type: "audio",
    };
    runtime.media(track, &mut mock);
    let cover = MediaEvent::Thumbnail {
        has_thumbnail: true,
        primary: [1.0, 0.0, 0.0],
        secondary: [0.0; 3],
        tertiary: [0.0; 3],
        text: [0.0, 0.0, 0.5],
        high_contrast: [1.0; 3],
    };
    runtime.media(cover, &mut mock);
    runtime.media(MediaEvent::Timeline { position: 3.0, duration: 180.0 }, &mut mock);
    assert_eq!(
        runtime.animation_event(script, "blink", 12.0, Datum::Number(1.0), &mut mock),
        Some(Datum::Number(2.0))
    );
    runtime.apply_general_settings(&[("language".into(), Value::from("uk-ua"))], &mut mock);
    runtime.destroy(&mut mock);
    assert_eq!(
        mock.field(0, Field::Text),
        Datum::Text("status true,playback true,Song by Band on Album,cover true true 0.5,3/180,blink@12,language uk-ua".into())
    );
}

#[test]
fn what_is_not_implemented_is_a_harmless_stub_reported_once() {
    let mut mock = Mock::with_layers(&["a"]);
    let source = "const asset = engine.registerAsset('models/thing.json');
        export function update() {
            const made = thisScene.createLayer(asset);
            made.origin = new Vec3(1, 2, 3);
            const animation = thisObject.getAnimation();
            animation.play();
            animation.setFrame(4);
            thisLayer.getAnimation('named').stop();
            thisLayer.getTextureAnimation().pause();
            thisLayer.getVideoTexture().setCurrentTime(2);
            thisLayer.getAnimationLayer(0).rate = 2;
            thisLayer.play();
            thisLayer.emitParticles(5);
            thisLayer.setParent(made);
            thisLayer.applyBonePhysicsImpulse(0, new Vec3(1, 0, 0), new Vec3(0, 0, 0));
            thisScene.sortLayer(thisLayer, 3);
            thisScene.bloom = true;
            return [made.origin.x, made.name, animation.isPlaying(), animation.getFrame(), animation.frameCount, thisLayer.getBoneCount(),
                thisLayer.getBoneTransform(0) instanceof Mat4, thisLayer.getAttachmentOrigin('hand') instanceof Vec3, thisScene.sortLayer(thisLayer, 3),
                thisScene.destroyLayer(made), engine.openUserShortcut('key'), thisScene.bloom, thisScene.clearcolor instanceof Vec3,
                thisScene.getCameraTransforms().zoom, thisLayer.getTransformMatrix().translation().x, String(asset)].join('|');
        }";
    let mut runtime = Runtime::new().unwrap();
    let script = attach(&mut runtime, &mut mock, source);
    for _ in 0..3 {
        let value = runtime.update(script, Datum::Null, &mut mock);
        assert_eq!(
            value,
            Some(Datum::Text(
                "0||true|4|0|0|true|true|false|false|false|false|true|1|0|models/thing.json".into()
            ))
        );
    }
    assert_eq!(runtime.failure(script), None);
    let debug = mock.logged(Level::Debug);
    for name in [
        "thisScene.createLayer",
        "animation.play",
        "textureAnimation.pause",
        "thisLayer.emitParticles",
        "thisScene.sortLayer",
        "engine.openUserShortcut",
    ] {
        assert_eq!(
            debug.iter().filter(|line| **line == format!("unsupported: {name}")).count(),
            1,
            "{name} in {debug:?}"
        );
    }
    let counted: HashMap<String, u32> = runtime.unsupported().into_iter().collect();
    assert_eq!(counted.get("thisScene.sortLayer"), Some(&6));
    assert!(mock.logged(Level::Error).is_empty());
}

#[test]
fn a_host_that_carries_methods_out_gets_them_with_their_arguments() {
    let mut mock = Mock::with_layers(&["a", "b"]);
    mock.answers_calls = true;
    let source = "export function update() {
        const bound = thisObject.getAnimation();
        bound.setFrame(3);
        thisLayer.getAnimation('wave').play();
        thisLayer.setParent(thisScene.getLayer('b'), true);
        thisLayer.rotateObjectSpace(new Vec3(0, 0, 45));
        return bound.getFrame() + (thisScene.sortLayer('b', 0) ? 100 : 0);
    }";
    let mut runtime = Runtime::new().unwrap();
    let script = runtime.attach(source, binding(0, Field::Alpha, Datum::Number(1.0)), &mut mock).unwrap();
    assert_eq!(runtime.update(script, Datum::Null, &mut mock), Some(Datum::Number(107.0)));
    assert_eq!(
        mock.calls,
        [
            "0:animation.setFrame([Null, Number(3.0)]) on alpha",
            "0:animation.play([Text(\"wave\")])",
            "0:setParent([Number(1.0), Bool(true)])",
            "0:rotateObjectSpace([Vec3([0.0, 0.0, 45.0])])",
            "0:animation.getFrame([Null]) on alpha",
            "1:sortLayer([Number(0.0)])",
        ]
    );
    assert!(runtime.unsupported().is_empty(), "{:?}", runtime.unsupported());
}

#[test]
fn console_writes_through_the_host() {
    let mut mock = Mock::with_layers(&["a"]);
    eval(
        &mut mock,
        "",
        "(console.log('value', 1.5, new Vec3(1, 2, 3), {a: 1}, [1, 2], null, undefined), console.error('bad', true), 0)",
    );
    assert_eq!(mock.logged(Level::Info), ["value 1.5 1 2 3 {\"a\":1} 1,2 null undefined"]);
    assert_eq!(mock.logged(Level::Error), ["bad true"]);
}

#[test]
fn runaway_recursion_is_an_error_and_not_a_crash() {
    let mut mock = Mock::with_layers(&["a"]);
    let mut runtime = Runtime::new().unwrap();
    let good = attach(&mut runtime, &mut mock, "export function update() { return 1; }");
    let recurses = attach(
        &mut runtime,
        &mut mock,
        "function down(n) { return down(n + 1) + 1; }\nexport function update() { return down(0); }",
    );
    assert_eq!(runtime.update(recurses, Datum::Null, &mut mock), None);
    assert_eq!(runtime.failure(recurses).map(|failure| failure.kind), Some(ErrorKind::Threw));
    assert_eq!(runtime.update(good, Datum::Null, &mut mock), Some(Datum::Number(1.0)));
}

fn env_dir(name: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os(name)?);
    dir.is_dir().then_some(dir)
}

/// The built-in classes and modules stand in for Wallpaper Engine's own, so they must
/// compute the same.
#[test]
fn built_in_classes_agree_with_wallpaper_engines() {
    let Some(assets) = env_dir("WEPAPER_TEST_ASSETS") else {
        eprintln!("skipped: WEPAPER_TEST_ASSETS is not set");
        return;
    };
    let mut stock = Mock::with_layers(&["a"]);
    let mut files = Files::new();
    files.mount_dir(assets);
    stock.files = Some(files);
    let mut built_in = Mock::with_layers(&["a"]);
    let imports = "import * as WEMath from 'WEMath';\nimport * as WEColor from 'WEColor';\nimport * as WEVector from 'WEVector';";
    let a = "new Vec3(1.5, -2, 3)";
    let b = "new Vec3(-4, 0.5, 2)";
    let m = "Mat4.compose(new Vec3(3, -1, 2), new Vec3(20, -35, 50), new Vec3(2, 3, 0.5))";
    let expressions = [
        format!("[{a}.length(), {a}.lengthSqr(), {a}.distance({b}), {a}.dot({b}), {a}.angleBetween({b})].join(' ')"),
        format!("[{a}.add({b}), {a}.subtract(2), {a}.multiply({b}), {a}.divide(new Vec2(2, 4)), {a}.cross({b}), {a}.normalize()].join(' ')"),
        format!("[{a}.reflect({b}.normalize()), {a}.refract({b}.normalize(), 0.5), {a}.project({b}), {a}.mix({b}, 0.3), {a}.mix({b}, {a})].join(' ')"),
        format!("[{a}.min({b}), {a}.max(1), {a}.clamp(-1, {b}), {a}.abs(), {a}.sign(), {a}.round(), {a}.floor(), {a}.ceil(), {a}.fract()].join(' ')"),
        format!("[{a}.mod(2), {a}.mod({b}), {a}.step(0), {a}.smoothStep(-3, 4), {a}.toSpherical(), Vec3.fromSpherical(2, 30, 60), {a}.negate()].join(' ')"),
        format!("[{a}.equals({a}.copy()), {a}.equals({b}), {a}.isFinite(), {a}.divide(0).isFinite(), new Vec3(), new Vec3(2), new Vec3(1, 2), new Vec3('4 5 6')].join(' ')"),
        "[new Vec2(3, 4).perpendicular(), new Vec2(3, 4).angle(), new Vec2(3, 4).angleBetween(new Vec2(-1, 2)), new Vec2(3, 4).rotate(30), new Vec2(5), new Vec2(new Vec3(1, 2, 3))].join(' ')".to_owned(),
        "[new Vec4(1, 2, 3, 4).add(new Vec4(4, 3, 2, 1)).multiply(0.5), new Vec4(1, 2, 3, 4).normalize(), new Vec4(2), new Vec4(1, 2), new Vec4('1 2 3 4').length()].join(' ')".to_owned(),
        format!("[{m}, {m}.inverse(), {m}.transpose(), {m}.determinant(), {m}.translation(), {m}.right(), {m}.up(), {m}.forward()].join(' ')"),
        format!("[{m}.transformPoint({a}), {m}.transformDirection({a}), {m}.multiply(new Vec4(1, 2, 3, 1)), {m}.multiply({m}), {m}.multiply(2), {m}.add({m}).subtract({m})].join(' ')"),
        format!("[{m}.translate({a}), {m}.rotate(33, {a}), {m}.scale({b}), {m}.scale(2), {m}.normalMatrix(), {m}.equals({m}.copy())].join(' ')"),
        format!("[Mat4.fromEuler(10, 20, 30), Mat4.fromEuler(new Vec3(-70, 15, 200)).extractEuler(), Mat4.fromRotation(40, {a}), Mat4.fromScale({b}), Mat4.fromTranslation(new Vec2(1, 2))].join(' ')"),
        format!("[Mat4.lookAt({a}, {b}, new Vec3(0, 1, 0)), Mat4.fromBasis({a}, {b}, {a}.cross({b})), new Mat4('1 2 3'), new Mat4().translation(new Vec3(1, 2, 3))].join(' ')"),
        format!("(() => {{ const d = {m}.decompose(); return [d.translation, d.rotation, d.scale].join(' '); }})()"),
        "(() => { const m = Mat3.compose(new Vec2(3, 4), 30, new Vec2(2, 0.5)); const d = m.decompose();
            return [m, m.inverse(), m.transpose(), m.determinant(), m.angle(), m.translation(), m.transformPoint(new Vec2(1, 2)), m.transformDirection(new Vec2(1, 2)),
                m.multiply(m), m.multiply(new Vec3(1, 2, 1)), m.translate(new Vec2(1, 1)), m.rotate(15), m.scale(new Vec2(2, 3)), d.translation, d.rotation, d.scale,
                Mat3.fromMat4(Mat4.fromEuler(10, 20, 30)), Mat3.fromBasis(new Vec2(1, 2), new Vec2(3, 4))].join(' '); })()"
            .to_owned(),
        "[WEMath.smoothStep(2, 6, 3), WEMath.mix(2, 6, 0.3), WEMath.deg2rad, WEMath.rad2deg, WEVector.angleVector2(33), WEVector.vectorAngle2(new Vec2(-2, 5))].join(' ')".to_owned(),
        "[[0.2, 0.6, 0.4], [0.9, 0.1, 0.1], [0.1, 0.2, 0.9], [0.5, 0.5, 0.5], [0, 0, 0], [1, 1, 0]].map((c) => { const hsv = WEColor.rgb2hsv(new Vec3(c[0], c[1], c[2]));
            return [hsv, WEColor.hsv2rgb(hsv), WEColor.normalizeColor(hsv), WEColor.expandColor(hsv)].join(' '); }).join(' ')"
            .to_owned(),
    ];
    for expression in &expressions {
        let numbers = |mock: &mut Mock| match eval(mock, imports, expression) {
            Datum::Text(text) => text
                .split([' ', ','])
                .map(|part| part.parse::<f64>().unwrap_or(if part == "true" { 1.0 } else { 0.0 }))
                .collect::<Vec<f64>>(),
            other => panic!("{expression} gave {other:?}"),
        };
        let (ours, theirs) = (numbers(&mut built_in), numbers(&mut stock));
        assert_eq!(ours.len(), theirs.len(), "{expression}");
        for (i, (ours, theirs)) in ours.iter().zip(&theirs).enumerate() {
            assert!(
                (ours - theirs).abs() < 1e-9 || (ours.is_nan() && theirs.is_nan()),
                "{expression}: number {i} is {ours} built in and {theirs} in Wallpaper Engine"
            );
        }
    }
    assert!(stock.logs.is_empty() && built_in.logs.is_empty(), "{:?} {:?}", stock.logs, built_in.logs);
}

/// A script found in a scene, with what it is bound to.
struct Found {
    layer: u32,
    /// The keys from the scene object down to the value, array indices as numbers.
    path: Vec<String>,
    source: String,
    properties: Value,
    value: Datum,
}

/// The value of a scene's field as scripts are given it.
fn datum_of(field: Field<'_>, value: &Value) -> Datum {
    match value {
        Value::Bool(flag) => Datum::Bool(*flag),
        Value::Number(number) => Datum::Number(*number),
        Value::String(text) if field == Field::Text => Datum::Text(text.clone()),
        Value::String(text) => {
            let parts: Vec<f64> = text.split_whitespace().map_while(|part| part.parse().ok()).collect();
            match (parts.len() == text.split_whitespace().count(), parts.as_slice()) {
                (true, [x]) => Datum::Number(*x),
                (true, [x, y]) => Datum::Vec2([*x, *y]),
                (true, [x, y, z]) => Datum::Vec3([*x, *y, *z]),
                (true, [x, y, z, w]) => Datum::Vec4([*x, *y, *z, *w]),
                _ => Datum::Text(text.clone()),
            }
        }
        _ => Datum::Null,
    }
}

/// The field a path inside a scene object stands for.
fn field_of(path: &[String]) -> Field<'_> {
    let index = |at: usize| path.get(at).and_then(|part| part.parse().ok()).unwrap_or(0);
    match path.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["origin"] => Field::Origin,
        ["scale"] => Field::Scale,
        ["angles"] => Field::Angles,
        ["visible"] => Field::Visible,
        ["alpha"] => Field::Alpha,
        ["color"] => Field::Color,
        ["text"] => Field::Text,
        ["size"] => Field::Size,
        ["parallaxDepth"] => Field::ParallaxDepth,
        ["effects", _, "visible"] => Field::EffectVisible(index(1)),
        ["effects", _, .., name] => Field::EffectConstant(index(1), name),
        ["instanceoverride", name] => Field::Instance(name),
        ["animationlayers", _, name] => Field::AnimationLayer(index(1), name),
        [.., name] => Field::Named(name),
        [] => Field::Named(""),
    }
}

/// Collects the values under `value` that carry a script.
fn find_scripts(value: &Value, layer: u32, path: &mut Vec<String>, found: &mut Vec<Found>) {
    if let Some(source) = value.get("script").as_str() {
        let initial = datum_of(field_of(path), value.get("value"));
        found.push(Found {
            layer,
            path: path.clone(),
            source: source.to_owned(),
            properties: value.get("scriptproperties").clone(),
            value: initial,
        });
    }
    match value {
        Value::Object(_) => {
            for (key, member) in value.members() {
                path.push(key.to_owned());
                find_scripts(member, layer, path, found);
                path.pop();
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                path.push(i.to_string());
                find_scripts(item, layer, path, found);
                path.pop();
            }
        }
        _ => {}
    }
}

fn count(tally: &mut BTreeMap<String, usize>, key: impl Into<String>) {
    *tally.entry(key.into()).or_default() += 1;
}

fn top(tally: &BTreeMap<String, usize>, limit: usize) -> String {
    let mut rows: Vec<_> = tally.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    rows.iter().take(limit).map(|(key, count)| format!("  {count:5}  {key}")).collect::<Vec<_>>().join("\n")
}

/// Attaches every script of every scene in the library to a host made of the scene's
/// own values, runs `init`, the user properties, 30 frames and the events, and prints
/// what came of it.
#[test]
fn every_script_of_the_library_attaches_and_runs() {
    let (Some(assets), Some(workshop)) = (env_dir("WEPAPER_TEST_ASSETS"), env_dir("WEPAPER_TEST_WORKSHOP"))
    else {
        eprintln!("skipped: WEPAPER_TEST_ASSETS / WEPAPER_TEST_WORKSHOP are not set");
        return;
    };
    let (mut scenes, mut scenes_with_scripts, mut scripts, mut compiled, mut survived) = (0, 0, 0, 0, 0);
    let (mut compile_failures, mut run_failures, mut unsupported) =
        (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let (mut handlers, mut slowest, mut examples) = (BTreeMap::new(), Duration::ZERO, Vec::new());

    let mut wallpapers: Vec<PathBuf> = std::fs::read_dir(&workshop)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    wallpapers.sort();
    for dir in wallpapers {
        let Ok(project) = std::fs::read(dir.join("project.json")) else { continue };
        let Ok(project) = wepaper_json::parse(&String::from_utf8_lossy(&project)) else { continue };
        if !project.get("type").as_str().is_some_and(|kind| kind.eq_ignore_ascii_case("scene")) {
            continue;
        }
        let mut files = Files::new();
        files.mount_dir(assets.clone());
        files.mount_dir(dir.clone());
        if let Ok(archive) = Archive::open(&dir.join("scene.pkg")) {
            files.mount_archive(archive);
        }
        let Ok(scene) = files.read_text(project.get("file").as_str().unwrap_or("scene.json")) else {
            continue;
        };
        let Ok(scene) = wepaper_json::parse(&scene) else { continue };
        scenes += 1;

        let mut mock = Mock::default();
        let mut found = Vec::new();
        for object in scene.get("objects").items() {
            let layer = mock
                .layer(object.get("name").as_str().unwrap_or(""), object.get("id").as_i64().unwrap_or(-1));
            for (key, member) in object.members() {
                // What a plain value is before any script: itself, or the `value` beside the script.
                let plain = if member.get("value").is_null() { member } else { member.get("value") };
                let path = [key.to_owned()];
                let field = field_of(&path);
                if !matches!(field, Field::Named(_)) {
                    mock.set(layer, field, datum_of(field, plain));
                }
            }
            for (i, effect) in object.get("effects").items().enumerate() {
                mock.set(
                    layer,
                    Field::EffectName(i),
                    Datum::Text(effect.get("name").as_str().unwrap_or("").to_owned()),
                );
                mock.set(layer, Field::EffectVisible(i), Datum::Bool(true));
            }
            find_scripts(object, layer.0, &mut Vec::new(), &mut found);
        }
        if found.is_empty() {
            continue;
        }
        for (layer, object) in scene.get("objects").items().enumerate() {
            if let Some(parent) = object.get("parent").as_i64().and_then(|id| mock.layer_with_id(id)) {
                mock.set(LayerId(layer as u32), Field::Parent, Datum::Layer(parent));
            }
        }
        scenes_with_scripts += 1;
        let properties: Vec<(String, Value)> = project
            .get("general")
            .get("properties")
            .members()
            .map(|(name, property)| (name.to_owned(), property.get("value").clone()))
            .collect();
        mock.user = properties.iter().cloned().collect();
        mock.files = Some(files);
        let projection = scene.get("general").get("orthogonalprojection");
        mock.environment.canvas_size = [
            projection.get("width").as_f64().unwrap_or(1920.0),
            projection.get("height").as_f64().unwrap_or(1080.0),
        ];
        mock.environment.screen_size = [2560.0, 1440.0];
        mock.environment.time_of_day = 0.4;

        let mut runtime = Runtime::new().unwrap();
        let mut attached: Vec<(ScriptId, Datum)> = Vec::new();
        for script in &found {
            scripts += 1;
            let binding = Binding {
                layer: LayerId(script.layer),
                field: field_of(&script.path),
                value: script.value.clone(),
                properties: &script.properties,
            };
            match runtime.attach(&script.source, binding, &mut mock) {
                Ok(id) => {
                    compiled += 1;
                    attached.push((id, script.value.clone()));
                }
                Err(error) => count(&mut compile_failures, format!("{:?}: {}", error.kind, error.message)),
            }
        }
        for (id, value) in runtime.init(&mut mock) {
            if let Some(slot) = attached.iter_mut().find(|(script, _)| *script == id) {
                slot.1 = value;
            }
        }
        runtime.apply_user_properties(&properties, &mut mock);
        for frame in 0..30 {
            mock.environment.time = f64::from(frame) / 30.0;
            mock.environment.frame_time = 1.0 / 30.0;
            mock.environment.cursor_scene = [f64::from(frame) * 40.0, 500.0];
            mock.environment.spectrum = [[0.3 + 0.01 * frame as f32; 64]; 2];
            let started = std::time::Instant::now();
            runtime.begin_frame(&mut mock);
            for (id, value) in &mut attached {
                if let Some(new) = runtime.update(*id, value.clone(), &mut mock) {
                    *value = new;
                }
            }
            slowest = slowest.max(started.elapsed());
            if frame == 10 {
                for layer in 0..mock.layers.len() as u32 {
                    for event in [
                        CursorEvent::Enter,
                        CursorEvent::Down,
                        CursorEvent::Up,
                        CursorEvent::Click,
                        CursorEvent::Leave,
                    ] {
                        runtime.cursor(event, Some(LayerId(layer)), &mut mock);
                    }
                }
                runtime.cursor(CursorEvent::Move, None, &mut mock);
                runtime.media(MediaEvent::Status { enabled: true }, &mut mock);
                runtime.media(MediaEvent::Playback { state: 1 }, &mut mock);
                let track = MediaEvent::Properties {
                    title: "Title",
                    artist: "Artist",
                    sub_title: "",
                    album_title: "Album",
                    album_artist: "Artist",
                    genres: "",
                    content_type: "audio",
                };
                runtime.media(track, &mut mock);
                let cover = MediaEvent::Thumbnail {
                    has_thumbnail: true,
                    primary: [0.8, 0.2, 0.2],
                    secondary: [0.2; 3],
                    tertiary: [0.5; 3],
                    text: [1.0; 3],
                    high_contrast: [1.0; 3],
                };
                runtime.media(cover, &mut mock);
                runtime.media(MediaEvent::Timeline { position: 10.0, duration: 200.0 }, &mut mock);
            }
        }
        runtime.destroy(&mut mock);
        for (id, _) in &attached {
            match runtime.failure(*id) {
                None => survived += 1,
                Some(error) => {
                    count(&mut run_failures, format!("{:?}: {}", error.kind, error.message));
                    examples.push(format!(
                        "  {}: {error}",
                        dir.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
            }
            for (name, handler) in [
                ("update", Handler::Update),
                ("init", Handler::Init),
                ("applyUserProperties", Handler::UserProperties),
                ("cursorClick", Handler::CursorClick),
            ] {
                if runtime.handles(*id, handler) {
                    count(&mut handlers, name);
                }
            }
        }
        for (name, uses) in runtime.unsupported() {
            *unsupported.entry(name).or_default() += uses as usize;
        }
    }

    eprintln!("{scenes} scenes, {scenes_with_scripts} with scripts, {scripts} scripts");
    eprintln!("compiled: {compiled}; failed to compile: {}", scripts - compiled);
    eprintln!("{}", top(&compile_failures, 15));
    eprintln!(
        "ran 30 frames and the events without failing: {survived}; threw or timed out: {}",
        compiled - survived
    );
    eprintln!("{}", top(&run_failures, 25));
    eprintln!(
        "the first of them, by wallpaper:\n{}",
        examples.iter().take(12).cloned().collect::<Vec<_>>().join("\n")
    );
    eprintln!("unsupported members used (by uses):\n{}", top(&unsupported, 40));
    eprintln!("still enabled with a handler: {handlers:?}; slowest frame of a scene: {slowest:?}");
    assert!(scenes > 0 && scripts > 0, "nothing was checked");
    assert!(compiled * 10 >= scripts * 9, "only {compiled} of {scripts} scripts compile");
}
