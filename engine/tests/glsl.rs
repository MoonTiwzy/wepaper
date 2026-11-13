//! The shader dialect: includes, declarations, assembly, repairs.

use wepaper_engine::glsl::{Combos, Source, Stage};

fn include(name: &str) -> Option<String> {
    match name {
        "common.h" => Some("#define M_PI 3.14\n#include \"inner.h\"\nfloat twice(float x) { return x * 2.0; }\n".to_owned()),
        "inner.h" => Some("// inner\n#include \"common.h\"\nfloat inner() { return 1.0; }\n".to_owned()),
        "blend.h" => Some("#include \"common.h\"\nvec3 blend(vec3 a) { return a; }\n".to_owned()),
        _ => None,
    }
}

#[test]
fn resolves_nested_includes_once() {
    let source = Source::load(Stage::Fragment, "#include \"common.h\"\n#include \"blend.h\"\n#include \"gone.h\"\nvoid main() {}\n", &include);
    assert_eq!(source.text.matches("float twice").count(), 1, "a header included twice is inlined once");
    assert_eq!(source.text.matches("float inner").count(), 1);
    assert!(source.text.contains("vec3 blend"));
    assert!(source.text.contains("// missing include: gone.h"));
    assert!(!source.text.contains("#include"));
    let inner = source.text.find("float inner").unwrap();
    assert!(inner < source.text.find("float twice").unwrap(), "an include is inlined where it stands");
}

#[test]
fn reads_combos_uniforms_and_samplers() {
    let text = r#"
// [COMBO] {"material":"ui_blend","combo":"BLENDMODE","type":"imageblending","default":2}
// [COMBO] {"combo":"MASK","default":0}
uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"x","hidden":true}
uniform sampler2D g_Texture1; // {"material":"mask","combo":"MASK","default":"util/white"}
uniform float u_Speed; // {"material":"speed","default":0.5,"range":[0,1]}
uniform vec3 u_Tint; // {"material":"tint","type":"color","default":"1 0.5 0"}
uniform float g_Time;
uniform vec2 u_Arr[4]; // {"material":"arr"}
"#;
    let declared = Source::load(Stage::Fragment, text, &include).declared;
    assert_eq!(declared.combos.iter().map(|c| (c.name.as_str(), c.default)).collect::<Vec<_>>(), [("BLENDMODE", 2), ("MASK", 0)]);
    assert_eq!(declared.samplers.len(), 2);
    assert_eq!((declared.samplers[1].slot, declared.samplers[1].combo.as_deref(), declared.samplers[1].default.as_deref()), (1, Some("MASK"), Some("util/white")));
    assert_eq!(declared.samplers[0].default, None);
    let speed = declared.uniforms.iter().find(|u| u.name == "u_Speed").unwrap();
    assert_eq!((speed.material.as_deref(), speed.default.as_f64()), (Some("speed"), Some(0.5)));
    assert_eq!(declared.uniforms.iter().find(|u| u.name == "u_Tint").unwrap().default.as_str(), Some("1 0.5 0"));
    assert!(declared.uniforms.iter().any(|u| u.name == "u_Arr"), "an array's name has no brackets");
    assert!(!declared.uniforms.iter().any(|u| u.name == "g_Time"), "no comment, nothing declared");
}

#[test]
fn assembles_both_stages() {
    let mut combos = Combos::new();
    combos.insert("MASK".to_owned(), 1);
    let vertex = Source::load(Stage::Vertex, "#version 110\nattribute vec3 a_Position;\nvarying vec2 v_TexCoord;\nvoid main() {}\n", &include).assemble(&combos);
    assert!(vertex.starts_with("#version 330 core\n"));
    assert_eq!(vertex.matches("#version").count(), 2, "the shader's own #version is commented out: {vertex}");
    assert!(vertex.contains("// #version 110"));
    assert!(vertex.contains("#define attribute in") && vertex.contains("#define varying out"));
    assert!(vertex.contains("#define MASK 1\n"));
    let fragment = Source::load(Stage::Fragment, "varying vec2 v_TexCoord;\nvoid main() { gl_FragColor = vec4(1.0); }\n", &include).assemble(&combos);
    assert!(fragment.contains("#define varying in") && fragment.contains("#define gl_FragColor wepaper_color"));
    assert!(fragment.contains("out vec4 wepaper_color;"));
    assert!(fragment.find("#define MASK 1").unwrap() < fragment.find("void main").unwrap());
}

#[test]
fn reserved_identifiers_are_renamed() {
    let text = "uniform float filter;\nvec4 sample = texSample2D(g_Texture0, uv);\nfloat x = v.sample + sample.x * filter; // sample stays here\nflat in int id;\nfloat flat = 1.0;\n";
    let out = Source::load(Stage::Fragment, text, &include).text;
    assert!(out.contains("uniform float filter_w;"));
    assert!(out.contains("vec4 sample_w = texSample2D"));
    assert!(out.contains("v.sample + sample_w.x * filter_w; // sample stays here"));
    assert!(out.contains("flat in int id;"), "a qualifier is not a name: {out}");
    assert!(out.contains("float flat_w = 1.0;"));
}

#[test]
fn own_mod_function_replaces_the_builtin() {
    let text = "float mod(float x, float y) { return x - y * floor(x / y); }\nvoid main() { float a = mod(3.0, 2.0); float b = fmod(a, 1.0); vec2 c = v.mod; }\n";
    let out = Source::load(Stage::Fragment, text, &include).text;
    assert!(out.contains("float wepaper_mod(float x, float y)"));
    assert!(out.contains("float a = wepaper_mod(3.0, 2.0);"));
    assert!(out.contains("fmod(a, 1.0)") && out.contains("v.mod"), "{out}");
    let untouched = Source::load(Stage::Fragment, "void main() { float a = mod(3.0, 2.0); }\n", &include).text;
    assert!(untouched.contains("float a = mod(3.0, 2.0);"));
}
