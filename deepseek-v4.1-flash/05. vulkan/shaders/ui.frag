#version 450
// UI fragment shader: samples the R8 bitmap-font atlas (red channel = coverage).

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 0) out vec4 out_color;

layout(set = 0, binding = 0) uniform texture2D font_atlas;
layout(set = 0, binding = 1) uniform sampler font_sampler;

void main() {
    float coverage = texture(sampler2D(font_atlas, font_sampler), v_uv).r;
    out_color = vec4(v_color.rgb, v_color.a * coverage);
}
