//! What goes in front of every shader.

/// Maps the dialect onto GLSL 330. Both stages get it.
pub(super) const COMMON: &str = "\
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
";

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
