#version 330 core
// Darkens the frame by u_Amount: 0 leaves it alone, 1 is black.
uniform sampler2D u_Frame;
uniform float u_Amount;
in vec2 v_TexCoord;
out vec4 color;

void main() {
    vec4 frame = texture(u_Frame, v_TexCoord);
    color = vec4(frame.rgb * (1.0 - clamp(u_Amount, 0.0, 1.0)), 1.0);
}
