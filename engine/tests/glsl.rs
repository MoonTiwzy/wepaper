//! The shader dialect: includes, declarations, assembly, repairs.

use wepaper_engine::glsl::{Combos, Source, Stage};

fn include(name: &str) -> Option<String> {
    match name {
        "common.h" => Some(
            "#define M_PI 3.14\n#include \"inner.h\"\nfloat twice(float x) { return x * 2.0; }\n".to_owned(),
        ),
        "inner.h" => Some("// inner\n#include \"common.h\"\nfloat inner() { return 1.0; }\n".to_owned()),
        "blend.h" => Some("#include \"common.h\"\nvec3 blend(vec3 a) { return a; }\n".to_owned()),
        _ => None,
    }
}

#[test]
fn resolves_nested_includes_once() {
    let source = Source::load(
        Stage::Fragment,
        "#include \"common.h\"\n#include \"blend.h\"\n#include \"gone.h\"\nvoid main() {}\n",
        &include,
    );
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
    assert_eq!(
        declared.combos.iter().map(|c| (c.name.as_str(), c.default)).collect::<Vec<_>>(),
        [("BLENDMODE", 2), ("MASK", 0)]
    );
    assert_eq!(declared.samplers.len(), 2);
    assert_eq!(
        (
            declared.samplers[1].slot,
            declared.samplers[1].combo.as_deref(),
            declared.samplers[1].default.as_deref()
        ),
        (1, Some("MASK"), Some("util/white"))
    );
    assert_eq!(declared.samplers[0].default, None);
    let speed = declared.uniforms.iter().find(|u| u.name == "u_Speed").unwrap();
    assert_eq!((speed.material.as_deref(), speed.default.as_f64()), (Some("speed"), Some(0.5)));
    assert_eq!(
        declared.uniforms.iter().find(|u| u.name == "u_Tint").unwrap().default.as_str(),
        Some("1 0.5 0")
    );
    assert!(declared.uniforms.iter().any(|u| u.name == "u_Arr"), "an array's name has no brackets");
    assert!(!declared.uniforms.iter().any(|u| u.name == "g_Time"), "no comment, nothing declared");
}

#[test]
fn assembles_both_stages() {
    let mut combos = Combos::new();
    combos.insert("MASK".to_owned(), 1);
    let vertex = Source::load(
        Stage::Vertex,
        "#version 110\nattribute vec3 a_Position;\nvarying vec2 v_TexCoord;\nvoid main() {}\n",
        &include,
    )
    .assemble(&combos, false);
    assert!(vertex.starts_with("#version 330 core\n"));
    assert_eq!(vertex.matches("#version").count(), 2, "the shader's own #version is commented out: {vertex}");
    assert!(vertex.contains("// #version 110"));
    assert!(vertex.contains("#define attribute in") && vertex.contains("#define varying out"));
    assert!(vertex.contains("#define MASK 1\n"));
    let fragment = Source::load(
        Stage::Fragment,
        "varying vec2 v_TexCoord;\nvoid main() { gl_FragColor = vec4(1.0); }\n",
        &include,
    )
    .assemble(&combos, false);
    assert!(
        fragment.contains("#define varying in") && fragment.contains("#define gl_FragColor wepaper_color")
    );
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
    let untouched =
        Source::load(Stage::Fragment, "void main() { float a = mod(3.0, 2.0); }\n", &include).text;
    assert!(untouched.contains("float a = mod(3.0, 2.0);"));
}

#[test]
fn samplers_are_declared_before_the_headers_that_use_them() {
    let include = |name: &str| {
        (name == "blur.h")
            .then(|| "vec3 blur(vec2 u) { return texSample2D(g_Texture0, u).rgb; }\n".to_owned())
    };
    let text = "#include \"blur.h\"\nuniform sampler2D g_Texture0; // {\"material\":\"framebuffer\"}\n#if MASK\nuniform sampler2D g_Texture1; // {\"combo\":\"MASK\"}\n#endif\nuniform sampler2D g_Texture0;\nuniform float g_Time;\nvoid main() {}\n";
    let source = Source::load(Stage::Fragment, text, &include);
    assert!(
        source.text.starts_with("uniform sampler2D g_Texture0;\nuniform sampler2D g_Texture1;\n"),
        "{}",
        source.text
    );
    assert_eq!(source.text.matches("\nuniform sampler2D g_Texture0").count(), 0, "declared once, at the top");
    assert!(
        source.text.find("vec3 blur").unwrap() > source.text.find("uniform sampler2D g_Texture0;").unwrap()
    );
    assert!(source.text.contains("uniform float g_Time;"), "other uniforms stay where they are");
    assert_eq!(source.declared.samplers.len(), 2, "the settings in the comments were read before the move");
}

#[test]
fn lenient_assembly_has_no_version_and_no_redefinitions() {
    let source = Source::load(
        Stage::Fragment,
        "varying vec2 v;\nvoid main() { gl_FragColor = vec4(1.0); }\n",
        &include,
    );
    let lenient = source.assemble(&Combos::new(), true);
    assert!(
        !lenient.contains("#version")
            && !lenient.contains("#define varying")
            && !lenient.contains("wepaper_color")
    );
    assert!(lenient.contains("#define texSample2D texture") && lenient.contains("#line 0\nvarying vec2 v;"));
}

#[test]
fn scalar_and_vector_arguments_get_wrappers() {
    let source = Source::load(
        Stage::Fragment,
        "void main() { gl_FragColor = vec4(max(0, vec3(1.0)), 1.0); }\n",
        &include,
    );
    let text = source.assemble(&Combos::new(), false);
    assert!(text.contains("vec3 wepaper_max(float a, vec3 b) { return max(vec3(a), b); }"));
    let wrappers = text.find("vec3 wepaper_max(float a, vec3 b)").unwrap();
    let rename = text.find("#define max wepaper_max").unwrap();
    assert!(
        wrappers < rename && rename < text.find("void main").unwrap(),
        "the wrappers call the real max, the shader calls the wrappers"
    );
    assert!(!text.contains("int wepaper_pow"), "pow has no integer form");
}

#[test]
fn the_lighting_a_shader_requires_is_written_in_for_it() {
    let text = "#include \"common.h\"\n#require LightingV1\nvoid main() { vec3 l = PerformLighting_V1(p, c, n, v, s, f, r, m); }\n";
    let source = Source::load(Stage::Fragment, text, &include);
    let definition = source.text.find("vec3 PerformLighting_V1(").expect("the function is defined");
    assert!(definition < source.text.find("void main").unwrap(), "where the shader asked for it");
    assert!(!source.text.contains("#require"), "the directive itself is gone");
    // Without a light count from the scene there is nothing to loop over: no uniforms.
    let unlit = source.assemble(&Combos::new(), false);
    assert!(unlit.contains("#define WEPAPER_LIGHTS 0"));
    let mut combos = Combos::new();
    combos.insert("WEPAPER_LIGHTS".to_owned(), 2);
    let lit = source.assemble(&combos, false);
    assert!(lit.contains("#define WEPAPER_LIGHTS 2\n"));
    assert!(lit.contains("uniform vec4 g_LightsColorRadius[WEPAPER_LIGHTS];"));
    assert!(lit.contains("uniform vec4 g_LightsPositionExponent[WEPAPER_LIGHTS];"));
    assert!(lit.contains("ComputePBRLightShadow(") && lit.contains("ComputePBRLightShadowInfinite("));
    assert!(lit.contains("isnan("), "a light that is not a number gives none, as HLSL's max() makes it");
    // Anything else a shader requires is not known: it stays a comment.
    let other = Source::load(Stage::Fragment, "#require SomethingElse\nvoid main() {}\n", &include);
    assert!(other.text.contains("// #require SomethingElse"));
}

#[test]
fn a_varying_the_stages_declare_differently_takes_the_vertex_stage_type() {
    let vertex = Source::load(
        Stage::Vertex,
        "varying vec4 v_TexCoord;\nvarying vec2 v_Small;\nvarying vec3 v_Same;\nvoid main() { v_TexCoord.xy = a; }\n",
        &include,
    );
    let fragment = Source::load(
        Stage::Fragment,
        "varying vec2 v_TexCoord;\nvarying vec4 v_Small;\nvarying vec3 v_Same;\nvoid main() { gl_FragColor = vec4(v_TexCoord, v_Small.zw); }\n",
        &include,
    );
    let fixed = fragment.matching(&vertex);
    // A wider one is read through its first components, a narrower one is padded with zeros.
    assert!(
        fixed.text.contains("varying vec4 v_TexCoord;\n#define v_TexCoord v_TexCoord.xy\n"),
        "{}",
        fixed.text
    );
    assert!(
        fixed.text.contains("varying vec2 v_Small;\n#define v_Small vec4(v_Small, 0.0, 0.0)\n"),
        "{}",
        fixed.text
    );
    assert!(
        fixed.text.contains("varying vec3 v_Same;\nvarying")
            || fixed.text.contains("varying vec3 v_Same;\nvoid")
    );
    assert!(!fixed.text.contains("#define v_Same"));
    // Stages that agree come back as they were.
    assert_eq!(vertex.matching(&vertex).text, vertex.text);
}

#[test]
fn a_mask_slot_names_the_combos_of_its_channels() {
    let text = r#"uniform sampler2D g_Texture2; // {"combo":"PBRMASKS","components":[{"label":"m","combo":"METALLIC_MAP"},{"label":"e","combo":"EMISSIVE_MAP"}]}
uniform sampler2D g_Texture0; // {"material":"albedo"}
"#;
    let declared = Source::load(Stage::Fragment, text, &include).declared;
    let mask = declared.samplers.iter().find(|s| s.slot == 2).unwrap();
    assert_eq!(mask.combo.as_deref(), Some("PBRMASKS"));
    assert_eq!(mask.components, ["METALLIC_MAP", "EMISSIVE_MAP"]);
    assert!(declared.samplers.iter().find(|s| s.slot == 0).unwrap().components.is_empty());
}
