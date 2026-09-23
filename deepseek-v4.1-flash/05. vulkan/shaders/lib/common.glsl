// =============================================================================
// Shared GPU scene description, ray/primitive intersection and shading code.
//
// This file is textually included (see build.rs) by BOTH
//   * shaders/raytrace.comp      - the production path tracer
//   * shaders/intersect_test.comp - the GPU intersection self-test
// so that the code under test is literally the code that renders the image.
//
// Scene geometry is a fixed analytic description:
//   * 3 spheres (analytic, never tessellated)
//   * 10 triangles (open-front room: floor, ceiling, back wall, 2 side walls,
//     two triangles each)
// Primitive ordering is deterministic: spheres are indexed 0..sphere_count-1 and
// are tested before triangles, which are indexed 0..triangle_count-1.  Nearest
// hits use a strict `<` comparison, so on an exact distance tie the lower
// (earlier) primitive wins.
// =============================================================================

// --- documented numerical tolerances ----------------------------------------
const float RAY_T_MIN = 1.0e-4;             // nearest accepted ray distance
const float RAY_T_MAX = 1.0e30;             // farthest accepted ray distance
const float SECONDARY_ORIGIN_EPS = 1.0e-4;  // secondary ray origins are pushed this far along N
const float SHADOW_TMAX_SLACK = 1.0e-3;     // shadow ray tmax = |light - P| - slack
const float AMBIENT_SCALE = 0.02;           // ambient = 0.02 * baseColor
const float TRI_PARALLEL_EPS = 1.0e-12;     // |det| below this => ray parallel to triangle
const float HALF_VECTOR_EPS = 1.0e-6;       // |L + V| below this => zero specular
const float CONTRIBUTION_EPS = 1.0e-6;      // remaining throughput below this => stop

const uint PRIM_SPHERE = 0u;
const uint PRIM_TRIANGLE = 1u;

struct Sphere {
    vec4 center_radius;   // xyz = center, w = radius
    vec4 base_color;      // linear RGB
    vec4 params;          // x = kd, y = ks, z = shininess, w = reflectivity
};

struct Triangle {
    vec4 v0;
    vec4 v1;
    vec4 v2;
    vec4 base_color;
    vec4 params;          // x = kd, y = ks, z = shininess, w = reflectivity
};

struct Surface {
    float t;              // ray parameter of the hit
    uint prim_kind;       // PRIM_SPHERE or PRIM_TRIANGLE
    uint prim_index;      // index inside its primitive array
    vec3 position;
    vec3 normal;          // unit, always oriented against the incoming ray
    vec3 base_color;
    float kd;
    float ks;
    float shininess;
    float reflectivity;
};

layout(set = 0, binding = 0, rgba32f) uniform image2D accum_image;
layout(set = 0, binding = 1, std430) readonly buffer SphereBuffer { Sphere spheres[]; };
layout(set = 0, binding = 2, std430) readonly buffer TriangleBuffer { Triangle triangles[]; };

layout(set = 0, binding = 3, std140) uniform SceneUniform {
    vec4 cam_origin;
    vec4 cam_lower_left;
    vec4 cam_horizontal;
    vec4 cam_vertical;
    vec4 light_position;
    vec4 light_intensity;   // linear RGB intensity
    uvec4 counts;           // x = sphere_count, y = triangle_count, z = seed, w = max_reflections
} scene;

// --- deterministic integer hash (PCG-style avalanche) -----------------------
uint hash_u32(uint x) {
    x ^= x >> 17;
    x *= 0xed5ad4bbu;
    x ^= x >> 11;
    x *= 0xac4c1b51u;
    x ^= x >> 15;
    x *= 0x31848babu;
    x ^= x >> 14;
    return x;
}

float hash_unit_float(uint x) {
    // 2^-32 scaling of a full 32-bit hash -> [0, 1)
    return float(hash_u32(x)) * (1.0 / 4294967296.0);
}

// --- ray/sphere -------------------------------------------------------------
// Ray direction is assumed normalized.  Returns the nearest root inside
// [tmin, tmax) - the upper bound is exclusive, so a hit at exactly the current
// nearest distance never replaces it (this is what makes the deterministic
// primitive ordering below work).  A ray starting inside the sphere reports the
// exit point, and a tangent ray (discriminant == 0) reports the single touch
// point.  A negative discriminant (miss) is rejected without evaluating sqrt().
bool intersect_sphere(vec3 ro, vec3 rd, float tmin, float tmax, Sphere s, out float t_hit) {
    vec3 oc = ro - s.center_radius.xyz;
    float radius = s.center_radius.w;
    float b = dot(oc, rd);
    float c = dot(oc, oc) - radius * radius;
    float disc = b * b - c;
    if (disc < 0.0) {
        return false;
    }
    float sq = sqrt(disc);
    float t0 = -b - sq;
    float t1 = -b + sq;
    float t = t0;
    if (t < tmin || t >= tmax) {
        t = t1;
        if (t < tmin || t >= tmax) {
            return false;
        }
    }
    t_hit = t;
    return true;
}

// --- ray/triangle (Möller-Trumbore, two-sided) ------------------------------
// Parallel rays (|det| <= TRI_PARALLEL_EPS) are rejected, no back-face culling
// is performed, and the returned normal is flipped to oppose the incoming ray.
bool intersect_triangle(vec3 ro, vec3 rd, float tmin, float tmax, Triangle tri, out float t_hit) {
    vec3 e1 = tri.v1.xyz - tri.v0.xyz;
    vec3 e2 = tri.v2.xyz - tri.v0.xyz;
    vec3 pv = cross(rd, e2);
    float det = dot(e1, pv);
    if (abs(det) <= TRI_PARALLEL_EPS) {
        return false;   // ray is parallel to (or degenerate on) the triangle plane
    }
    float inv_det = 1.0 / det;
    vec3 tv = ro - tri.v0.xyz;
    float u = dot(tv, pv) * inv_det;
    if (u < 0.0 || u > 1.0) {
        return false;
    }
    vec3 qv = cross(tv, e1);
    float v = dot(rd, qv) * inv_det;
    if (v < 0.0 || u + v > 1.0) {
        return false;
    }
    float t = dot(e2, qv) * inv_det;
    if (t < tmin || t >= tmax) {
        return false;
    }
    t_hit = t;
    return true;
}

vec3 triangle_normal(Triangle tri, vec3 rd) {
    vec3 e1 = tri.v1.xyz - tri.v0.xyz;
    vec3 e2 = tri.v2.xyz - tri.v0.xyz;
    vec3 n = normalize(cross(e1, e2));
    if (dot(n, rd) > 0.0) {
        n = -n;         // two-sided: always face the incoming ray
    }
    return n;
}

// --- scene traversal --------------------------------------------------------
// Nearest hit over every primitive inside [tmin, tmax].  Brute force (13
// primitives), spheres first, then triangles; strict `<` keeps the earlier
// primitive on an exact distance tie.
bool trace_scene(vec3 ro, vec3 rd, float tmin, float tmax, out Surface surf) {
    bool found = false;
    float best_t = tmax;

    for (uint i = 0u; i < scene.counts.x; ++i) {
        float t;
        if (intersect_sphere(ro, rd, tmin, best_t, spheres[i], t)) {
            best_t = t;
            found = true;
            surf.t = t;
            surf.prim_kind = PRIM_SPHERE;
            surf.prim_index = i;
            surf.position = ro + rd * t;
            surf.normal = normalize(surf.position - spheres[i].center_radius.xyz);
            if (dot(surf.normal, rd) > 0.0) {
                surf.normal = -surf.normal;
            }
            surf.base_color = spheres[i].base_color.rgb;
            surf.kd = spheres[i].params.x;
            surf.ks = spheres[i].params.y;
            surf.shininess = spheres[i].params.z;
            surf.reflectivity = spheres[i].params.w;
        }
    }

    for (uint i = 0u; i < scene.counts.y; ++i) {
        float t;
        if (intersect_triangle(ro, rd, tmin, best_t, triangles[i], t)) {
            best_t = t;
            found = true;
            surf.t = t;
            surf.prim_kind = PRIM_TRIANGLE;
            surf.prim_index = i;
            surf.position = ro + rd * t;
            surf.normal = triangle_normal(triangles[i], rd);
            surf.base_color = triangles[i].base_color.rgb;
            surf.kd = triangles[i].params.x;
            surf.ks = triangles[i].params.y;
            surf.shininess = triangles[i].params.z;
            surf.reflectivity = triangles[i].params.w;
        }
    }
    return found;
}

// Any-hit query used for hard shadows: returns true as soon as an occluder is
// found inside [tmin, tmax].  Callers limit tmax to the light distance, so
// geometry behind the light can never cast a shadow.
bool any_hit(vec3 ro, vec3 rd, float tmin, float tmax) {
    for (uint i = 0u; i < scene.counts.x; ++i) {
        float t;
        if (intersect_sphere(ro, rd, tmin, tmax, spheres[i], t)) {
            return true;
        }
    }
    for (uint i = 0u; i < scene.counts.y; ++i) {
        float t;
        if (intersect_triangle(ro, rd, tmin, tmax, triangles[i], t)) {
            return true;
        }
    }
    return false;
}

// --- direct lighting --------------------------------------------------------
// local = ambient + visibility * attenuation * (diffuse + specular)
vec3 shade_direct(Surface s, vec3 rd) {
    vec3 ambient = AMBIENT_SCALE * s.base_color;

    vec3 to_light = scene.light_position.xyz - s.position;
    float dist2 = dot(to_light, to_light);
    float dist = sqrt(dist2);
    if (dist <= 0.0) {
        return ambient;
    }
    vec3 l = to_light / dist;
    float ndl = dot(s.normal, l);
    if (ndl <= 0.0) {
        return ambient;     // light is behind the surface: no direct lighting at all
    }

    // Offset the shadow ray origin along the shading normal, and stop it just
    // short of the light, so the surface never shadows itself and geometry
    // behind the light never occludes.
    vec3 shadow_origin = s.position + s.normal * SECONDARY_ORIGIN_EPS;
    float shadow_tmax = dist - SHADOW_TMAX_SLACK;
    if (shadow_tmax <= RAY_T_MIN) {
        return ambient;
    }
    if (any_hit(shadow_origin, l, RAY_T_MIN, shadow_tmax)) {
        return ambient;     // ambient stays present in shadow
    }

    vec3 v = -rd;
    float specular = 0.0;
    vec3 h = l + v;
    float h_len = length(h);
    if (h_len > HALF_VECTOR_EPS) {     // zero-length half vector => zero specular
        h /= h_len;
        specular = s.ks * pow(max(dot(s.normal, h), 0.0), s.shininess);
    }

    vec3 diffuse = s.base_color * s.kd * ndl;
    vec3 specular_rgb = vec3(specular);
    float attenuation = 1.0 / dist2;
    return ambient + scene.light_intensity.rgb * attenuation * (diffuse + specular_rgb);
}

// --- iterative path tracing (no recursion) ----------------------------------
// One primary ray, then at most `scene.counts.w` reflection bounces.  At the
// bounce limit the untraced reflected contribution stays black; rays that miss
// the scene or whose remaining throughput is negligible terminate early.
vec3 trace_path(vec3 origin, vec3 dir) {
    vec3 radiance = vec3(0.0);
    vec3 throughput = vec3(1.0);
    vec3 ro = origin;
    vec3 rd = dir;

    for (uint bounce = 0u; bounce <= scene.counts.w; ++bounce) {
        Surface s;
        if (!trace_scene(ro, rd, RAY_T_MIN, RAY_T_MAX, s)) {
            break;      // miss: black background outside the room
        }
        vec3 local = shade_direct(s, rd);
        radiance += throughput * (1.0 - s.reflectivity) * local;

        if (s.reflectivity <= 0.0) {
            break;
        }
        if (bounce == scene.counts.w) {
            break;      // bounce limit: reflected contribution is untraced (black)
        }
        throughput *= s.reflectivity;
        if (max(max(throughput.r, throughput.g), throughput.b) < CONTRIBUTION_EPS) {
            break;      // remaining contribution is negligible
        }
        ro = s.position + s.normal * SECONDARY_ORIGIN_EPS;
        rd = reflect(rd, s.normal);
    }
    return radiance;
}

// --- camera ray -------------------------------------------------------------
vec3 camera_ray_dir(vec2 uv) {
    vec3 p = scene.cam_lower_left.xyz + uv.x * scene.cam_horizontal.xyz
           + uv.y * scene.cam_vertical.xyz;
    return normalize(p - scene.cam_origin.xyz);
}
