//! What goes in front of every shader.

/// Maps the dialect's names onto GLSL's.
const NAMES: &str = "\
#define GLSL 1
#define highp
#define mediump
#define lowp
#define mul(x, y) ((y) * (x))
#define frac fract
#define lerp mix
#define saturate(x) clamp(x, 0.0, 1.0)
#define atan2 atan
#define fmod(x, y) ((x) - (y) * trunc((x) / (y)))
#define log10(x) (log2(x) * 0.30102999566)
#define ddx dFdx
#define ddy(x) dFdy(-(x))
#define CAST2(x) vec2(x)
#define CAST3(x) vec3(x)
#define CAST4(x) vec4(x)
#define CAST3X3(x) mat3(x)
#define float1 float
#define float2 vec2
#define float3 vec3
#define float4 vec4
#define float2x2 mat2
#define float3x3 mat3
#define float4x4 mat4
#define float4x3 mat4x3
#define texSample2D texture
#define texSample2DLod textureLod
#define texSample3D texture
#define texture2D texture
#define texture2DLod textureLod
#define textureCube texture
#define sampler2DComparison sampler2DShadow
#define texSample2DCompare(s, uv, depth) texture(s, vec3(uv, depth))
float wepaper_rem(float x, float y) { return x - y * trunc(x / y); }
float wepaper_rem(float x, int y) { return wepaper_rem(x, float(y)); }
float wepaper_rem(int x, float y) { return wepaper_rem(float(x), y); }
int wepaper_rem(int x, int y) { return x % y; }
uint wepaper_rem(uint x, uint y) { return x % y; }
uint wepaper_rem(uint x, int y) { return x % uint(y); }
uint wepaper_rem(int x, uint y) { return uint(x) % y; }
";

/// HLSL widens a scalar to match a vector argument (`max(0, colour.rgb)`); GLSL has
/// no such overloads for these functions. They get wrappers that do.
fn promoting(name: &str, integers: bool) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "float wepaper_{name}(float a, float b) {{ return {name}(a, b); }}");
    if integers {
        let _ = writeln!(out, "int wepaper_{name}(int a, int b) {{ return {name}(a, b); }}");
    }
    for vector in ["vec2", "vec3", "vec4"] {
        let _ = writeln!(out, "{vector} wepaper_{name}({vector} a, {vector} b) {{ return {name}(a, b); }}");
        let _ = writeln!(
            out,
            "{vector} wepaper_{name}({vector} a, float b) {{ return {name}(a, {vector}(b)); }}"
        );
        let _ = writeln!(
            out,
            "{vector} wepaper_{name}(float a, {vector} b) {{ return {name}({vector}(a), b); }}"
        );
    }
    out
}

/// What goes in front of both stages: the names, then the wrappers and the macros
/// that route the plain names to them.
pub(super) fn common() -> &'static str {
    static COMMON: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    COMMON.get_or_init(|| {
        let mut out = String::from(NAMES);
        for (name, integers) in [("max", true), ("min", true), ("pow", false)] {
            out.push_str(&promoting(name, integers));
        }
        // After the wrappers, which call the real functions.
        out.push_str("#define max wepaper_max\n#define min wepaper_min\n#define pow wepaper_pow\n");
        out
    })
}

/// What only the vertex stage gets.
pub(super) const VERTEX: &str = "\
#define attribute in
#define varying out
";

/// What only the fragment stage gets.
pub(super) const FRAGMENT: &str = "\
#define varying in
out vec4 wepaper_color;
#define gl_FragColor wepaper_color
";
