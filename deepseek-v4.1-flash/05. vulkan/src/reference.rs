//! CPU reference implementation of the GPU intersection and shading math.
//!
//! This is the *oracle* used by the intersection self-test: the GPU computes a
//! result, this module computes the same result on the CPU, and the test
//! compares them.  It intentionally mirrors `shaders/lib/common.glsl` step by
//! step (same primitive order, same strict comparisons, same tolerances) so that
//! any divergence points at a real GPU/host mismatch.  It is never used to
//! produce the rendered image.

use glam::Vec3;

use crate::scene::{GpuSphere, GpuTriangle, Scene};

// Tolerances, mirrored from shaders/lib/common.glsl.
pub const RAY_T_MIN: f32 = 1.0e-4;
pub const RAY_T_MAX: f32 = 1.0e30;
pub const SECONDARY_ORIGIN_EPS: f32 = 1.0e-4;
pub const SHADOW_TMAX_SLACK: f32 = 1.0e-3;
pub const AMBIENT_SCALE: f32 = 0.02;
pub const TRI_PARALLEL_EPS: f32 = 1.0e-12;
pub const HALF_VECTOR_EPS: f32 = 1.0e-6;
pub const CONTRIBUTION_EPS: f32 = 1.0e-6;

pub const PRIM_SPHERE: u32 = 0;
pub const PRIM_TRIANGLE: u32 = 1;

#[derive(Clone, Copy, Debug)]
pub struct RefSurface {
    pub t: f32,
    pub prim_kind: u32,
    pub prim_index: u32,
    pub position: Vec3,
    pub normal: Vec3,
    pub base_color: Vec3,
    pub kd: f32,
    pub ks: f32,
    pub shininess: f32,
    pub reflectivity: f32,
}

pub fn intersect_sphere(
    ro: Vec3,
    rd: Vec3,
    t_min: f32,
    t_max: f32,
    sphere: &GpuSphere,
) -> Option<f32> {
    let center = Vec3::new(
        sphere.center_radius[0],
        sphere.center_radius[1],
        sphere.center_radius[2],
    );
    let radius = sphere.center_radius[3];
    let oc = ro - center;
    let b = oc.dot(rd);
    let c = oc.dot(oc) - radius * radius;
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let sq = disc.sqrt();
    let t0 = -b - sq;
    let t1 = -b + sq;
    let mut t = t0;
    if t < t_min || t >= t_max {
        t = t1;
        if t < t_min || t >= t_max {
            return None;
        }
    }
    Some(t)
}

pub fn intersect_triangle(
    ro: Vec3,
    rd: Vec3,
    t_min: f32,
    t_max: f32,
    triangle: &GpuTriangle,
) -> Option<f32> {
    let v0 = Vec3::new(triangle.v0[0], triangle.v0[1], triangle.v0[2]);
    let v1 = Vec3::new(triangle.v1[0], triangle.v1[1], triangle.v1[2]);
    let v2 = Vec3::new(triangle.v2[0], triangle.v2[1], triangle.v2[2]);
    let e1 = v1 - v0;
    let e2 = v2 - v0;
    let pv = rd.cross(e2);
    let det = e1.dot(pv);
    if det.abs() <= TRI_PARALLEL_EPS {
        return None;
    }
    let inv_det = 1.0 / det;
    let tv = ro - v0;
    let u = tv.dot(pv) * inv_det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qv = tv.cross(e1);
    let v = rd.dot(qv) * inv_det;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(qv) * inv_det;
    if t < t_min || t >= t_max {
        return None;
    }
    Some(t)
}

pub fn triangle_normal(triangle: &GpuTriangle, rd: Vec3) -> Vec3 {
    let v0 = Vec3::new(triangle.v0[0], triangle.v0[1], triangle.v0[2]);
    let v1 = Vec3::new(triangle.v1[0], triangle.v1[1], triangle.v1[2]);
    let v2 = Vec3::new(triangle.v2[0], triangle.v2[1], triangle.v2[2]);
    let mut n = (v1 - v0).cross(v2 - v0).normalize();
    if n.dot(rd) > 0.0 {
        n = -n;
    }
    n
}

/// Nearest hit inside `[t_min, t_max]`; spheres before triangles, strict `<`
/// so that an exact distance tie keeps the earlier primitive.
pub fn trace_scene(
    scene: &Scene,
    ro: Vec3,
    rd: Vec3,
    t_min: f32,
    t_max: f32,
) -> Option<RefSurface> {
    let mut found: Option<RefSurface> = None;
    let mut best_t = t_max;

    for (index, sphere) in scene.spheres.iter().enumerate() {
        if let Some(t) = intersect_sphere(ro, rd, t_min, best_t, sphere) {
            best_t = t;
            let position = ro + rd * t;
            let center = Vec3::new(
                sphere.center_radius[0],
                sphere.center_radius[1],
                sphere.center_radius[2],
            );
            let mut normal = (position - center).normalize();
            if normal.dot(rd) > 0.0 {
                normal = -normal;
            }
            found = Some(RefSurface {
                t,
                prim_kind: PRIM_SPHERE,
                prim_index: index as u32,
                position,
                normal,
                base_color: Vec3::new(
                    sphere.base_color[0],
                    sphere.base_color[1],
                    sphere.base_color[2],
                ),
                kd: sphere.params[0],
                ks: sphere.params[1],
                shininess: sphere.params[2],
                reflectivity: sphere.params[3],
            });
        }
    }

    for (index, triangle) in scene.triangles.iter().enumerate() {
        if let Some(t) = intersect_triangle(ro, rd, t_min, best_t, triangle) {
            best_t = t;
            found = Some(RefSurface {
                t,
                prim_kind: PRIM_TRIANGLE,
                prim_index: index as u32,
                position: ro + rd * t,
                normal: triangle_normal(triangle, rd),
                base_color: Vec3::new(
                    triangle.base_color[0],
                    triangle.base_color[1],
                    triangle.base_color[2],
                ),
                kd: triangle.params[0],
                ks: triangle.params[1],
                shininess: triangle.params[2],
                reflectivity: triangle.params[3],
            });
        }
    }

    found
}

pub fn any_hit(scene: &Scene, ro: Vec3, rd: Vec3, t_min: f32, t_max: f32) -> bool {
    for sphere in &scene.spheres {
        if intersect_sphere(ro, rd, t_min, t_max, sphere).is_some() {
            return true;
        }
    }
    for triangle in &scene.triangles {
        if intersect_triangle(ro, rd, t_min, t_max, triangle).is_some() {
            return true;
        }
    }
    false
}

/// `local = ambient + visibility * attenuation * (diffuse + specular)`
pub fn shade_direct(scene: &Scene, surface: &RefSurface, rd: Vec3) -> Vec3 {
    let ambient = surface.base_color * AMBIENT_SCALE;

    let to_light = scene.light_position - surface.position;
    let dist2 = to_light.dot(to_light);
    let dist = dist2.sqrt();
    if dist <= 0.0 {
        return ambient;
    }
    let l = to_light / dist;
    let ndl = surface.normal.dot(l);
    if ndl <= 0.0 {
        return ambient;
    }

    let shadow_origin = surface.position + surface.normal * SECONDARY_ORIGIN_EPS;
    let shadow_tmax = dist - SHADOW_TMAX_SLACK;
    if shadow_tmax <= RAY_T_MIN {
        return ambient;
    }
    if any_hit(scene, shadow_origin, l, RAY_T_MIN, shadow_tmax) {
        return ambient;
    }

    let v = -rd;
    let mut specular = 0.0f32;
    let h = l + v;
    let h_len = h.length();
    if h_len > HALF_VECTOR_EPS {
        let h = h / h_len;
        specular = surface.ks * surface.normal.dot(h).max(0.0).powf(surface.shininess);
    }

    let diffuse = surface.base_color * surface.kd * ndl;
    let attenuation = 1.0 / dist2;
    let intensity = Vec3::new(
        scene.light_intensity.x,
        scene.light_intensity.y,
        scene.light_intensity.z,
    );
    ambient + intensity * attenuation * (diffuse + Vec3::splat(specular))
}

/// Iterative path tracing: one primary ray plus `max_reflections` bounces.
pub fn trace_path(scene: &Scene, origin: Vec3, dir: Vec3, max_reflections: u32) -> Vec3 {
    let mut radiance = Vec3::ZERO;
    let mut throughput = Vec3::ONE;
    let mut ro = origin;
    let mut rd = dir;

    let mut bounce = 0u32;
    while bounce <= max_reflections {
        let Some(surface) = trace_scene(scene, ro, rd, RAY_T_MIN, RAY_T_MAX) else {
            break;
        };
        let local = shade_direct(scene, &surface, rd);
        radiance += throughput * (1.0 - surface.reflectivity) * local;

        if surface.reflectivity <= 0.0 || bounce == max_reflections {
            break;
        }
        throughput *= surface.reflectivity;
        if throughput.max_element() < CONTRIBUTION_EPS {
            break;
        }
        ro = surface.position + surface.normal * SECONDARY_ORIGIN_EPS;
        rd = rd.reflect(surface.normal).normalize();
        bounce += 1;
    }
    radiance
}

/// CPU mirror of the synthetic-surface modes of `shaders/intersect_test.comp`:
/// a surface whose normal is the ray direction, with `kd = 0` so that only the
/// ambient and specular terms can contribute.
///
/// With `control = false` the half vector `L + V` is exactly zero (the specular
/// term must fall back to zero); with `control = true` the view direction is
/// perturbed so the half vector is short but non-zero and specular becomes
/// visible.
pub fn synthetic_half_vector_shade(scene: &Scene, ro: Vec3, control: bool) -> Vec3 {
    let l = (scene.light_position - ro).normalize();
    let mut surface = RefSurface {
        t: 0.0,
        prim_kind: PRIM_SPHERE,
        prim_index: 0,
        position: ro,
        normal: l,
        base_color: Vec3::ONE,
        kd: 0.0,
        ks: 1.0,
        shininess: 8.0,
        reflectivity: 0.0,
    };
    if control {
        let tangent = l.cross(Vec3::Z).normalize();
        let v = (l + tangent * 0.1).normalize();
        let h = (l + v).normalize();
        surface.normal = h;
        shade_direct(scene, &surface, -v)
    } else {
        shade_direct(scene, &surface, l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{GpuSphere, GpuTriangle, Scene};

    fn sphere_at(center: Vec3, radius: f32) -> GpuSphere {
        GpuSphere {
            center_radius: [center.x, center.y, center.z, radius],
            base_color: [1.0, 1.0, 1.0, 1.0],
            params: [1.0, 0.0, 1.0, 0.0],
        }
    }

    fn triangle(v0: Vec3, v1: Vec3, v2: Vec3) -> GpuTriangle {
        GpuTriangle {
            v0: [v0.x, v0.y, v0.z, 1.0],
            v1: [v1.x, v1.y, v1.z, 1.0],
            v2: [v2.x, v2.y, v2.z, 1.0],
            base_color: [1.0, 1.0, 1.0, 1.0],
            params: [1.0, 0.0, 1.0, 0.0],
        }
    }

    fn scene_with(spheres: Vec<GpuSphere>, triangles: Vec<GpuTriangle>) -> Scene {
        Scene {
            spheres,
            triangles,
            light_position: Vec3::new(0.0, 5.5, 1.0),
            light_intensity: Vec3::splat(60.0),
        }
    }

    /// Two coincident primitives produce bit-identical ray parameters, so the
    /// strict `<` comparison must keep the earlier one: this is the
    /// deterministic tie-break the renderer relies on.
    #[test]
    fn identical_spheres_keep_the_lower_index() {
        let scene = scene_with(
            vec![
                sphere_at(Vec3::new(0.0, 1.0, 0.0), 1.0),
                sphere_at(Vec3::new(0.0, 1.0, 0.0), 1.0),
            ],
            vec![],
        );
        let hit = trace_scene(
            &scene,
            Vec3::new(0.0, 1.0, 10.0),
            Vec3::new(0.0, 0.0, -1.0),
            RAY_T_MIN,
            RAY_T_MAX,
        )
        .expect("hit");
        assert_eq!(hit.prim_kind, PRIM_SPHERE);
        assert_eq!(hit.prim_index, 0);
    }

    #[test]
    fn identical_triangles_keep_the_lower_index() {
        let scene = scene_with(
            vec![],
            vec![
                triangle(
                    Vec3::new(-1.0, 0.0, -1.0),
                    Vec3::new(1.0, 0.0, -1.0),
                    Vec3::new(1.0, 0.0, 1.0),
                ),
                triangle(
                    Vec3::new(-1.0, 0.0, -1.0),
                    Vec3::new(1.0, 0.0, -1.0),
                    Vec3::new(1.0, 0.0, 1.0),
                ),
            ],
        );
        let hit = trace_scene(
            &scene,
            Vec3::new(0.5, 5.0, 0.5),
            Vec3::new(0.0, -1.0, 0.0),
            RAY_T_MIN,
            RAY_T_MAX,
        )
        .expect("hit");
        assert_eq!(hit.prim_kind, PRIM_TRIANGLE);
        assert_eq!(hit.prim_index, 0);
    }

    /// A sphere and a triangle at the same distance: spheres are tested first,
    /// so the sphere wins the tie.
    #[test]
    fn spheres_win_ties_against_triangles() {
        let scene = scene_with(
            vec![sphere_at(Vec3::new(0.0, 0.0, 0.0), 1.0)],
            vec![triangle(
                Vec3::new(-2.0, 0.0, -2.0),
                Vec3::new(2.0, 0.0, -2.0),
                Vec3::new(2.0, 0.0, 2.0),
            )],
        );
        // Ray from (0, 5, 0) straight down: the sphere top is at y = 1, the
        // triangle plane at y = 0 -> sphere is nearer, and at equal distance it
        // would still win because it is tested first.
        let hit = trace_scene(
            &scene,
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            RAY_T_MIN,
            RAY_T_MAX,
        )
        .expect("hit");
        assert_eq!(hit.prim_kind, PRIM_SPHERE);
    }

    #[test]
    fn rays_inside_spheres_report_the_exit_point() {
        let scene = scene_with(vec![sphere_at(Vec3::new(0.0, 0.0, 0.0), 2.0)], vec![]);
        let hit = trace_scene(
            &scene,
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 1.0),
            RAY_T_MIN,
            RAY_T_MAX,
        )
        .expect("hit");
        assert!((hit.t - 2.0).abs() < 1e-6, "{}", hit.t);
    }

    #[test]
    fn tangent_rays_hit_exactly_once() {
        let scene = scene_with(vec![sphere_at(Vec3::ZERO, 1.0)], vec![]);
        let hit = trace_scene(
            &scene,
            Vec3::new(0.0, 1.0, -5.0),
            Vec3::new(0.0, 0.0, 1.0),
            RAY_T_MIN,
            RAY_T_MAX,
        )
        .expect("tangent hit");
        assert!((hit.t - 5.0).abs() < 1e-5, "{}", hit.t);
    }

    #[test]
    fn shadow_rays_stop_at_the_light() {
        // A blocker placed beyond the light must not occlude the sample point.
        let mut scene = scene_with(vec![], vec![]);
        scene.light_position = Vec3::new(0.0, 1.0, 0.0);
        scene.spheres = vec![sphere_at(Vec3::new(0.0, 3.0, 0.0), 1.0)];
        let surface = RefSurface {
            t: 0.0,
            prim_kind: PRIM_TRIANGLE,
            prim_index: 0,
            position: Vec3::ZERO,
            normal: Vec3::Y,
            base_color: Vec3::ONE,
            kd: 1.0,
            ks: 0.0,
            shininess: 1.0,
            reflectivity: 0.0,
        };
        let shaded = shade_direct(&scene, &surface, -Vec3::Y);
        let ambient = surface.base_color * AMBIENT_SCALE;
        assert!(shaded.length() > ambient.length(), "{shaded:?}");
    }
}
