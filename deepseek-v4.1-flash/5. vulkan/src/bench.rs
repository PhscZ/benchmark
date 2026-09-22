//! Fixed-camera benchmark mode.
//!
//! Sequence (per the specification):
//!   1. default camera, 1280x720, 64 spp, 4 reflection bounces, exposure 1.0,
//!      seed 12345
//!   2. one unmeasured warm-up render
//!   3. clear the accumulation
//!   4. measured render with identical settings, split into bounded dispatches
//!
//! Reported timings:
//!   * initialization (instance/device/resource creation) and pipeline creation,
//!     measured separately on the host
//!   * GPU time from Vulkan timestamp queries, summed over the compute
//!     dispatches of the measured render (this is GPU execution time, not CPU
//!     submission time)
//!   * wall-clock time to complete the measured accumulation, including waiting
//!     for GPU completion
//!
//! PNG encoding and disk writes happen after all timings are taken and are
//! therefore excluded from them.

use std::path::PathBuf;

use crate::error::Result;
use crate::render::{DispatchStats, Renderer, DEFAULT_SEED, MAX_REFLECTION_BOUNCES};
use crate::vk::GpuInfo;

pub const BENCHMARK_WIDTH: u32 = 1280;
pub const BENCHMARK_HEIGHT: u32 = 720;
pub const BENCHMARK_SAMPLES: u32 = 64;
pub const BENCHMARK_REFLECTIONS: u32 = 4;
pub const BENCHMARK_EXPOSURE: f32 = 1.0;

#[derive(Clone, Debug)]
pub struct BenchConfig {
    pub width: u32,
    pub height: u32,
    pub samples_per_pixel: u32,
    pub max_reflections: u32,
    pub exposure: f32,
    pub seed: u32,
    pub output_dir: PathBuf,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            width: BENCHMARK_WIDTH,
            height: BENCHMARK_HEIGHT,
            samples_per_pixel: BENCHMARK_SAMPLES,
            max_reflections: BENCHMARK_REFLECTIONS,
            exposure: BENCHMARK_EXPOSURE,
            seed: DEFAULT_SEED,
            output_dir: PathBuf::from("renders"),
        }
    }
}

impl BenchConfig {
    pub fn validate(&self) -> Result<()> {
        crate::render::validate_samples(self.samples_per_pixel)?;
        if self.max_reflections > MAX_REFLECTION_BOUNCES {
            return crate::error::msg(format!(
                "max reflections must be 0..={MAX_REFLECTION_BOUNCES}"
            ));
        }
        if self.width == 0 || self.height == 0 {
            return crate::error::msg("benchmark resolution must be non-zero");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct PhaseTiming {
    pub samples: u32,
    pub dispatches: u32,
    pub gpu_seconds: Option<f64>,
    pub wall_seconds: f64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct BenchReport {
    pub gpu: GpuInfo,
    pub resolution: [u32; 2],
    pub samples_per_pixel: u32,
    pub completed_samples: u32,
    pub render_complete: bool,
    pub max_reflections: u32,
    pub exposure: f32,
    pub seed: u32,
    pub samples_per_dispatch: u32,
    pub initialization_seconds: f64,
    pub pipeline_creation_seconds: f64,
    pub warmup: PhaseTiming,
    pub measured: PhaseTiming,
    pub primary_rays: u64,
    pub measured_primary_rays_per_second: Option<f64>,
    pub timestamps_supported: bool,
    pub timestamp_period_ns: f32,
    pub png_path: String,
    pub png_sha256: String,
    pub png_excluded_from_timings: bool,
    pub notes: Vec<String>,
}

impl BenchReport {
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str("=== vk-raytracer benchmark ===\n");
        out.push_str(&format!("GPU                : {}\n", self.gpu.device_name));
        out.push_str(&format!(
            "Driver             : {} {} (Vulkan {})\n",
            self.gpu.driver_name, self.gpu.driver_version, self.gpu.api_version
        ));
        out.push_str(&format!(
            "Resolution         : {}x{}\n",
            self.resolution[0], self.resolution[1]
        ));
        out.push_str(&format!(
            "Settings           : {} spp, {} reflection bounces, exposure {:.2}, seed {}\n",
            self.samples_per_pixel, self.max_reflections, self.exposure, self.seed
        ));
        out.push_str(&format!(
            "Samples completed  : {} of {} ({})\n",
            self.completed_samples,
            self.samples_per_pixel,
            if self.render_complete {
                "complete"
            } else {
                "partial"
            }
        ));
        out.push_str(&format!(
            "Initialization     : {:.3} s (instance, device, resources)\n",
            self.initialization_seconds
        ));
        out.push_str(&format!(
            "Pipeline creation  : {:.3} s\n",
            self.pipeline_creation_seconds
        ));
        out.push_str(&format!(
            "Warm-up (unmeasured): {} dispatches, wall {:.3} s{}\n",
            self.warmup.dispatches,
            self.warmup.wall_seconds,
            self.warmup
                .gpu_seconds
                .map(|g| format!(", GPU {g:.3} s"))
                .unwrap_or_default()
        ));
        out.push_str(&format!(
            "Measured GPU time  : {}\n",
            self.measured
                .gpu_seconds
                .map(|g| format!("{g:.4} s ({} dispatches)", self.measured.dispatches))
                .unwrap_or_else(|| "unavailable (no timestamp support)".to_string())
        ));
        out.push_str(&format!(
            "Measured wall time : {:.4} s (includes waiting for GPU completion)\n",
            self.measured.wall_seconds
        ));
        out.push_str(&format!(
            "Primary rays       : {}{}\n",
            self.primary_rays,
            self.measured_primary_rays_per_second
                .map(|r| format!(", {:.3} Mrays/s", r / 1e6))
                .unwrap_or_default()
        ));
        out.push_str(&format!("PNG                : {}\n", self.png_path));
        out.push_str(&format!("PNG sha256         : {}\n", self.png_sha256));
        for note in &self.notes {
            out.push_str(&format!("Note               : {note}\n"));
        }
        out
    }
}

/// Runs the measured benchmark.  The caller is responsible for the warm-up
/// render (the interactive loop performs it, so progress is visible) and for
/// having configured the renderer with the benchmark settings.
pub fn run_measured(
    renderer: &mut Renderer,
    config: &BenchConfig,
    initialization_seconds: f64,
    pipeline_creation_seconds: f64,
    warmup: PhaseTiming,
    notes: Vec<String>,
) -> Result<BenchReport> {
    config.validate()?;
    let mut notes = notes;

    // Clear the accumulation, then measure.
    renderer.restart()?;
    let batch = config
        .samples_per_pixel
        .div_ceil(crate::render::BENCHMARK_DISPATCHES)
        .max(1);
    let stats: DispatchStats =
        renderer.dispatch_batches(config.samples_per_pixel, batch, true)?;

    if !renderer.timestamps_supported() {
        notes.push(
            "timestamp queries are unavailable on this device; only wall-clock time is reported"
                .to_string(),
        );
    }
    let gpu_seconds = stats.gpu_ns.map(|ns| ns as f64 / 1e9);
    let primary_rays = config.width as u64 * config.height as u64 * stats.dispatched_samples as u64;
    let measured = PhaseTiming {
        samples: stats.dispatched_samples,
        dispatches: stats.dispatches,
        gpu_seconds,
        wall_seconds: stats.wall_seconds,
    };

    // Export after timing.
    let export = renderer.save_png(&config.output_dir)?;
    let report_path = config.output_dir.join(format!(
        "benchmark_report_{}.json",
        crate::util::timestamp_string()
    ));
    notes.push("PNG encoding and disk writes excluded from the timings above".to_string());
    notes.push(format!("report: {}", report_path.display()));

    let report = BenchReport {
        gpu: renderer.gpu_info().clone(),
        resolution: [config.width, config.height],
        samples_per_pixel: config.samples_per_pixel,
        completed_samples: renderer.accumulated_samples(),
        render_complete: renderer.accumulated_samples() >= config.samples_per_pixel,
        max_reflections: config.max_reflections,
        exposure: config.exposure,
        seed: config.seed,
        samples_per_dispatch: batch,
        initialization_seconds,
        pipeline_creation_seconds,
        warmup,
        measured: measured.clone(),
        primary_rays,
        measured_primary_rays_per_second: measured
            .gpu_seconds
            .filter(|g| *g > 0.0)
            .map(|g| primary_rays as f64 / g),
        timestamps_supported: renderer.timestamps_supported(),
        timestamp_period_ns: renderer.gpu_info().timestamp_period_ns,
        png_path: export.path.display().to_string(),
        png_sha256: export.decoded_pixel_sha256.clone(),
        png_excluded_from_timings: true,
        notes,
    };

    let json = serde_json::to_string_pretty(&report)
        .map_err(|e| crate::error::Error::Message(e.to_string()))?;
    std::fs::create_dir_all(&config.output_dir)?;
    std::fs::write(&report_path, json)?;
    Ok(report)
}
