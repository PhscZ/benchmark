//! Orbit camera.
//!
//! Defaults from the specification: position `(0, 3, 10)`, target
//! `(0, 2.5, 0)`, up `(0, 1, 0)`, 45 degree vertical field of view.
//!
//! The camera is stored as an orbit around the look-at target (yaw, pitch,
//! distance) so that mouse orbit and zoom are exact and clamped.

use glam::{Vec2, Vec3};

/// Vertical field of view, degrees.
pub const FOV_Y_DEGREES: f32 = 45.0;
pub const DEFAULT_POSITION: Vec3 = Vec3::new(0.0, 3.0, 10.0);
pub const DEFAULT_TARGET: Vec3 = Vec3::new(0.0, 2.5, 0.0);
pub const DEFAULT_UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);

/// Orbit distance limits (world units).
pub const MIN_DISTANCE: f32 = 1.5;
pub const MAX_DISTANCE: f32 = 40.0;
/// Pitch limits keep the up vector from degenerating (|pitch| < 90 degrees).
pub const MAX_PITCH_RADIANS: f32 = 89.0 * std::f32::consts::PI / 180.0;
/// Orbit sensitivity: radians of rotation per pixel of mouse motion.
pub const ORBIT_RADIANS_PER_PIXEL: f32 = 0.005;
/// Zoom sensitivity: distance multiplier per wheel notch (zoom in / zoom out).
pub const ZOOM_FACTOR_PER_NOTCH: f32 = 0.9;

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub target: Vec3,
    pub up: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov_y_degrees: f32,
}

/// The per-frame camera basis uploaded to the shader.
#[derive(Clone, Copy, Debug)]
pub struct CameraBasis {
    pub origin: Vec3,
    pub lower_left: Vec3,
    pub horizontal: Vec3,
    pub vertical: Vec3,
}

impl Default for Camera {
    fn default() -> Self {
        Self::new(DEFAULT_POSITION, DEFAULT_TARGET, DEFAULT_UP)
    }
}

impl Camera {
    /// Builds an orbit camera that reproduces the given eye position exactly.
    pub fn new(position: Vec3, target: Vec3, up: Vec3) -> Self {
        let offset = position - target;
        let distance = offset.length();
        let pitch = (offset.y / distance).asin();
        let yaw = offset.x.atan2(offset.z);
        Self {
            target,
            up,
            yaw,
            pitch,
            distance,
            fov_y_degrees: FOV_Y_DEGREES,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn position(&self) -> Vec3 {
        let cos_pitch = self.pitch.cos();
        self.target
            + Vec3::new(
                self.yaw.sin() * cos_pitch,
                self.pitch.sin(),
                self.yaw.cos() * cos_pitch,
            ) * self.distance
    }

    /// Orbits by a mouse delta in pixels.
    pub fn orbit(&mut self, delta: Vec2) {
        self.yaw += delta.x * ORBIT_RADIANS_PER_PIXEL;
        self.pitch = (self.pitch + delta.y * ORBIT_RADIANS_PER_PIXEL)
            .clamp(-MAX_PITCH_RADIANS, MAX_PITCH_RADIANS);
        // Keep yaw in a sane range to avoid precision loss over long drags.
        let two_pi = std::f32::consts::TAU;
        if self.yaw > std::f32::consts::PI || self.yaw < -std::f32::consts::PI {
            self.yaw = self.yaw.rem_euclid(two_pi);
            if self.yaw > std::f32::consts::PI {
                self.yaw -= two_pi;
            }
        }
    }

    /// Zooms by a number of wheel notches (positive = zoom in).
    pub fn zoom(&mut self, notches: f32) {
        self.distance =
            (self.distance * ZOOM_FACTOR_PER_NOTCH.powf(notches)).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Computes the ray-generation basis for a render resolution.
    pub fn basis(&self, width: u32, height: u32) -> CameraBasis {
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let origin = self.position();
        let forward = (self.target - origin).normalize();
        let right = forward.cross(self.up).normalize();
        let true_up = right.cross(forward);

        let half_height = (self.fov_y_degrees.to_radians() * 0.5).tan();
        let viewport_height = 2.0 * half_height;
        let viewport_width = aspect * viewport_height;
        let horizontal = right * viewport_width;
        let vertical = true_up * viewport_height;
        let lower_left = origin + forward - horizontal * 0.5 - vertical * 0.5;

        CameraBasis {
            origin,
            lower_left,
            horizontal,
            vertical,
        }
    }

    /// Human-readable camera state for the UI.
    pub fn describe(&self) -> String {
        let p = self.position();
        format!(
            "cam ({:.2},{:.2},{:.2}) d={:.2}",
            p.x, p.y, p.z, self.distance
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_camera_matches_specification() {
        let camera = Camera::default();
        let position = camera.position();
        assert!((position - DEFAULT_POSITION).length() < 1e-5, "{position:?}");
        assert!((camera.target - DEFAULT_TARGET).length() < 1e-6);
        assert_eq!(camera.fov_y_degrees, 45.0);
    }

    #[test]
    fn center_ray_points_at_target() {
        let camera = Camera::default();
        let basis = camera.basis(1280, 720);
        let center = basis.lower_left + basis.horizontal * 0.5 + basis.vertical * 0.5;
        let dir = (center - basis.origin).normalize();
        let expected = (camera.target - camera.position()).normalize();
        assert!((dir - expected).length() < 1e-5, "{dir:?} vs {expected:?}");
    }

    #[test]
    fn vertical_field_of_view_is_45_degrees() {
        let camera = Camera::default();
        let basis = camera.basis(1280, 720);
        // Bottom edge direction versus the forward axis.
        let bottom = basis.lower_left + basis.horizontal * 0.5;
        let dir = (bottom - basis.origin).normalize();
        let forward = (camera.target - camera.position()).normalize();
        let angle = dir.dot(forward).clamp(-1.0, 1.0).acos().to_degrees();
        assert!((angle - 22.5).abs() < 1e-3, "{angle}");
    }

    #[test]
    fn aspect_ratio_scales_horizontal_extent() {
        let camera = Camera::default();
        let wide = camera.basis(1920, 1080);
        let square = camera.basis(1080, 1080);
        let ratio = wide.horizontal.length() / square.horizontal.length();
        assert!((ratio - 1920.0 / 1080.0).abs() < 1e-4, "{ratio}");
    }

    #[test]
    fn zoom_and_pitch_are_clamped() {
        let mut camera = Camera::default();
        for _ in 0..500 {
            camera.zoom(1.0);
        }
        assert!((camera.distance - MIN_DISTANCE).abs() < 1e-4);
        for _ in 0..500 {
            camera.zoom(-1.0);
        }
        assert!((camera.distance - MAX_DISTANCE).abs() < 1e-4);
        camera.orbit(Vec2::new(0.0, 100000.0));
        assert!(camera.pitch <= MAX_PITCH_RADIANS + 1e-6);
        camera.orbit(Vec2::new(0.0, -1000000.0));
        assert!(camera.pitch >= -MAX_PITCH_RADIANS - 1e-6);
        // The up vector never degenerates.
        let basis = camera.basis(640, 480);
        assert!(basis.horizontal.is_finite() && basis.vertical.is_finite());
    }

    #[test]
    fn reset_restores_defaults() {
        let mut camera = Camera::default();
        camera.orbit(Vec2::new(120.0, -40.0));
        camera.zoom(3.0);
        camera.reset();
        assert!((camera.position() - DEFAULT_POSITION).length() < 1e-5);
    }
}
