#version 330 core
// Darkens the frame towards its corners. u_Radius is where the darkening
// starts (in half-diagonals), u_Strength how dark the corners get.
uniform sampler2D u_Frame;
uniform vec2 u_Resolution;
uniform float u_Strength;
uniform float u_Radius;
in vec2 v_TexCoord;
out vec4 color;

void main() {
    vec4 frame = texture(u_Frame, v_TexCoord);
    vec2 from_center = (v_TexCoord - 0.5) * vec2(u_Resolution.x / u_Resolution.y, 1.0);
    float distance_out = length(from_center) / length(vec2(u_Resolution.x / u_Resolution.y, 1.0) * 0.5);
    float shade = smoothstep(u_Radius, u_Radius + 0.6, distance_out) * u_Strength;
    color = vec4(frame.rgb * (1.0 - clamp(shade, 0.0, 1.0)), 1.0);
}
