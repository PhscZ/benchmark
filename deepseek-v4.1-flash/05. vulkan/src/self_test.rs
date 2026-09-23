//! GPU verification suite.
//!
//! Three independent checks, all of which exercise the real GPU code paths:
//!
//! 1. **Intersection tests** - a table of known rays is dispatched through
//!    `shaders/intersect_test.comp` (which shares its intersection/shading code
//!    with the production shader) and the results are read back and compared
//!    against an independent CPU implementation (`crate::reference`).
//! 2. **Pixel reference check** - a 1 spp render is read back and compared, per
//!    pixel, against the CPU reference evaluated with the identical camera ray.
//! 3. **Reproducibility** - the same settings and seed are rendered twice; the
//!    decoded RGBA8 pixels must hash identically.  Exposure changes must reuse
//!    the accumulated linear samples.

use glam::Vec3;

use crate::camera::Camera;
use crate::color;
use crate::error::Result;
use crate::reference::{self, RefSurface};
use crate::render::{Renderer, TestRay, DEFAULT_HEIGHT, DEFAULT_WIDTH};
use crate::scene::Scene;

const MODE_INTERSECT: u32 = 0;
const MODE_PATH: u32 = 1;
const MODE_SHADE: u32 = 2;
const MODE_HALF_VECTOR: u32 = 3;
const MODE_HALF_VECTOR_CONTROL: u32 = 4;

const T_ABS_TOL: f32 = 1.0e-3;
const T_REL_TOL: f32 = 1.0e-4;
const VEC_ABS_TOL: f32 = 3.0e-3;
const COLOR_ABS_TOL: f32 = 4.0e-3;
const COLOR_REL_TOL: f32 = 5.0e-3;

#[derive(Clone, Debug)]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default)]
pub struct SelfTestReport {
    pub checks: Vec<CheckResult>,
}

impl SelfTestReport {
    pub fn passed(&self) -> bool {
        self.checks.iter().all(|c| c.passed)
    }

    pub fn failures(&self) -> usize {
        self.checks.iter().filter(|c| !c.passed).count()
    }

    pub fn push(&mut self, name: impl Into<String>, passed: bool, detail: impl Into<String>) {
        self.checks.push(CheckResult {
            name: name.into(),
            passed,
            detail: detail.into(),
        });
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "self-test: {} checks, {} failed\n",
            self.checks.len(),
            self.failures()
        ));
        for check in &self.checks {
            out.push_str(&format!(
                "  [{}] {}: {}\n",
                if check.passed { "PASS" } else { "FAIL" },
                check.name,
                check.detail
            ));
        }
        out
    }
}

/// What the CPU oracle is expected to report for a case (validates the test
/// itself, so a case cannot silently stop covering what it claims to cover).
#[derive(Clone, Debug, PartialEq)]
enum Expect {
    Any,
    /// A hit on this primitive kind and index (validated against the oracle).
    Hit(u32, u32),
    /// A hit on this primitive kind, index unchecked.
    Kind(u32),
    /// A hit on one of these (kind, index) candidates: used for hits on shared
    /// edges / tangency points, where which of the two coincident primitives
    /// wins depends on floating-point rounding.  The tie-break rule itself is
    /// verified exactly by the unit tests in `crate::reference`.
    AnyOf(Vec<(u32, u32)>),
    Miss,
}

struct Case {
    name: String,
    ray: TestRay,
    expect: Expect,
    /// True for the synthetic half-vector cases, which do not trace the scene.
    synthetic: bool,
}

fn ray(origin: Vec3, dir: Vec3, t_min: f32, t_max: f32, mode: u32) -> TestRay {
    let dir = dir.normalize();
    TestRay {
        origin: [origin.x, origin.y, origin.z, 1.0],
        dir: [dir.x, dir.y, dir.z, 0.0],
        t_min_max: [t_min, t_max, mode as f32, 0.0],
    }
}

fn close(a: f32, b: f32, abs_tol: f32, rel_tol: f32) -> bool {
    let diff = (a - b).abs();
    diff <= abs_tol || diff <= rel_tol * a.abs().max(b.abs())
}

fn close_vec(a: [f32; 3], b: Vec3, abs_tol: f32, rel_tol: f32) -> bool {
    close(a[0], b.x, abs_tol, rel_tol)
        && close(a[1], b.y, abs_tol, rel_tol)
        && close(a[2], b.z, abs_tol, rel_tol)
}

/// Runs the whole verification suite and restores the previous settings.
pub fn run(renderer: &mut Renderer) -> Result<SelfTestReport> {
    let mut report = SelfTestReport::default();
    let scene = Scene::fixed();
    let saved = renderer.settings();
    let saved_camera = *renderer.camera();

    intersection_checks(renderer, &scene, &mut report)?;
    pixel_reference_check(renderer, &scene, &mut report)?;
    reproducibility_check(renderer, &mut report)?;
    resource_lifetime_check(renderer, &mut report)?;

    // Restore state so the interactive session continues unaffected.
    renderer.set_samples_per_pixel(saved.samples_per_pixel)?;
    renderer.set_reflections(saved.max_reflections)?;
    renderer.set_exposure(saved.exposure);
    renderer.set_seed(saved.seed)?;
    *renderer.camera_mut() = saved_camera;
    renderer.restart()?;
    Ok(report)
}

fn intersection_checks(
    renderer: &mut Renderer,
    scene: &Scene,
    report: &mut SelfTestReport,
) -> Result<()> {
    let camera = Camera::default();
    let max_reflections = renderer.settings().max_reflections;
    let light = scene.light_position;

    // Capacity covers the explicit cases plus the camera-ray grid added below.
    let mut cases: Vec<Case> = Vec::with_capacity(128);

    // --- explicit analytic cases ------------------------------------------
    cases.push(Case {
        name: "ray-sphere front hit".to_string(),
        ray: ray(Vec3::new(0.0, 1.0, 10.0), Vec3::new(0.0, 0.0, -1.0), reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_INTERSECT),
        expect: Expect::Hit(reference::PRIM_SPHERE, 2),
        synthetic: false,
    });
    cases.push(Case {
        name: "ray originating inside a sphere (exit hit)".to_string(),
        ray: ray(Vec3::new(0.0, 0.75, 1.0), Vec3::new(0.0, 0.0, 1.0), reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_INTERSECT),
        expect: Expect::Hit(reference::PRIM_SPHERE, 2),
        synthetic: false,
    });
    cases.push(Case {
        name: "tangent ray (discriminant == 0)".to_string(),
        ray: ray(Vec3::new(0.0, 1.5, 10.0), Vec3::new(0.0, 0.0, -1.0), reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_INTERSECT),
        expect: Expect::Hit(reference::PRIM_SPHERE, 2),
        synthetic: false,
    });
    cases.push(Case {
        name: "miss (black background)".to_string(),
        ray: ray(Vec3::new(0.0, 1.0, 10.0), Vec3::new(0.0, 1.0, 0.0), reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_INTERSECT),
        expect: Expect::Miss,
        synthetic: false,
    });
    cases.push(Case {
        name: "shared-edge hit on the floor diagonal (coincident triangles)".to_string(),
        ray: ray(Vec3::new(0.0, 5.0, 0.0), Vec3::new(0.0, -1.0, 0.0), reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_INTERSECT),
        expect: Expect::AnyOf(vec![
            (reference::PRIM_TRIANGLE, 0),
            (reference::PRIM_TRIANGLE, 1),
        ]),
        synthetic: false,
    });
    cases.push(Case {
        name: "tangency point: mirror sphere touches the floor (coincident primitives)".to_string(),
        ray: ray(Vec3::new(0.0, 1.0, 1.0), Vec3::new(0.0, -1.0, 0.0), reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_INTERSECT),
        expect: Expect::AnyOf(vec![
            (reference::PRIM_SPHERE, 2),
            (reference::PRIM_TRIANGLE, 1),
        ]),
        synthetic: false,
    });
    cases.push(Case {
        name: "two-sided triangle from below (normal flipped against the ray)".to_string(),
        ray: ray(Vec3::new(-1.0, -1.0, 2.0), Vec3::new(0.0, 1.0, 0.0), reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_INTERSECT),
        expect: Expect::Hit(reference::PRIM_TRIANGLE, 1),
        synthetic: false,
    });
    cases.push(Case {
        name: "t_max rejection (hit beyond interval)".to_string(),
        ray: ray(Vec3::new(0.0, 1.0, 10.0), Vec3::new(0.0, 0.0, -1.0), reference::RAY_T_MIN, 0.1, MODE_INTERSECT),
        expect: Expect::Miss,
        synthetic: false,
    });
    cases.push(Case {
        name: "t_min rejection (entry before interval, exit accepted)".to_string(),
        ray: ray(Vec3::new(0.0, 1.0, 10.0), Vec3::new(0.0, 0.0, -1.0), 9.5, reference::RAY_T_MAX, MODE_INTERSECT),
        expect: Expect::Hit(reference::PRIM_SPHERE, 2),
        synthetic: false,
    });
    cases.push(Case {
        name: "parallel ray (floor/ceiling rejected, wall hit)".to_string(),
        ray: ray(Vec3::new(0.0, 3.0, 0.0), Vec3::new(1.0, 0.0, 0.0), reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_INTERSECT),
        // The floor and ceiling planes are parallel to this ray, so they are
        // rejected; the hit is the first triangle of the right wall.
        expect: Expect::Hit(reference::PRIM_TRIANGLE, 8),
        synthetic: false,
    });

    // --- shadow ray toward the light --------------------------------------
    // A floor point that sees the light directly (verified below by the oracle).
    let lit_point = Vec3::new(-2.0, 0.0, 2.0);
    let to_light = light - lit_point;
    let distance = to_light.length();
    let shadow_ray = ray(
        lit_point + Vec3::new(0.0, 1.0, 0.0) * reference::SECONDARY_ORIGIN_EPS,
        to_light,
        reference::RAY_T_MIN,
        distance - reference::SHADOW_TMAX_SLACK,
        MODE_INTERSECT,
    );
    cases.push(Case {
        name: "shadow ray limited to the light distance (clear)".to_string(),
        ray: shadow_ray,
        expect: Expect::Miss,
        synthetic: false,
    });

    // A point in the gold sphere's shadow, found with the CPU oracle so the
    // case is guaranteed to be occluded.
    if let Some(point) = find_shadowed_floor_point(scene) {
        let normal = Vec3::new(0.0, 1.0, 0.0);
        let origin = point + normal * 0.5;
        let to_light = light - point;
        let distance = to_light.length();
        let occluded = ray(
            point + normal * reference::SECONDARY_ORIGIN_EPS,
            to_light,
            reference::RAY_T_MIN,
            distance - reference::SHADOW_TMAX_SLACK,
            MODE_INTERSECT,
        );
        cases.push(Case {
            name: "shadow ray occluded by the gold sphere".to_string(),
            ray: occluded,
            expect: Expect::Hit(reference::PRIM_SPHERE, 1),
            synthetic: false,
        });
        // Shaded sample in shadow: only ambient may remain.
        cases.push(Case {
            name: "shadowed surface sample (ambient only)".to_string(),
            ray: ray(origin, -normal, reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_SHADE),
            expect: Expect::Kind(reference::PRIM_TRIANGLE),
            synthetic: false,
        });
    }
    // A lit floor sample for contrast.
    let lit_origin = lit_point + Vec3::new(0.0, 0.5, 0.0);
    cases.push(Case {
        name: "lit surface sample (diffuse + shadow visibility)".to_string(),
        ray: ray(lit_origin, Vec3::new(0.0, -1.0, 0.0), reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_SHADE),
        expect: Expect::Kind(reference::PRIM_TRIANGLE),
        synthetic: false,
    });

    // --- zero-length half vector ------------------------------------------
    let half_vector_origin = lit_point;
    let toward_light = (light - half_vector_origin).normalize();
    cases.push(Case {
        name: "zero-length half vector -> zero specular (ambient only)".to_string(),
        ray: ray(half_vector_origin, toward_light, reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_HALF_VECTOR),
        expect: Expect::Any,
        synthetic: true,
    });
    cases.push(Case {
        name: "half-vector control (short but non-zero -> specular > 0)".to_string(),
        ray: ray(half_vector_origin, toward_light, reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_HALF_VECTOR_CONTROL),
        expect: Expect::Any,
        synthetic: true,
    });

    // --- explicit reflection cases ----------------------------------------
    let camera_position = camera.position();
    cases.push(Case {
        name: "mirror sphere: full reflection path".to_string(),
        // Aimed at the upper front of the sphere so that the reflected ray
        // travels into the room instead of out of the open front.
        ray: ray(camera_position, Vec3::new(0.0, 1.275, 1.525) - camera_position, reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_PATH),
        expect: Expect::Hit(reference::PRIM_SPHERE, 2),
        synthetic: false,
    });
    cases.push(Case {
        name: "glossy gold sphere: specular + reflection path".to_string(),
        ray: ray(camera_position, Vec3::new(1.4, 2.0, -1.0) - camera_position, reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_PATH),
        expect: Expect::Hit(reference::PRIM_SPHERE, 1),
        synthetic: false,
    });
    cases.push(Case {
        name: "matte blue sphere: no reflection contribution".to_string(),
        ray: ray(camera_position, Vec3::new(-1.6, 2.0, -0.8) - camera_position, reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_PATH),
        expect: Expect::Hit(reference::PRIM_SPHERE, 0),
        synthetic: false,
    });

    // --- camera ray grids (intersection, shading and full paths) -----------
    let mut grid_intersect = 0usize;
    for grid_y in 0..5 {
        for grid_x in 0..5 {
            let uv = Vec3::new(
                (grid_x as f32 + 0.5) / 5.0,
                (grid_y as f32 + 0.5) / 5.0,
                0.0,
            );
            let dir = camera_direction(&camera, DEFAULT_WIDTH, DEFAULT_HEIGHT, uv.x, uv.y);
            let origin = camera.position();
            cases.push(Case {
                name: format!("camera grid intersect {grid_x},{grid_y}"),
                ray: ray(origin, dir, reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_INTERSECT),
                expect: Expect::Any,
                synthetic: false,
            });
            grid_intersect += 1;
            cases.push(Case {
                name: format!("camera grid shade {grid_x},{grid_y}"),
                ray: ray(origin, dir, reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_SHADE),
                expect: Expect::Any,
                synthetic: false,
            });
            cases.push(Case {
                name: format!("camera grid path {grid_x},{grid_y}"),
                ray: ray(origin, dir, reference::RAY_T_MIN, reference::RAY_T_MAX, MODE_PATH),
                expect: Expect::Any,
                synthetic: false,
            });
        }
    }

    let rays: Vec<TestRay> = cases.iter().map(|c| c.ray).collect();
    let gpu = renderer.run_intersection_tests(&rays)?;
    if gpu.len() != cases.len() {
        report.push(
            "intersection dispatch returned one result per ray",
            false,
            format!("expected {} results, got {}", cases.len(), gpu.len()),
        );
        return Ok(());
    }

    // Dispatching the same ray table again must reproduce every result
    // bit-for-bit (deterministic primitive ordering, no races).
    let gpu_repeat = renderer.run_intersection_tests(&rays)?;
    let mut repeat_mismatch: Option<String> = None;
    for (index, (first, second)) in gpu.iter().zip(gpu_repeat.iter()).enumerate() {
        let same = first.hit == second.hit
            && first.t.to_bits() == second.t.to_bits()
            && first.prim_kind == second.prim_kind
            && first.prim_index == second.prim_index
            && first.any_hit == second.any_hit
            && first.position.map(f32::to_bits) == second.position.map(f32::to_bits)
            && first.normal.map(f32::to_bits) == second.normal.map(f32::to_bits)
            && first.color.map(f32::to_bits) == second.color.map(f32::to_bits);
        if !same && repeat_mismatch.is_none() {
            repeat_mismatch = Some(format!(
                "ray {index} ({}): {:?} vs {:?}",
                cases[index].name, first, second
            ));
        }
    }
    report.push(
        "repeated intersection dispatch is bit-identical",
        repeat_mismatch.is_none(),
        repeat_mismatch.unwrap_or_else(|| {
            format!("{} rays dispatched twice with identical results", rays.len())
        }),
    );

    let mut coverage = Coverage::default();
    let mut mismatches: Vec<String> = Vec::new();
    let mut checks_run = 0usize;

    for (case, result) in cases.iter().zip(gpu.iter()) {
        let ro = Vec3::new(case.ray.origin[0], case.ray.origin[1], case.ray.origin[2]);
        let rd = Vec3::new(case.ray.dir[0], case.ray.dir[1], case.ray.dir[2]);
        let t_min = case.ray.t_min_max[0];
        let t_max = case.ray.t_min_max[1];
        let mode = case.ray.t_min_max[2] as u32;

        if case.synthetic {
            let control = mode == MODE_HALF_VECTOR_CONTROL;
            let expected = reference::synthetic_half_vector_shade(scene, ro, control);
            if !close_vec(result.color, expected, COLOR_ABS_TOL, COLOR_REL_TOL) {
                mismatches.push(format!(
                    "{}: gpu {:?} vs cpu {:?}",
                    case.name, result.color, expected
                ));
            }
            let ambient = Vec3::splat(reference::AMBIENT_SCALE);
            if control {
                if expected.dot(Vec3::ONE) <= ambient.dot(Vec3::ONE) + 1.0e-6 {
                    mismatches.push(format!(
                        "{}: control case produced no specular ({expected:?})",
                        case.name
                    ));
                }
            } else if (expected - ambient).length() > 1.0e-6 {
                mismatches.push(format!(
                    "{}: expected ambient only for the zero half vector, got {expected:?}",
                    case.name
                ));
            }
            checks_run += 1;
            continue;
        }

        let oracle: Option<RefSurface> = reference::trace_scene(scene, ro, rd, t_min, t_max);
        let oracle_any_hit = reference::any_hit(scene, ro, rd, t_min, t_max);

        match &case.expect {
            Expect::Any => {}
            Expect::Hit(kind, index) => match oracle {
                Some(surface) => {
                    if surface.prim_kind != *kind || surface.prim_index != *index {
                        mismatches.push(format!(
                            "{}: oracle reports kind {} index {}, test expects kind {} index {}",
                            case.name, surface.prim_kind, surface.prim_index, kind, index
                        ));
                    }
                }
                None => mismatches.push(format!("{}: oracle reports a miss", case.name)),
            },
            Expect::AnyOf(candidates) => match oracle {
                Some(surface) => {
                    let found = (surface.prim_kind, surface.prim_index);
                    if !candidates.contains(&found) {
                        mismatches.push(format!(
                            "{}: oracle reports {found:?}, which is not one of the coincident candidates {candidates:?}",
                            case.name
                        ));
                    }
                }
                None => mismatches.push(format!("{}: oracle reports a miss", case.name)),
            },
            Expect::Kind(kind) => match oracle {
                Some(surface) => {
                    if surface.prim_kind != *kind {
                        mismatches.push(format!(
                            "{}: oracle reports kind {}, test expects kind {}",
                            case.name, surface.prim_kind, kind
                        ));
                    }
                }
                None => mismatches.push(format!("{}: oracle reports a miss", case.name)),
            },
            Expect::Miss => {
                if oracle.is_some() {
                    mismatches.push(format!(
                        "{}: oracle reports a hit but the test expects a miss",
                        case.name
                    ));
                }
            }
        }

        // GPU vs CPU.
        match (&oracle, result.hit) {
            (None, true) => mismatches.push(format!(
                "{}: gpu reports a hit at t={} but the cpu reports a miss",
                case.name, result.t
            )),
            (Some(_), false) => {
                mismatches.push(format!("{}: gpu reports a miss but the cpu reports a hit", case.name))
            }
            (Some(surface), true) => {
                if !close(result.t, surface.t, T_ABS_TOL, T_REL_TOL) {
                    mismatches.push(format!(
                        "{}: t gpu {} vs cpu {}",
                        case.name, result.t, surface.t
                    ));
                }
                if result.prim_kind != surface.prim_kind || result.prim_index != surface.prim_index {
                    mismatches.push(format!(
                        "{}: primitive gpu {}:{} vs cpu {}:{}",
                        case.name, result.prim_kind, result.prim_index, surface.prim_kind, surface.prim_index
                    ));
                }
                if !close_vec(result.position, surface.position, VEC_ABS_TOL, T_REL_TOL) {
                    mismatches.push(format!(
                        "{}: position gpu {:?} vs cpu {:?}",
                        case.name, result.position, surface.position
                    ));
                }
                if !close_vec(result.normal, surface.normal, VEC_ABS_TOL, T_REL_TOL) {
                    mismatches.push(format!(
                        "{}: normal gpu {:?} vs cpu {:?}",
                        case.name, result.normal, surface.normal
                    ));
                }
                if mode == MODE_SHADE {
                    let expected = reference::shade_direct(scene, surface, rd);
                    if !close_vec(result.color, expected, COLOR_ABS_TOL, COLOR_REL_TOL) {
                        mismatches.push(format!(
                            "{}: shade gpu {:?} vs cpu {:?}",
                            case.name, result.color, expected
                        ));
                    }
                    let ambient = surface.base_color * reference::AMBIENT_SCALE;
                    if (expected - ambient).length() < 1.0e-6 {
                        coverage.shadowed_shade += 1;
                    } else {
                        coverage.lit_shade += 1;
                    }
                }
                if mode == MODE_PATH {
                    let expected = reference::trace_path(scene, ro, rd, max_reflections);
                    if !close_vec(result.color, expected, COLOR_ABS_TOL, COLOR_REL_TOL) {
                        mismatches.push(format!(
                            "{}: path gpu {:?} vs cpu {:?}",
                            case.name, result.color, expected
                        ));
                    }
                    // Did the reflection actually contribute?
                    let local_only = (1.0 - surface.reflectivity)
                        * reference::shade_direct(scene, surface, rd);
                    if (expected - local_only).length() > 1.0e-4 {
                        coverage.reflective_path += 1;
                    }
                }
                coverage.hits += 1;
                if surface.prim_kind == reference::PRIM_SPHERE {
                    coverage.sphere_hits += 1;
                } else {
                    coverage.triangle_hits += 1;
                }
            }
            (None, false) => coverage.misses += 1,
        }

        if result.any_hit != oracle_any_hit {
            mismatches.push(format!(
                "{}: any_hit gpu {} vs cpu {}",
                case.name, result.any_hit, oracle_any_hit
            ));
        }
        if mode != MODE_INTERSECT {
            // any_hit is only meaningful for the tracing modes.
            coverage.any_hit_checks += 1;
        }
        checks_run += 1;
    }

    report.push(
        "GPU intersection tests match the CPU reference",
        mismatches.is_empty(),
        if mismatches.is_empty() {
            format!("{checks_run} rays verified ({} camera grid rays)", grid_intersect * 3)
        } else {
            format!("{} mismatch(es): {}", mismatches.len(), mismatches.join(" | "))
        },
    );
    report.push(
        "ray case coverage",
        coverage.shadowed_shade > 0
            && coverage.lit_shade > 0
            && coverage.sphere_hits > 0
            && coverage.triangle_hits > 0
            && coverage.misses > 0
            && coverage.reflective_path > 0,
        format!("{coverage:?}"),
    );
    Ok(())
}

#[derive(Default, Debug)]
struct Coverage {
    hits: usize,
    sphere_hits: usize,
    triangle_hits: usize,
    misses: usize,
    shadowed_shade: usize,
    lit_shade: usize,
    reflective_path: usize,
    any_hit_checks: usize,
}

/// Scans the floor with the CPU oracle for a point that the gold sphere shadows.
fn find_shadowed_floor_point(scene: &Scene) -> Option<Vec3> {
    let normal = Vec3::new(0.0, 1.0, 0.0);
    let mut z = -2.8f32;
    while z <= 2.8 {
        let mut x = -2.8f32;
        while x <= 2.8 {
            let point = Vec3::new(x, 0.0, z);
            let to_light = scene.light_position - point;
            let distance = to_light.length();
            if distance > reference::RAY_T_MIN {
                let origin = point + normal * reference::SECONDARY_ORIGIN_EPS;
                let occluded = reference::any_hit(
                    scene,
                    origin,
                    to_light.normalize(),
                    reference::RAY_T_MIN,
                    distance - reference::SHADOW_TMAX_SLACK,
                );
                // The sample must also be reachable straight down from above.
                let probe_origin = point + normal * 0.5;
                let reachable = reference::trace_scene(
                    scene,
                    probe_origin,
                    -normal,
                    reference::RAY_T_MIN,
                    reference::RAY_T_MAX,
                )
                .map(|s| (s.position - point).length() < 1.0e-3)
                .unwrap_or(false);
                if occluded && reachable {
                    return Some(point);
                }
            }
            x += 0.05;
        }
        z += 0.05;
    }
    None
}

fn camera_direction(camera: &Camera, width: u32, height: u32, u: f32, v: f32) -> Vec3 {
    let basis = camera.basis(width, height);
    // `v` is measured from the top of the image, matching the shader.
    let point = basis.lower_left + basis.horizontal * u + basis.vertical * (1.0 - v);
    (point - basis.origin).normalize()
}

/// Renders 1 spp and compares pixels against the CPU reference traced with the
/// identical camera ray (center-of-pixel sampling for the 1 spp preset).
fn pixel_reference_check(
    renderer: &mut Renderer,
    scene: &Scene,
    report: &mut SelfTestReport,
) -> Result<()> {
    let camera = *renderer.camera();
    renderer.set_samples_per_pixel(1)?;
    renderer.restart()?;
    renderer.set_headless_extent(DEFAULT_WIDTH, DEFAULT_HEIGHT);
    let stats = renderer.dispatch_batches(1, 1, false)?;
    if stats.dispatched_samples != 1 {
        report.push(
            "1 spp render dispatched exactly one sample",
            false,
            format!("{stats:?}"),
        );
        return Ok(());
    }
    let sums = renderer.read_accum_rgba32f()?;
    let (width, height) = (DEFAULT_WIDTH, DEFAULT_HEIGHT);
    let max_reflections = renderer.settings().max_reflections;

    let mut probes: Vec<(u32, u32)> = vec![
        (width / 2, height / 2),
        (width / 4, height / 4),
        (width / 4 * 3, height / 4),
        (width / 4, height / 4 * 3),
        (width / 4 * 3, height / 4 * 3),
        (width / 2, height / 4),
        (width / 2, height / 4 * 3),
        (width / 4, height / 2),
        (width / 4 * 3, height / 2),
    ];
    // Add probe pixels that actually hit a sphere (so reflections and specular
    // shading are covered by the per-pixel comparison as well).
    let basis = camera.basis(width, height);
    let mut found = 0usize;
    'scan: for y in (height / 2..height).step_by(16) {
        for x in (0..width).step_by(16) {
            let u = (x as f32 + 0.5) / width as f32;
            let v = (y as f32 + 0.5) / height as f32;
            let dir = camera_direction(&camera, width, height, u, v);
            if let Some(hit) = reference::trace_scene(scene, basis.origin, dir, reference::RAY_T_MIN, reference::RAY_T_MAX)
            {
                if hit.prim_kind == reference::PRIM_SPHERE {
                    probes.push((x, y));
                    found += 1;
                    if found >= 4 {
                        break 'scan;
                    }
                }
            }
        }
    }

    let mut worst = 0.0f32;
    let mut mismatches = Vec::new();
    for (x, y) in probes.iter().copied() {
        let u = (x as f32 + 0.5) / width as f32;
        let v = (y as f32 + 0.5) / height as f32;
        let basis = camera.basis(width, height);
        let dir = camera_direction(&camera, width, height, u, v);
        let expected = reference::trace_path(scene, basis.origin, dir, max_reflections);
        let index = ((y * width + x) * 4) as usize;
        let actual = Vec3::new(sums[index], sums[index + 1], sums[index + 2]);
        let diff = (actual - expected).length();
        worst = worst.max(diff);
        if diff > 2.0e-2 {
            mismatches.push(format!("pixel ({x},{y}): gpu {actual:?} vs cpu {expected:?}"));
        }
    }
    report.push(
        "1 spp render matches the CPU reference per pixel",
        mismatches.is_empty(),
        if mismatches.is_empty() {
            format!(
                "{} probe pixels ({} on spheres), worst |gpu-cpu| = {worst:.2e}",
                probes.len(),
                found
            )
        } else {
            mismatches.join(" | ")
        },
    );
    Ok(())
}

/// Repeats the operations that recreate GPU resources - render-resolution
/// changes (the same path a window resize takes: swapchain-independent
/// accumulation image + readback buffer recreation, descriptor updates,
/// accumulation clear) and render restarts - and verifies that the renderer's
/// live Vulkan object counts return exactly to the baseline.
///
/// This is the machine-checked form of "repeated resizing and render restarts
/// must not leak GPU resources".
fn resource_lifetime_check(renderer: &mut Renderer, report: &mut SelfTestReport) -> Result<()> {
    use crate::vk::ResourceCounts;

    const ROUNDS: usize = 12;
    // Sizes that force both growth and shrink of the accumulation image.
    const SIZES: [(u32, u32); 4] = [(640, 360), (1280, 720), (800, 450), (1024, 576)];

    let (width, height) = (renderer.accum_extent().width, renderer.accum_extent().height);
    let windowed = renderer.has_swapchain();
    renderer.wait_idle()?;
    let before = renderer.resource_counts();
    let baseline_total: i64 = before.iter().map(|(_, count)| *count).sum();
    let mut peak_total = baseline_total;

    for round in 0..ROUNDS {
        let (w, h) = SIZES[round % SIZES.len()];
        if windowed {
            // Full path: surface resize -> swapchain recreation -> accumulation
            // image recreation -> present.
            renderer.resize_and_present(w, h)?;
        }
        renderer.set_render_resolution(w, h)?;
        // Exercise the allocation path for the frame-scoped buffers too.
        renderer.dispatch_batches(2, 1, false)?;
        renderer.restart()?;
        renderer.wait_idle()?;
        let total: i64 = renderer
            .resource_counts()
            .iter()
            .map(|(_, count)| *count)
            .sum();
        peak_total = peak_total.max(total);
    }

    if windowed {
        renderer.resize_and_present(width, height)?;
    }
    renderer.set_render_resolution(width, height)?;
    renderer.restart()?;
    renderer.wait_idle()?;
    let after = renderer.resource_counts();
    let differences = ResourceCounts::diff(&before, &after);
    let baseline_total_after: i64 = after.iter().map(|(_, count)| *count).sum();

    report.push(
        "repeated resolution changes and restarts do not leak GPU resources",
        differences.is_empty(),
        if differences.is_empty() {
            format!(
                "{ROUNDS} rounds over {} resolutions ({}); live objects {} -> {} (peak {peak_total})",
                SIZES.len(),
                if windowed {
                    "swapchain recreation + accumulation image recreation"
                } else {
                    "accumulation image recreation"
                },
                baseline_total,
                baseline_total_after
            )
        } else {
            format!(
                "live object counts changed: {}",
                differences
                    .iter()
                    .map(|(kind, delta)| format!("{} {delta:+}", kind.name()))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        },
    );

    // Nothing may be leaked by the verification run itself either: the counts
    // must be identical across a second identical pass.
    let before_second = renderer.resource_counts();
    for _ in 0..4 {
        if windowed {
            renderer.resize_and_present(640, 360)?;
        }
        renderer.set_render_resolution(640, 360)?;
        renderer.restart()?;
        if windowed {
            renderer.resize_and_present(width, height)?;
        }
        renderer.set_render_resolution(width, height)?;
        renderer.restart()?;
    }
    renderer.wait_idle()?;
    let after_second = renderer.resource_counts();
    let differences = ResourceCounts::diff(&before_second, &after_second);
    report.push(
        "second resize pass is also leak-free",
        differences.is_empty(),
        if differences.is_empty() {
            format!("live objects steady at {baseline_total_after}")
        } else {
            differences
                .iter()
                .map(|(kind, delta)| format!("{} {delta:+}", kind.name()))
                .collect::<Vec<_>>()
                .join(", ")
        },
    );
    Ok(())
}

fn reproducibility_check(renderer: &mut Renderer, report: &mut SelfTestReport) -> Result<()> {
    let saved = renderer.settings();
    renderer.set_samples_per_pixel(16)?;
    renderer.set_seed(12345)?;
    renderer.set_exposure(1.0);
    renderer.restart()?;
    renderer.dispatch_batches(16, 4, false)?;
    let (pixels_a, width, height, samples_a) = renderer.decode_pixels()?;
    let hash_a = color::hash_pixels_rgba8(&pixels_a);

    // Re-render with identical settings: the decoded pixels must be identical.
    renderer.restart()?;
    renderer.dispatch_batches(16, 4, false)?;
    let (pixels_b, width_b, height_b, samples_b) = renderer.decode_pixels()?;
    let hash_b = color::hash_pixels_rgba8(&pixels_b);

    report.push(
        "same settings + seed reproduce identical decoded pixels",
        hash_a == hash_b && samples_a == 16 && samples_b == 16,
        format!(
            "{width}x{height}, {samples_a}/{samples_b} samples, sha256 {hash_a} vs {hash_b}"
        ),
    );
    report.push(
        "accumulation stops at the selected sample count",
        renderer.status() == crate::render::RenderStatus::Complete
            && renderer.accumulated_samples() == 16
            && width_b == width
            && height_b == height,
        format!(
            "status {:?}, accumulated {} of {}",
            renderer.status(),
            renderer.accumulated_samples(),
            renderer.settings().samples_per_pixel
        ),
    );

    // Exposure must reuse the accumulated samples (no re-render).
    let sums = renderer.read_accum_rgba32f()?;
    renderer.set_exposure(2.0);
    let (pixels_exposed, _, _, samples_exposed) = renderer.decode_pixels()?;
    let mut expected = vec![0u8; pixels_exposed.len()];
    for (index, chunk) in expected.chunks_exact_mut(4).enumerate() {
        let base = index * 4;
        let rgb = color::linear_sum_to_srgb8(
            [sums[base], sums[base + 1], sums[base + 2]],
            samples_exposed,
            2.0,
        );
        chunk[0] = rgb[0];
        chunk[1] = rgb[1];
        chunk[2] = rgb[2];
        chunk[3] = 255;
    }
    report.push(
        "exposure change reuses accumulated samples",
        expected == pixels_exposed && samples_exposed == samples_a,
        format!(
            "{} pixels compared at exposure 2.0, {} samples kept",
            expected.len() / 4,
            samples_exposed
        ),
    );

    renderer.set_exposure(saved.exposure);
    Ok(())
}
