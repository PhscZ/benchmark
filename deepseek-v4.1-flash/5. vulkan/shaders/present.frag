#version 450
// Display pass: averages the accumulated linear samples, applies exposure,
// Reinhard tone mapping and (optionally) the sRGB transfer function.
// The exact same arithmetic is used by the CPU PNG export path (src/color.rs).

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

// naga's GLSL front end takes the separate-image/sampler form (as wgpu does):
// binding 0 is the sampled image, binding 1 the sampler.
layout(set = 0, binding = 0) uniform texture2D accum_texture;
layout(set = 0, binding = 1) uniform sampler accum_sampler;

layout(push_constant) uniform PushConstants {
    float inv_sample_count;   // 1 / accumulated sample count
    float exposure;
    float encode_srgb;        // 1 => encode here (UNORM swapchain), 0 => hardware sRGB
    float pad;
} pc;

vec3 srgb_encode(vec3 c) {
    c = clamp(c, 0.0, 1.0);
    vec3 linear_part = c * 12.92;
    vec3 gamma_part = 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055;
    return mix(linear_part, gamma_part, step(vec3(0.0031308), c));
}

void main() {
    vec3 sum = texture(sampler2D(accum_texture, accum_sampler), v_uv).rgb;
    vec3 c = sum * pc.inv_sample_count;     // average samples before exposure
    c *= pc.exposure;
    c = c / (1.0 + c);                      // Reinhard, component-wise
    if (pc.encode_srgb > 0.5) {
        c = srgb_encode(c);                 // converted to sRGB exactly once
    }
    out_color = vec4(c, 1.0);
}
