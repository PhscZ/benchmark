//! `vk-raytracer` - a desktop GPU ray tracer written in Rust.
//!
//! The image is produced entirely by a Vulkan compute shader
//! (`shaders/raytrace.comp`): ray generation, analytic ray/sphere and
//! ray/triangle intersection, direct lighting with hard shadows, iterative
//! mirror reflections and progressive sample accumulation all run on the GPU.
//! Rasterization is used only to present the finished image and the UI overlay.
//!
//! See `README.md` for build/run instructions and the required driver
//! capabilities.

pub mod app;
pub mod bench;
pub mod camera;
pub mod color;
pub mod error;
pub mod reference;
pub mod render;
pub mod scene;
pub mod self_test;
pub mod ui;
pub mod util;
pub mod vk;

/// SPIR-V shaders compiled from `shaders/*.glsl` at build time by `build.rs`.
pub mod shaders {
    include!(concat!(env!("OUT_DIR"), "/shaders.rs"));
}
