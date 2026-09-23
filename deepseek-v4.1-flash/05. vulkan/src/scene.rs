//! Fixed scene description.
//!
//! The GPU layout of these structs must match `shaders/lib/common.glsl`
//! exactly (std430 for the storage buffers, std140 for the uniform block).
//!
//! Coordinate system: right-handed, +Y up, +X right, +Z toward the viewer.
//! The room is open at Z = +3 (the front), so a camera at (0, 3, 10) looks in
//! through the open front.

use glam::Vec3;

use crate::camera::CameraBasis;

/// Open-front room extents.
pub const ROOM_MIN_X: f32 = -3.0;
pub const ROOM_MAX_X: f32 = 3.0;
pub const ROOM_MIN_Y: f32 = 0.0;
pub const ROOM_MAX_Y: f32 = 6.0;
pub const ROOM_MIN_Z: f32 = -3.0;
pub const ROOM_MAX_Z: f32 = 3.0;

pub const WALL_GREY: [f32; 3] = [0.75, 0.75, 0.75];
pub const LEFT_WALL_RED: [f32; 3] = [0.65, 0.05, 0.05];
pub const RIGHT_WALL_GREEN: [f32; 3] = [0.05, 0.65, 0.05];

pub const LIGHT_POSITION: Vec3 = Vec3::new(0.0, 5.5, 1.0);
pub const LIGHT_INTENSITY: Vec3 = Vec3::new(60.0, 60.0, 60.0);

/// Primitive kind ids; identical to `PRIM_SPHERE` / `PRIM_TRIANGLE` in GLSL.
pub const PRIM_SPHERE: u32 = 0;
pub const PRIM_TRIANGLE: u32 = 1;

/// `Sphere` in `shaders/lib/common.glsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuSphere {
    pub center_radius: [f32; 4],
    pub base_color: [f32; 4],
    /// x = kd, y = ks, z = shininess, w = reflectivity
    pub params: [f32; 4],
}

/// `Triangle` in `shaders/lib/common.glsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuTriangle {
    pub v0: [f32; 4],
    pub v1: [f32; 4],
    pub v2: [f32; 4],
    pub base_color: [f32; 4],
    /// x = kd, y = ks, z = shininess, w = reflectivity
    pub params: [f32; 4],
}

/// `SceneUniform` in `shaders/lib/common.glsl` (std140).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuSceneUniform {
    pub cam_origin: [f32; 4],
    pub cam_lower_left: [f32; 4],
    pub cam_horizontal: [f32; 4],
    pub cam_vertical: [f32; 4],
    pub light_position: [f32; 4],
    pub light_intensity: [f32; 4],
    /// x = sphere_count, y = triangle_count, z = seed, w = max_reflections
    pub counts: [u32; 4],
}

/// A material as uploaded to the GPU.
#[derive(Clone, Copy, Debug)]
pub struct Material {
    pub base_color: [f32; 3],
    pub kd: f32,
    pub ks: f32,
    pub shininess: f32,
    pub reflectivity: f32,
}

impl Material {
    const fn new(base_color: [f32; 3], kd: f32, ks: f32, shininess: f32, reflectivity: f32) -> Self {
        Self {
            base_color,
            kd,
            ks,
            shininess,
            reflectivity,
        }
    }

    fn params(&self) -> [f32; 4] {
        [self.kd, self.ks, self.shininess, self.reflectivity]
    }
}

/// Wall material: fully diffuse.
pub const WALL_MATERIAL: Material = Material::new([0.0, 0.0, 0.0], 1.0, 0.0, 1.0, 0.0);
/// Matte blue sphere.
pub const BLUE_SPHERE_MATERIAL: Material = Material::new([0.05, 0.15, 0.8], 1.0, 0.0, 1.0, 0.0);
/// Glossy gold sphere.
pub const GOLD_SPHERE_MATERIAL: Material = Material::new([0.8, 0.5, 0.1], 0.8, 0.5, 64.0, 0.15);
/// Perfect mirror sphere.
pub const MIRROR_SPHERE_MATERIAL: Material = Material::new([1.0, 1.0, 1.0], 0.0, 0.0, 1.0, 1.0);

/// The complete fixed scene: 3 analytic spheres + 10 triangles.
#[derive(Clone, Debug)]
pub struct Scene {
    pub spheres: Vec<GpuSphere>,
    pub triangles: Vec<GpuTriangle>,
    pub light_position: Vec3,
    pub light_intensity: Vec3,
}

impl Scene {
    /// The fixed benchmark scene described in the task specification.
    pub fn fixed() -> Self {
        let sphere = |center: Vec3, radius: f32, m: Material| GpuSphere {
            center_radius: [center.x, center.y, center.z, radius],
            base_color: [m.base_color[0], m.base_color[1], m.base_color[2], 1.0],
            params: m.params(),
        };

        let spheres = vec![
            // index 0
            sphere(
                Vec3::new(-1.6, 1.0, -0.8),
                1.0,
                BLUE_SPHERE_MATERIAL,
            ),
            // index 1
            sphere(Vec3::new(1.4, 1.0, -1.0), 1.0, GOLD_SPHERE_MATERIAL),
            // index 2 (tangent to the floor at (0, 0, 1))
            sphere(Vec3::new(0.0, 0.75, 1.0), 0.75, MIRROR_SPHERE_MATERIAL),
        ];

        // Quad helper: two triangles, deterministic order (a,b,c) then (a,c,d).
        let mut triangles = Vec::with_capacity(10);
        let mut quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3, color: [f32; 3]| {
            let m = Material {
                base_color: color,
                ..WALL_MATERIAL
            };
            triangles.push(GpuTriangle {
                v0: [a.x, a.y, a.z, 1.0],
                v1: [b.x, b.y, b.z, 1.0],
                v2: [c.x, c.y, c.z, 1.0],
                base_color: [m.base_color[0], m.base_color[1], m.base_color[2], 1.0],
                params: m.params(),
            });
            triangles.push(GpuTriangle {
                v0: [a.x, a.y, a.z, 1.0],
                v1: [c.x, c.y, c.z, 1.0],
                v2: [d.x, d.y, d.z, 1.0],
                base_color: [m.base_color[0], m.base_color[1], m.base_color[2], 1.0],
                params: m.params(),
            });
        };

        // Triangles 0-1: floor (y = 0)
        quad(
            Vec3::new(ROOM_MIN_X, ROOM_MIN_Y, ROOM_MIN_Z),
            Vec3::new(ROOM_MAX_X, ROOM_MIN_Y, ROOM_MIN_Z),
            Vec3::new(ROOM_MAX_X, ROOM_MIN_Y, ROOM_MAX_Z),
            Vec3::new(ROOM_MIN_X, ROOM_MIN_Y, ROOM_MAX_Z),
            WALL_GREY,
        );
        // Triangles 2-3: ceiling (y = 6)
        quad(
            Vec3::new(ROOM_MIN_X, ROOM_MAX_Y, ROOM_MIN_Z),
            Vec3::new(ROOM_MAX_X, ROOM_MAX_Y, ROOM_MIN_Z),
            Vec3::new(ROOM_MAX_X, ROOM_MAX_Y, ROOM_MAX_Z),
            Vec3::new(ROOM_MIN_X, ROOM_MAX_Y, ROOM_MAX_Z),
            WALL_GREY,
        );
        // Triangles 4-5: back wall (z = -3)
        quad(
            Vec3::new(ROOM_MIN_X, ROOM_MIN_Y, ROOM_MIN_Z),
            Vec3::new(ROOM_MAX_X, ROOM_MIN_Y, ROOM_MIN_Z),
            Vec3::new(ROOM_MAX_X, ROOM_MAX_Y, ROOM_MIN_Z),
            Vec3::new(ROOM_MIN_X, ROOM_MAX_Y, ROOM_MIN_Z),
            WALL_GREY,
        );
        // Triangles 6-7: left wall (x = -3)
        quad(
            Vec3::new(ROOM_MIN_X, ROOM_MIN_Y, ROOM_MIN_Z),
            Vec3::new(ROOM_MIN_X, ROOM_MIN_Y, ROOM_MAX_Z),
            Vec3::new(ROOM_MIN_X, ROOM_MAX_Y, ROOM_MAX_Z),
            Vec3::new(ROOM_MIN_X, ROOM_MAX_Y, ROOM_MIN_Z),
            LEFT_WALL_RED,
        );
        // Triangles 8-9: right wall (x = 3)
        quad(
            Vec3::new(ROOM_MAX_X, ROOM_MIN_Y, ROOM_MAX_Z),
            Vec3::new(ROOM_MAX_X, ROOM_MIN_Y, ROOM_MIN_Z),
            Vec3::new(ROOM_MAX_X, ROOM_MAX_Y, ROOM_MIN_Z),
            Vec3::new(ROOM_MAX_X, ROOM_MAX_Y, ROOM_MAX_Z),
            RIGHT_WALL_GREEN,
        );

        Self {
            spheres,
            triangles,
            light_position: LIGHT_POSITION,
            light_intensity: LIGHT_INTENSITY,
        }
    }

    pub fn sphere_count(&self) -> u32 {
        self.spheres.len() as u32
    }

    pub fn triangle_count(&self) -> u32 {
        self.triangles.len() as u32
    }

    /// Builds the std140 uniform block for the current camera / settings.
    pub fn uniform(&self, basis: &CameraBasis, seed: u32, max_reflections: u32) -> GpuSceneUniform {
        GpuSceneUniform {
            cam_origin: [basis.origin.x, basis.origin.y, basis.origin.z, 1.0],
            cam_lower_left: [
                basis.lower_left.x,
                basis.lower_left.y,
                basis.lower_left.z,
                0.0,
            ],
            cam_horizontal: [
                basis.horizontal.x,
                basis.horizontal.y,
                basis.horizontal.z,
                0.0,
            ],
            cam_vertical: [basis.vertical.x, basis.vertical.y, basis.vertical.z, 0.0],
            light_position: [
                self.light_position.x,
                self.light_position.y,
                self.light_position.z,
                1.0,
            ],
            light_intensity: [
                self.light_intensity.x,
                self.light_intensity.y,
                self.light_intensity.z,
                0.0,
            ],
            counts: [
                self.sphere_count(),
                self.triangle_count(),
                seed,
                max_reflections,
            ],
        }
    }
}
