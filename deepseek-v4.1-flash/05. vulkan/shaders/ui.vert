#version 450
// Minimal immediate-mode UI: one vertex buffer of textured, coloured quads
// (bitmap font atlas + solid rectangles).  Rasterization is allowed for the UI.

layout(location = 0) in vec2 in_position;   // window pixels, origin top-left
layout(location = 1) in vec2 in_uv;
layout(location = 2) in vec4 in_color;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_color;

layout(push_constant) uniform PushConstants {
    vec2 viewport;      // framebuffer size in pixels
    vec2 pad;
} pc;

void main() {
    vec2 ndc = vec2(in_position.x / pc.viewport.x * 2.0 - 1.0,
                    in_position.y / pc.viewport.y * 2.0 - 1.0);
    gl_Position = vec4(ndc, 0.0, 1.0);
    v_uv = in_uv;
    v_color = in_color;
}
