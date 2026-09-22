//! The GPU renderer: accumulation image, compute dispatch, presentation pass,
//! readback/export, GPU timestamps and the GPU intersection self-test hook.

use std::path::{Path, PathBuf};
use std::time::Instant;

use ash::vk;

use crate::camera::Camera;
use crate::color;
use crate::error::{msg, Error, Result};
use crate::scene::{GpuSceneUniform, Scene};
use crate::ui::UiVertex;
use crate::vk::pipelines::{
    Pipelines, PresentPushConstants, TracePushConstants, UiPushConstants,
};
use crate::vk::resources::{
    create_sampler, image_barrier, GpuBuffer, GpuImage,
};
use crate::vk::swapchain::Swapchain;
use crate::vk::{Context, ContextOptions, GpuInfo};

/// Samples-per-pixel presets required by the specification.
pub const SAMPLE_PRESETS: [u32; 3] = [1, 16, 64];
pub const DEFAULT_SAMPLES_PER_PIXEL: u32 = 64;
pub const MAX_REFLECTION_BOUNCES: u32 = 4;
pub const DEFAULT_MAX_REFLECTIONS: u32 = 4;
pub const DEFAULT_SEED: u32 = 12345;
pub const DEFAULT_WIDTH: u32 = 1280;
pub const DEFAULT_HEIGHT: u32 = 720;
/// Samples dispatched per command buffer while interacting (bounded GPU work).
pub const INTERACTIVE_BATCH_SAMPLES: u32 = 1;
/// Number of dispatches the benchmark splits its samples into: bounding the work
/// per dispatch keeps every submission short enough that no Windows GPU timeout
/// (TDR) is triggered, without needing to raise the system timeout.
/// Measured on the RX 5700 XT, 8 x 8 spp and 1 x 64 spp take the same GPU time
/// (13.2 ms vs 13.1 ms at 1280x720), so the batching costs nothing.
pub const BENCHMARK_DISPATCHES: u32 = 8;
/// Maximum number of timestamp query pairs used by one benchmark render.
const MAX_QUERY_PAIRS: u32 = 64;
/// Timestamp query pair used for the interactive per-batch GPU time.
const INTERACTIVE_QUERY_PAIR: u32 = 0;
const TEST_RAY_CAPACITY: usize = 512;
const UI_VERTEX_CAPACITY: usize = 16 * 1024;
/// Compute work group size, must match `shaders/raytrace.comp`.
const WORKGROUP: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Settings {
    pub samples_per_pixel: u32,
    pub max_reflections: u32,
    pub exposure: f32,
    pub seed: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            samples_per_pixel: DEFAULT_SAMPLES_PER_PIXEL,
            max_reflections: DEFAULT_MAX_REFLECTIONS,
            exposure: color::DEFAULT_EXPOSURE,
            seed: DEFAULT_SEED,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderStatus {
    Idle,
    Accumulating,
    Complete,
    Paused,
}

impl RenderStatus {
    pub fn label(self) -> &'static str {
        match self {
            RenderStatus::Idle => "idle",
            RenderStatus::Accumulating => "accumulating",
            RenderStatus::Complete => "complete",
            RenderStatus::Paused => "paused",
        }
    }
}

/// Presentation state owned by the renderer: the surface plus the current
/// window extent.
pub struct SurfaceContext {
    pub surface: vk::SurfaceKHR,
    pub loader: ash::khr::surface::Instance,
    pub extent: vk::Extent2D,
}

#[derive(Clone, Debug, Default)]
pub struct RendererOptions {
    pub validation: bool,
    pub gpu_name_filter: Option<String>,
    /// Default samples-per-pixel (used for the first render).
    pub samples_per_pixel: Option<u32>,
    pub max_reflections: Option<u32>,
    pub exposure: Option<f32>,
    pub seed: Option<u32>,
}

pub struct FrameOutcome {
    pub presented: bool,
    pub dispatched_samples: u32,
    pub batch_gpu_ns: Option<u64>,
    pub swapchain_recreated: bool,
    pub extent: vk::Extent2D,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DispatchStats {
    pub dispatched_samples: u32,
    pub dispatches: u32,
    pub gpu_ns: Option<u64>,
    pub wall_seconds: f64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ExportInfo {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub accumulated_samples: u32,
    pub target_samples: u32,
    pub complete: bool,
    pub exposure: f32,
    pub decoded_pixel_sha256: String,
}

/// A ray handed to the GPU intersection self-test.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TestRay {
    pub origin: [f32; 4],
    pub dir: [f32; 4],
    /// x = t_min, y = t_max, z = mode (0 = intersect, 1 = path, 2 = shade)
    pub t_min_max: [f32; 4],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TestResult {
    pub hit: bool,
    pub t: f32,
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    pub prim_kind: u32,
    pub prim_index: u32,
    pub any_hit: bool,
    pub mode: u32,
}

struct FrameSlot {
    command_buffer: vk::CommandBuffer,
    guard_command_buffer: vk::CommandBuffer,
    fence: vk::Fence,
    image_available: vk::Semaphore,
    pending_query: bool,
}

pub struct Renderer {
    pub ctx: Context,
    surface: Option<SurfaceContext>,
    pipelines: Pipelines,
    swapchain: Option<Swapchain>,
    scene: Scene,
    settings: Settings,
    camera: Camera,

    sphere_buffer: GpuBuffer,
    triangle_buffer: GpuBuffer,
    uniform_buffer: GpuBuffer,
    test_ray_buffer: GpuBuffer,
    test_result_buffer: GpuBuffer,
    readback_buffer: GpuBuffer,
    ui_vertex_buffer: GpuBuffer,
    ui_capacity: usize,
    accum: GpuImage,
    accum_extent: vk::Extent2D,
    accum_layout: vk::ImageLayout,
    font_atlas: GpuImage,
    sampler_nearest: vk::Sampler,
    query_pool: vk::QueryPool,
    slots: Vec<FrameSlot>,
    slot_index: usize,

    accumulated_samples: u32,
    paused: bool,
    swapchain_dirty: bool,
    render_started: Option<Instant>,
    gpu_time_ns: u64,
    last_batch_ns: Option<u64>,
    frames_rendered: u64,
    /// Accumulation resolution used when running headless (no swapchain).
    headless_extent: vk::Extent2D,
    /// Time spent creating shader modules and pipelines (reported separately
    /// from the rest of initialization).
    pipeline_creation_seconds: f64,
}

impl Renderer {
    pub fn new(
        mut ctx: Context,
        extent: Option<vk::Extent2D>,
        options: &RendererOptions,
    ) -> Result<Self> {
        // Take ownership of the presentation surface (created during context
        // setup so that device selection could verify present support).
        let surface = match ctx.surface.take() {
            Some(info) => Some(SurfaceContext {
                surface: info.handle,
                loader: info.loader,
                extent: extent.unwrap_or(vk::Extent2D {
                    width: DEFAULT_WIDTH,
                    height: DEFAULT_HEIGHT,
                }),
            }),
            None => None,
        };
        let mut settings = Settings::default();
        if let Some(s) = options.samples_per_pixel {
            settings.samples_per_pixel = s;
        }
        if let Some(r) = options.max_reflections {
            settings.max_reflections = r.min(MAX_REFLECTION_BOUNCES);
        }
        if let Some(e) = options.exposure {
            settings.exposure = e;
        }
        if let Some(seed) = options.seed {
            settings.seed = seed;
        }
        settings.samples_per_pixel = validate_samples(settings.samples_per_pixel)?;

        let scene = Scene::fixed();
        let device = &ctx.device;

        // --- static GPU buffers ---------------------------------------------
        let sphere_bytes = bytemuck::cast_slice(&scene.spheres);
        let triangle_bytes = bytemuck::cast_slice(&scene.triangles);
        let sphere_buffer = GpuBuffer::upload(
            &ctx,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            sphere_bytes,
        )?;
        let triangle_buffer = GpuBuffer::upload(
            &ctx,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            triangle_bytes,
        )?;
        let uniform_buffer = GpuBuffer::host_visible(
            &ctx,
            std::mem::size_of::<GpuSceneUniform>() as u64,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
        )?;
        let test_ray_buffer = GpuBuffer::host_visible(
            &ctx,
            (TEST_RAY_CAPACITY * std::mem::size_of::<TestRay>()) as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        let test_result_buffer = GpuBuffer::host_visible(
            &ctx,
            (TEST_RAY_CAPACITY * 80) as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        let ui_vertex_buffer = GpuBuffer::host_visible(
            &ctx,
            (UI_VERTEX_CAPACITY * std::mem::size_of::<UiVertex>()) as u64,
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )?;

        let headless_extent = vk::Extent2D {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
        };
        let initial_extent = surface
            .as_ref()
            .map(|s| s.extent)
            .unwrap_or(headless_extent);
        let accum = GpuImage::new(
            &ctx,
            vk::Format::R32G32B32A32_SFLOAT,
            initial_extent,
            vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST,
            vk::ImageAspectFlags::COLOR,
        )?;

        let (atlas_w, atlas_h) = crate::ui::atlas_extent()?;
        let font_atlas = GpuImage::upload(
            &ctx,
            vk::Format::R8_UNORM,
            vk::Extent2D {
                width: atlas_w,
                height: atlas_h,
            },
            vk::ImageAspectFlags::COLOR,
            &crate::ui::build_font_atlas(),
            1,
        )?;

        let sampler_nearest = create_sampler(
            &ctx,
            vk::Filter::NEAREST,
            vk::SamplerAddressMode::CLAMP_TO_EDGE,
        )?;

        let readback_buffer = GpuBuffer::host_visible(
            &ctx,
            readback_size(initial_extent),
            vk::BufferUsageFlags::TRANSFER_DST,
        )?;

        // --- pipelines -------------------------------------------------------
        let present_format = match surface.as_ref() {
            Some(surface) => swapchain_format_hint(&ctx, Some(surface))?,
            None => vk::Format::B8G8R8A8_UNORM,
        };
        let pipeline_start = Instant::now();
        let pipelines = Pipelines::create(&ctx, present_format)?;
        let pipeline_creation_seconds = pipeline_start.elapsed().as_secs_f64();

        let query_pool = unsafe {
            device.create_query_pool(
                &vk::QueryPoolCreateInfo::default()
                    .query_type(vk::QueryType::TIMESTAMP)
                    .query_count(MAX_QUERY_PAIRS * 2),
                None,
            )?
        };
        ctx.resources.created(crate::vk::ResourceKind::QueryPool);

        let mut slots = Vec::with_capacity(crate::vk::FRAMES_IN_FLIGHT);
        for _ in 0..crate::vk::FRAMES_IN_FLIGHT {
            let command_buffer = ctx.allocate_command_buffer()?;
            let guard_command_buffer = ctx.allocate_command_buffer()?;
            let fence = ctx.create_fence(true)?;
            let image_available = ctx.create_semaphore()?;
            slots.push(FrameSlot {
                command_buffer,
                guard_command_buffer,
                fence,
                image_available,
                pending_query: false,
            });
        }

        let mut renderer = Self {
            ctx,
            surface,
            pipelines,
            swapchain: None,
            scene,
            settings,
            camera: Camera::default(),
            sphere_buffer,
            triangle_buffer,
            uniform_buffer,
            test_ray_buffer,
            test_result_buffer,
            readback_buffer,
            ui_vertex_buffer,
            ui_capacity: UI_VERTEX_CAPACITY,
            accum,
            accum_extent: initial_extent,
            accum_layout: vk::ImageLayout::UNDEFINED,
            font_atlas,
            sampler_nearest,
            query_pool,
            slots,
            slot_index: 0,
            accumulated_samples: 0,
            paused: false,
            swapchain_dirty: false,
            render_started: None,
            gpu_time_ns: 0,
            last_batch_ns: None,
            frames_rendered: 0,
            headless_extent,
            pipeline_creation_seconds,
        };

        if renderer.surface.is_some() {
            renderer.create_swapchain(None)?;
        }
        renderer.update_descriptors()?;
        renderer.clear_accumulation()?;
        Ok(renderer)
    }

    // --- accessors ---------------------------------------------------------

    pub fn gpu_info(&self) -> &GpuInfo {
        &self.ctx.info
    }

    pub fn settings(&self) -> Settings {
        self.settings
    }

    pub fn camera(&self) -> &Camera {
        &self.camera
    }

    pub fn camera_mut(&mut self) -> &mut Camera {
        &mut self.camera
    }

    pub fn accumulated_samples(&self) -> u32 {
        self.accumulated_samples
    }

    pub fn status(&self) -> RenderStatus {
        if self.accumulated_samples == 0 {
            RenderStatus::Idle
        } else if self.accumulated_samples >= self.settings.samples_per_pixel {
            RenderStatus::Complete
        } else if self.paused {
            RenderStatus::Paused
        } else {
            RenderStatus::Accumulating
        }
    }

    pub fn render_extent(&self) -> vk::Extent2D {
        match &self.swapchain {
            Some(swapchain) => swapchain.extent,
            None => self.headless_extent,
        }
    }

    pub fn swapchain_description(&self) -> Option<String> {
        self.swapchain.as_ref().map(|s| {
            format!(
                "{} {}x{} {} images",
                s.format_name(),
                s.extent.width,
                s.extent.height,
                s.image_count()
            )
        })
    }

    pub fn present_mode(&self) -> Option<&'static str> {
        self.swapchain.as_ref().map(|s| s.present_mode_name())
    }

    pub fn elapsed_seconds(&self) -> f64 {
        self.render_started
            .map(|start| start.elapsed().as_secs_f64())
            .unwrap_or(0.0)
    }

    pub fn gpu_time_ns(&self) -> u64 {
        self.gpu_time_ns
    }

    pub fn last_batch_ns(&self) -> Option<u64> {
        self.last_batch_ns
    }

    pub fn frames_rendered(&self) -> u64 {
        self.frames_rendered
    }

    pub fn timestamps_supported(&self) -> bool {
        self.ctx.info.timestamps_supported
    }

    pub fn has_swapchain(&self) -> bool {
        self.swapchain.is_some()
    }

    /// Drives one real window-resize round-trip (surface extent change followed
    /// by the swapchain recreation that `frame()` performs), so the self-test can
    /// verify the swapchain path as well as the accumulation-image path.
    pub fn resize_and_present(&mut self, width: u32, height: u32) -> Result<()> {
        self.resize_surface(width, height);
        let viewport = [width as f32, height as f32];
        self.frame(&[], viewport)?;
        Ok(())
    }

    /// Live Vulkan object counts (buffer, image, memory, pipeline, ...), used to
    /// verify that resizing and restarts do not leak GPU resources.
    pub fn resource_counts(&self) -> Vec<(crate::vk::ResourceKind, i64)> {
        self.ctx.resources.snapshot()
    }

    /// Time spent compiling/creating pipelines, reported separately from the
    /// rest of initialization.
    pub fn pipeline_creation_seconds(&self) -> f64 {
        self.pipeline_creation_seconds
    }

    // --- settings ----------------------------------------------------------

    /// Changes the samples-per-pixel preset.  Resets accumulation, because the
    /// sampling pattern (and therefore the image) changes.
    pub fn set_samples_per_pixel(&mut self, samples: u32) -> Result<()> {
        let samples = validate_samples(samples)?;
        if samples != self.settings.samples_per_pixel {
            self.settings.samples_per_pixel = samples;
            self.restart()?;
        }
        Ok(())
    }

    /// Changes the reflection bounce limit (0..=4).  Resets accumulation.
    pub fn adjust_reflections(&mut self, delta: i32) -> Result<()> {
        let current = self.settings.max_reflections as i32;
        let updated = (current + delta).clamp(0, MAX_REFLECTION_BOUNCES as i32) as u32;
        if updated != self.settings.max_reflections {
            self.settings.max_reflections = updated;
            self.restart()?;
        }
        Ok(())
    }

    pub fn set_reflections(&mut self, value: u32) -> Result<()> {
        let value = value.min(MAX_REFLECTION_BOUNCES);
        if value != self.settings.max_reflections {
            self.settings.max_reflections = value;
            self.restart()?;
        }
        Ok(())
    }

    /// Exposure only affects the display/export transform, so accumulated
    /// linear samples are reused and rendering does not restart.
    pub fn adjust_exposure(&mut self, steps: i32) {
        let factor = 1.25f32.powi(steps);
        self.set_exposure(self.settings.exposure * factor);
    }

    pub fn set_exposure(&mut self, exposure: f32) {
        self.settings.exposure = exposure.clamp(0.01, 100.0);
    }

    pub fn set_seed(&mut self, seed: u32) -> Result<()> {
        if seed != self.settings.seed {
            self.settings.seed = seed;
            self.restart()?;
        }
        Ok(())
    }

    /// Restarts accumulation from zero with the current settings.
    pub fn restart(&mut self) -> Result<()> {
        self.clear_accumulation()?;
        self.accumulated_samples = 0;
        self.render_started = None;
        self.gpu_time_ns = 0;
        self.last_batch_ns = None;
        self.paused = false;
        Ok(())
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn toggle_paused(&mut self) {
        self.paused = !self.paused;
    }

    pub fn reset_camera(&mut self) -> Result<()> {
        self.camera.reset();
        self.restart()
    }

    /// Called on window resize: the surface extent changed.
    pub fn resize_surface(&mut self, width: u32, height: u32) {
        if let Some(surface) = &mut self.surface {
            if width == 0 || height == 0 {
                surface.extent = vk::Extent2D {
                    width: 0,
                    height: 0,
                };
            } else {
                surface.extent = vk::Extent2D { width, height };
            }
        }
        self.swapchain_dirty = true;
    }

    // --- frame -------------------------------------------------------------

    /// Renders one frame: accumulates a bounded batch of samples and presents.
    pub fn frame(&mut self, ui_vertices: &[UiVertex], viewport: [f32; 2]) -> Result<FrameOutcome> {
        let mut recreated = false;
        if self.surface.is_some() && (self.swapchain_dirty || self.swapchain.is_none()) {
            let extent = self
                .surface
                .as_ref()
                .map(|s| s.extent)
                .unwrap_or(self.headless_extent);
            if extent.width == 0 || extent.height == 0 {
                // Minimized: nothing to do, and no resources to touch.
                return Ok(FrameOutcome {
                    presented: false,
                    dispatched_samples: 0,
                    batch_gpu_ns: None,
                    swapchain_recreated: false,
                    extent,
                });
            }
            self.recreate_swapchain()?;
            recreated = true;
        }

        let extent = self.render_extent();
        if extent.width == 0 || extent.height == 0 {
            return Ok(FrameOutcome {
                presented: false,
                dispatched_samples: 0,
                batch_gpu_ns: None,
                swapchain_recreated: recreated,
                extent,
            });
        }

        self.ensure_accum_extent(extent.width, extent.height)?;
        self.ensure_readback_capacity(extent)?;
        self.ensure_ui_capacity(ui_vertices.len())?;

        let slot_index = self.slot_index;
        let slot = &self.slots[slot_index];
        unsafe {
            self.ctx
                .device
                .wait_for_fences(&[slot.fence], true, u64::MAX)?;
        }
        let batch_gpu_ns = self.collect_batch_timestamps(slot_index)?;

        let should_dispatch = !self.paused
            && self.accumulated_samples < self.settings.samples_per_pixel;
        let dispatch_samples = if should_dispatch {
            INTERACTIVE_BATCH_SAMPLES
                .min(self.settings.samples_per_pixel - self.accumulated_samples)
        } else {
            0
        };

        // Copy out the swapchain handles we need so that the recording helpers
        // can take `&mut self`.
        let (swapchain_handle, swapchain_loader) = {
            let swapchain = self
                .swapchain
                .as_ref()
                .ok_or_else(|| Error::Message("frame() called without a swapchain".into()))?;
            (swapchain.handle, swapchain.loader.clone())
        };
        let acquire = unsafe {
            swapchain_loader.acquire_next_image(
                swapchain_handle,
                u64::MAX,
                self.slots[slot_index].image_available,
                vk::Fence::null(),
            )
        };
        let image_index = match acquire {
            Ok((index, suboptimal)) => {
                if suboptimal {
                    self.swapchain_dirty = true;
                }
                index
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.recreate_swapchain()?;
                return Ok(FrameOutcome {
                    presented: false,
                    dispatched_samples: 0,
                    batch_gpu_ns: None,
                    swapchain_recreated: true,
                    extent: self.render_extent(),
                });
            }
            Err(e) => return Err(e.into()),
        };

        let command_buffer = self.slots[slot_index].command_buffer;
        let fence = self.slots[slot_index].fence;
        // The timestamp query pair is reset on the host (Vulkan 1.2
        // vkResetQueryPool) rather than inside the command buffer: an in-buffer
        // reset introduces a command-processor sync point that measurably
        // inflates the timestamped interval.
        if dispatch_samples > 0 {
            unsafe {
                self.ctx
                    .device
                    .reset_query_pool(self.query_pool, INTERACTIVE_QUERY_PAIR * 2, 2);
            }
        }
        unsafe {
            self.ctx.device.reset_fences(&[fence])?;
            self.ctx
                .device
                .reset_command_buffer(command_buffer, vk::CommandBufferResetFlags::empty())?;
            self.ctx.device.begin_command_buffer(
                command_buffer,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
        }

        if dispatch_samples > 0 {
            self.record_dispatch(
                command_buffer,
                self.accumulated_samples,
                dispatch_samples,
                Some(INTERACTIVE_QUERY_PAIR),
            )?;
            self.slots[slot_index].pending_query = true;
        }
        self.record_presentation(command_buffer, image_index, ui_vertices, viewport)?;

        unsafe { self.ctx.device.end_command_buffer(command_buffer)? };

        let signal_semaphores = [self.swapchain.as_ref().unwrap().render_finished
            [image_index as usize]];
        let wait_semaphores = [self.slots[slot_index].image_available];
        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let command_buffers = [command_buffer];
        let submit = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(&command_buffers)
            .signal_semaphores(&signal_semaphores);
        unsafe {
            // The fence is signalled by the guard submission below, which the
            // queue processes after the present operation.  Waiting on it at the
            // start of the next frame therefore proves that both this submission
            // and the previous present (i.e. the render_finished semaphore wait)
            // have completed, which is what makes semaphore reuse safe.
            self.ctx
                .device
                .queue_submit(self.ctx.queue, &[submit], vk::Fence::null())?;
        }

        let swapchains = [swapchain_handle];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&signal_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        let present_result = unsafe { swapchain_loader.queue_present(self.ctx.queue, &present_info) };
        match present_result {
            Ok(suboptimal) => {
                if suboptimal {
                    self.swapchain_dirty = true;
                }
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.swapchain_dirty = true;
            }
            Err(e) => return Err(e.into()),
        }

        self.submit_guard(slot_index)?;

        self.frames_rendered += 1;
        if dispatch_samples > 0 {
            if self.render_started.is_none() {
                self.render_started = Some(Instant::now());
            }
            self.accumulated_samples += dispatch_samples;
        }
        if let Some(ns) = batch_gpu_ns {
            self.last_batch_ns = Some(ns);
            self.gpu_time_ns += ns;
        }
        self.slot_index = (slot_index + 1) % self.slots.len();

        Ok(FrameOutcome {
            presented: true,
            dispatched_samples: dispatch_samples,
            batch_gpu_ns,
            swapchain_recreated: recreated,
            extent,
        })
    }

    /// Dispatches `total` samples in bounded batches without presenting; used by
    /// benchmark mode so that wall-clock time is not inflated by presentation.
    pub fn dispatch_batches(
        &mut self,
        total_samples: u32,
        batch_samples: u32,
        measure_gpu: bool,
    ) -> Result<DispatchStats> {
        self.ensure_accum_extent(self.accum_extent.width, self.accum_extent.height)?;
        let batch_samples = batch_samples.max(1);
        let dispatches = total_samples.div_ceil(batch_samples);
        if measure_gpu && !self.ctx.info.timestamps_supported {
            return msg("GPU timestamp queries are not supported by this device/driver");
        }
        let mut command_buffers = Vec::with_capacity(dispatches as usize);
        let mut fences = Vec::with_capacity(dispatches as usize);
        for _ in 0..dispatches {
            command_buffers.push(self.ctx.allocate_command_buffer()?);
            fences.push(self.ctx.create_fence(false)?);
        }

        if measure_gpu {
            unsafe {
                self.ctx
                    .device
                    .reset_query_pool(self.query_pool, 0, dispatches * 2);
            }
        }
        let wall_start = Instant::now();
        let mut submitted = 0u32;
        let result = (|| -> Result<()> {
            let mut remaining = total_samples;
            let mut base = self.accumulated_samples;
            for (index, command_buffer) in command_buffers.iter().enumerate() {
                let count = remaining.min(batch_samples);
                unsafe {
                    self.ctx.device.begin_command_buffer(
                        *command_buffer,
                        &vk::CommandBufferBeginInfo::default()
                            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                    )?;
                }
                let query_pair = if measure_gpu { Some(index as u32) } else { None };
                self.record_dispatch(*command_buffer, base, count, query_pair)?;
                unsafe { self.ctx.device.end_command_buffer(*command_buffer)? };
                let command_buffers = [*command_buffer];
                let submit = vk::SubmitInfo::default().command_buffers(&command_buffers);
                unsafe {
                    self.ctx
                        .device
                        .queue_submit(self.ctx.queue, &[submit], fences[index])?;
                }
                base += count;
                remaining -= count;
                submitted += count;
            }
            // Queue submissions execute in order, so the last fence implies all
            // of them (and therefore the whole accumulation) have completed.
            if let Some(last) = fences.last() {
                unsafe {
                    self.ctx.device.wait_for_fences(&[*last], true, u64::MAX)?;
                }
            }
            Ok(())
        })();
        let wall_seconds = wall_start.elapsed().as_secs_f64();

        let gpu_ns = if result.is_ok() && measure_gpu {
            Some(self.read_benchmark_timestamps(dispatches as usize)?)
        } else {
            None
        };

        for fence in &fences {
            self.ctx.destroy_fence(*fence);
        }
        unsafe {
            for _ in &command_buffers {
                self.ctx
                    .resources
                    .destroyed(crate::vk::ResourceKind::CommandBuffer);
            }
            self.ctx
                .device
                .free_command_buffers(self.ctx.command_pool, &command_buffers);
        }
        result?;

        self.accumulated_samples += submitted;
        if self.render_started.is_none() && submitted > 0 {
            self.render_started = Some(Instant::now());
        }
        if let Some(ns) = gpu_ns {
            self.gpu_time_ns += ns;
        }

        Ok(DispatchStats {
            dispatched_samples: submitted,
            dispatches,
            gpu_ns,
            wall_seconds,
        })
    }

    // --- resource management ----------------------------------------------

    fn create_swapchain(&mut self, old: Option<vk::SwapchainKHR>) -> Result<()> {
        let surface = self
            .surface
            .as_ref()
            .ok_or_else(|| Error::Message("no surface".into()))?;
        let desired = surface.extent;
        let swapchain = Swapchain::create(
            &self.ctx,
            surface.surface,
            &surface.loader,
            self.pipelines.render_pass,
            desired,
            old,
        )?;
        if swapchain.format != self.pipelines.format {
            // The chosen swapchain format changed (e.g. a different monitor):
            // rebuild the format-dependent pipelines and descriptor sets.
            self.ctx.wait_idle()?;
            self.pipelines.destroy(&self.ctx);
            self.pipelines = Pipelines::create(&self.ctx, swapchain.format)?;
            self.update_descriptors()?;
        }
        self.swapchain = Some(swapchain);
        self.swapchain_dirty = false;
        Ok(())
    }

    /// Recreates the swapchain (and, when the size changed, the accumulation
    /// image).  Waits for the GPU first so nothing is destroyed while in use.
    fn recreate_swapchain(&mut self) -> Result<()> {
        self.ctx.wait_idle()?;
        let old = self.swapchain.take();
        let old_handle = old.as_ref().map(|s| s.handle);
        let extent = self
            .surface
            .as_ref()
            .map(|s| s.extent)
            .unwrap_or(self.headless_extent);
        let result = self.create_swapchain(old_handle);
        // Destroying the old swapchain is safe now: the device is idle and the
        // new swapchain no longer references it.
        if let Some(old) = old {
            old.destroy(&self.ctx);
        }
        result?;

        // Resolution changed => accumulated samples are meaningless.
        let extent = if extent.width == 0 || extent.height == 0 {
            self.render_extent()
        } else {
            extent
        };
        self.ensure_accum_extent(extent.width, extent.height)?;
        self.ensure_readback_capacity(self.render_extent())?;
        self.update_descriptors()?;
        self.clear_accumulation()?;
        self.accumulated_samples = 0;
        self.render_started = None;
        self.gpu_time_ns = 0;
        self.last_batch_ns = None;
        Ok(())
    }

    fn ensure_accum_extent(&mut self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            return msg("cannot create a zero-sized accumulation image");
        }
        if width > self.ctx.info.max_image_dimension_2d || height > self.ctx.info.max_image_dimension_2d
        {
            return Err(Error::Unsupported(format!(
                "requested resolution {width}x{height} exceeds maxImageDimension2D={}",
                self.ctx.info.max_image_dimension_2d
            )));
        }
        if self.accum.extent.width == width && self.accum.extent.height == height {
            return Ok(());
        }
        self.ctx.wait_idle()?;
        let new_image = GpuImage::new(
            &self.ctx,
            vk::Format::R32G32B32A32_SFLOAT,
            vk::Extent2D { width, height },
            vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST,
            vk::ImageAspectFlags::COLOR,
        )?;
        let old = std::mem::replace(&mut self.accum, new_image);
        old.destroy(&self.ctx);
        self.accum_extent = vk::Extent2D { width, height };
        self.accum_layout = vk::ImageLayout::UNDEFINED;
        self.update_descriptors()?;
        self.clear_accumulation()?;
        Ok(())
    }

    fn ensure_readback_capacity(&mut self, extent: vk::Extent2D) -> Result<()> {
        let required = readback_size(extent);
        if self.readback_buffer.size >= required {
            return Ok(());
        }
        self.ctx.wait_idle()?;
        let new_buffer = GpuBuffer::host_visible(
            &self.ctx,
            required,
            vk::BufferUsageFlags::TRANSFER_DST,
        )?;
        let old = std::mem::replace(&mut self.readback_buffer, new_buffer);
        old.destroy(&self.ctx);
        Ok(())
    }

    fn ensure_ui_capacity(&mut self, vertices: usize) -> Result<()> {
        if vertices <= self.ui_capacity {
            return Ok(());
        }
        let capacity = vertices.next_power_of_two();
        self.ctx.wait_idle()?;
        let new_buffer = GpuBuffer::host_visible(
            &self.ctx,
            (capacity * std::mem::size_of::<UiVertex>()) as u64,
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )?;
        let old = std::mem::replace(&mut self.ui_vertex_buffer, new_buffer);
        old.destroy(&self.ctx);
        self.ui_capacity = capacity;
        Ok(())
    }

    fn update_descriptors(&self) -> Result<()> {
        self.pipelines.update_trace_set(
            &self.ctx,
            self.accum.view,
            self.sphere_buffer.buffer,
            self.sphere_buffer.size,
            self.triangle_buffer.buffer,
            self.triangle_buffer.size,
            self.uniform_buffer.buffer,
            self.uniform_buffer.size,
            self.test_ray_buffer.buffer,
            self.test_ray_buffer.size,
            self.test_result_buffer.buffer,
            self.test_result_buffer.size,
        );
        self.pipelines
            .update_present_set(&self.ctx, self.accum.view, self.sampler_nearest);
        self.pipelines.update_ui_set(
            &self.ctx,
            self.font_atlas.view,
            self.sampler_nearest,
        );
        Ok(())
    }

    fn clear_accumulation(&mut self) -> Result<()> {
        let image = self.accum.image;
        let old_layout = self.accum_layout;
        let device = &self.ctx.device;
        self.ctx.one_shot(|cmd| unsafe {
            let barrier_to_clear = vk::ImageMemoryBarrier::default()
                .old_layout(old_layout)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(crate::vk::resources::subresource_range(
                    vk::ImageAspectFlags::COLOR,
                ))
                .src_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier_to_clear],
            );
            let clear = vk::ClearColorValue {
                float32: [0.0, 0.0, 0.0, 0.0],
            };
            device.cmd_clear_color_image(
                cmd,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &clear,
                &[crate::vk::resources::subresource_range(
                    vk::ImageAspectFlags::COLOR,
                )],
            );
            let barrier_to_general = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(crate::vk::resources::subresource_range(
                    vk::ImageAspectFlags::COLOR,
                ))
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE);
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier_to_general],
            );
        })?;
        self.accum_layout = vk::ImageLayout::GENERAL;
        Ok(())
    }

    // --- recording ---------------------------------------------------------

    /// Records the accumulation dispatch plus the barriers around it.
    /// `query_pair` selects a timestamp query pair to bracket the dispatch.
    fn record_dispatch(
        &mut self,
        command_buffer: vk::CommandBuffer,
        sample_base: u32,
        sample_count: u32,
        query_pair: Option<u32>,
    ) -> Result<()> {
        let uniform = self.scene.uniform(
            &self.camera.basis(self.accum_extent.width, self.accum_extent.height),
            self.settings.seed,
            self.settings.max_reflections,
        );
        self.uniform_buffer.write_bytes(bytemuck::bytes_of(&uniform));

        let device = &self.ctx.device;
        let (src_stage, src_access) = match self.accum_layout {
            vk::ImageLayout::UNDEFINED => (vk::PipelineStageFlags::TOP_OF_PIPE, vk::AccessFlags::empty()),
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL => (
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::AccessFlags::SHADER_READ,
            ),
            _ => (
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::AccessFlags::SHADER_WRITE,
            ),
        };
        image_barrier(
            device,
            command_buffer,
            self.accum.image,
            self.accum_layout,
            vk::ImageLayout::GENERAL,
            src_stage,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            src_access,
            vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE,
        );
        self.accum_layout = vk::ImageLayout::GENERAL;

        let (query_begin, query_end) = match query_pair {
            Some(pair) => (Some(pair * 2), Some(pair * 2 + 1)),
            None => (None, None),
        };

        let push = TracePushConstants {
            sample_base,
            sample_count,
            total_samples: self.settings.samples_per_pixel,
            // The 1 spp preset uses exact pixel centers; every higher preset
            // uses deterministic stratified subpixel jitter.
            stratified: u32::from(self.settings.samples_per_pixel > 1),
        };
        unsafe {
            device.cmd_bind_pipeline(
                command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                self.pipelines.raytrace_pipeline,
            );
            device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                self.pipelines.trace_pipeline_layout,
                0,
                &[self.pipelines.trace_set],
                &[],
            );
            device.cmd_push_constants(
                command_buffer,
                self.pipelines.trace_pipeline_layout,
                vk::ShaderStageFlags::COMPUTE,
                0,
                bytemuck::bytes_of(&push),
            );
            if let Some(query) = query_begin {
                device.cmd_write_timestamp(
                    command_buffer,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    self.query_pool,
                    query,
                );
            }
            let groups_x = self.accum_extent.width.div_ceil(WORKGROUP);
            let groups_y = self.accum_extent.height.div_ceil(WORKGROUP);
            device.cmd_dispatch(command_buffer, groups_x, groups_y, 1);
            if let Some(query) = query_end {
                device.cmd_write_timestamp(
                    command_buffer,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    self.query_pool,
                    query,
                );
            }
        }
        Ok(())
    }

    fn record_presentation(
        &mut self,
        command_buffer: vk::CommandBuffer,
        image_index: u32,
        ui_vertices: &[UiVertex],
        viewport: [f32; 2],
    ) -> Result<()> {
        let swapchain = self
            .swapchain
            .as_ref()
            .ok_or_else(|| Error::Message("no swapchain".into()))?;
        let device = &self.ctx.device;
        let extent = swapchain.extent;

        image_barrier(
            device,
            command_buffer,
            self.accum.image,
            self.accum_layout,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::SHADER_WRITE,
            vk::AccessFlags::SHADER_READ,
        );
        self.accum_layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;

        let clear = [vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [0.0, 0.0, 0.0, 1.0],
            },
        }];
        let render_area = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent,
        };
        let render_pass_info = vk::RenderPassBeginInfo::default()
            .render_pass(self.pipelines.render_pass)
            .framebuffer(swapchain.framebuffers[image_index as usize])
            .render_area(render_area)
            .clear_values(&clear);

        unsafe {
            device.cmd_begin_render_pass(
                command_buffer,
                &render_pass_info,
                vk::SubpassContents::INLINE,
            );
            let viewport_state = vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: extent.width as f32,
                height: extent.height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            };
            device.cmd_set_viewport(command_buffer, 0, &[viewport_state]);
            device.cmd_set_scissor(command_buffer, 0, &[render_area]);

            // --- ray-traced image ------------------------------------------
            let sample_count = self.accumulated_samples.max(1) as f32;
            let present_push = PresentPushConstants {
                inv_sample_count: 1.0 / sample_count,
                exposure: self.settings.exposure,
                encode_srgb: if swapchain.srgb { 0.0 } else { 1.0 },
                pad: 0.0,
            };
            device.cmd_bind_pipeline(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipelines.present_pipeline,
            );
            device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipelines.present_pipeline_layout,
                0,
                &[self.pipelines.present_set],
                &[],
            );
            device.cmd_push_constants(
                command_buffer,
                self.pipelines.present_pipeline_layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                bytemuck::bytes_of(&present_push),
            );
            device.cmd_draw(command_buffer, 3, 1, 0, 0);

            // --- UI overlay --------------------------------------------------
            if !ui_vertices.is_empty() {
                self.ui_vertex_buffer
                    .write_bytes(bytemuck::cast_slice(ui_vertices));
                let ui_push = UiPushConstants {
                    viewport,
                    pad: [0.0, 0.0],
                };
                device.cmd_bind_pipeline(
                    command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipelines.ui_pipeline,
                );
                device.cmd_bind_descriptor_sets(
                    command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipelines.ui_pipeline_layout,
                    0,
                    &[self.pipelines.ui_set],
                    &[],
                );
                device.cmd_push_constants(
                    command_buffer,
                    self.pipelines.ui_pipeline_layout,
                    vk::ShaderStageFlags::VERTEX,
                    0,
                    bytemuck::bytes_of(&ui_push),
                );
                device.cmd_bind_vertex_buffers(
                    command_buffer,
                    0,
                    &[self.ui_vertex_buffer.buffer],
                    &[0],
                );
                device.cmd_draw(command_buffer, ui_vertices.len() as u32, 1, 0, 0);
            }

            device.cmd_end_render_pass(command_buffer);
        }
        Ok(())
    }

    /// Submits an empty command buffer after presentation; the fence it signals
    /// is the frame's guard (see `frame`).
    fn submit_guard(&mut self, slot_index: usize) -> Result<()> {
        let device = &self.ctx.device;
        let command_buffer = self.slots[slot_index].guard_command_buffer;
        unsafe {
            device.reset_command_buffer(command_buffer, vk::CommandBufferResetFlags::empty())?;
            device.begin_command_buffer(
                command_buffer,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            device.end_command_buffer(command_buffer)?;
            let command_buffers = [command_buffer];
            let submit = vk::SubmitInfo::default().command_buffers(&command_buffers);
            device.queue_submit(
                self.ctx.queue,
                &[submit],
                self.slots[slot_index].fence,
            )?;
        }
        Ok(())
    }

    // --- timestamps --------------------------------------------------------

    fn collect_batch_timestamps(&mut self, slot_index: usize) -> Result<Option<u64>> {
        if !self.slots[slot_index].pending_query {
            return Ok(None);
        }
        self.slots[slot_index].pending_query = false;
        let mut data = [0u64; 2];
        unsafe {
            self.ctx.device.get_query_pool_results(
                self.query_pool,
                0,
                &mut data,
                vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
            )?;
        }
        Ok(Some(timestamp_delta_ns(
            data[0],
            data[1],
            self.ctx.info.timestamp_period_ns,
        )))
    }

    fn read_benchmark_timestamps(&self, dispatches: usize) -> Result<u64> {
        let mut data = vec![0u64; dispatches * 2];
        unsafe {
            self.ctx.device.get_query_pool_results(
                self.query_pool,
                0,
                &mut data,
                vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
            )?;
        }
        let mut total = 0u64;
        for pair in data.chunks_exact(2) {
            total += timestamp_delta_ns(
                pair[0],
                pair[1],
                self.ctx.info.timestamp_period_ns,
            );
        }
        Ok(total)
    }

    // --- readback / export -------------------------------------------------

    /// Copies the accumulation image into host memory and returns the raw
    /// (un-averaged) linear RGB sums as `f32` RGBA quadruples.
    pub fn read_accum_rgba32f(&mut self) -> Result<Vec<f32>> {
        self.ctx.wait_idle()?;
        let extent = self.accum_extent;
        let bytes = readback_size(extent);
        self.ensure_readback_capacity(extent)?;
        let image = self.accum.image;
        let buffer = self.readback_buffer.buffer;
        let old_layout = self.accum_layout;
        let device = &self.ctx.device;
        self.ctx.one_shot(|cmd| unsafe {
            let to_src = vk::ImageMemoryBarrier::default()
                .old_layout(old_layout)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(crate::vk::resources::subresource_range(
                    vk::ImageAspectFlags::COLOR,
                ))
                .src_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_src],
            );
            let region = vk::BufferImageCopy::default()
                .buffer_row_length(0)
                .buffer_image_height(0)
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_extent(vk::Extent3D {
                    width: extent.width,
                    height: extent.height,
                    depth: 1,
                });
            device.cmd_copy_image_to_buffer(
                cmd,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[region],
            );
            let back = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .new_layout(old_layout)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(crate::vk::resources::subresource_range(
                    vk::ImageAspectFlags::COLOR,
                ))
                .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE);
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[back],
            );
        })?;
        let raw = self.readback_buffer.read_bytes(bytes as usize);
        let mut values = vec![0f32; (extent.width * extent.height * 4) as usize];
        for (index, chunk) in raw.chunks_exact(4).enumerate() {
            values[index] = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        Ok(values)
    }

    /// Applies the display colour processing and writes a PNG at render
    /// resolution (no UI overlays).
    pub fn save_png(&mut self, directory: &Path) -> Result<ExportInfo> {
        let extent = self.accum_extent;
        let sums = self.read_accum_rgba32f()?;
        let sample_count = self.accumulated_samples;
        let mut pixels = vec![0u8; (extent.width * extent.height * 4) as usize];
        for (index, chunk) in pixels.chunks_exact_mut(4).enumerate() {
            let base = index * 4;
            let rgb = color::linear_sum_to_srgb8(
                [sums[base], sums[base + 1], sums[base + 2]],
                sample_count,
                self.settings.exposure,
            );
            chunk[0] = rgb[0];
            chunk[1] = rgb[1];
            chunk[2] = rgb[2];
            chunk[3] = 255;
        }

        std::fs::create_dir_all(directory)?;
        let complete = sample_count >= self.settings.samples_per_pixel;
        let name = format!(
            "render_{}_s{}{}.png",
            crate::util::timestamp_string(),
            sample_count,
            if complete { "" } else { "_partial" },
        );
        let path = directory.join(name);
        let file = std::fs::File::create(&path)?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), extent.width, extent.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let mut writer = encoder
            .write_header()
            .map_err(|e| Error::Png(e.to_string()))?;
        writer
            .write_image_data(&pixels)
            .map_err(|e| Error::Png(e.to_string()))?;
        writer
            .finish()
            .map_err(|e| Error::Png(e.to_string()))?;

        Ok(ExportInfo {
            path,
            width: extent.width,
            height: extent.height,
            accumulated_samples: sample_count,
            target_samples: self.settings.samples_per_pixel,
            complete,
            exposure: self.settings.exposure,
            decoded_pixel_sha256: color::hash_pixels_rgba8(&pixels),
        })
    }

    /// Encodes the current accumulation without touching the disk (used by the
    /// determinism self-test).
    pub fn decode_pixels(&mut self) -> Result<(Vec<u8>, u32, u32, u32)> {
        let extent = self.accum_extent;
        let sums = self.read_accum_rgba32f()?;
        let sample_count = self.accumulated_samples;
        let mut pixels = vec![0u8; (extent.width * extent.height * 4) as usize];
        for (index, chunk) in pixels.chunks_exact_mut(4).enumerate() {
            let base = index * 4;
            let rgb = color::linear_sum_to_srgb8(
                [sums[base], sums[base + 1], sums[base + 2]],
                sample_count,
                self.settings.exposure,
            );
            chunk[0] = rgb[0];
            chunk[1] = rgb[1];
            chunk[2] = rgb[2];
            chunk[3] = 255;
        }
        Ok((pixels, extent.width, extent.height, sample_count))
    }

    // --- GPU intersection tests -------------------------------------------

    /// Runs `shaders/intersect_test.comp` over the supplied rays and reads the
    /// results back to the CPU.
    pub fn run_intersection_tests(&mut self, rays: &[TestRay]) -> Result<Vec<TestResult>> {
        if rays.len() > TEST_RAY_CAPACITY {
            return msg(format!(
                "intersection test supports at most {TEST_RAY_CAPACITY} rays, got {}",
                rays.len()
            ));
        }
        self.ctx.wait_idle()?;
        // Keep the uniform block consistent with the current camera/seed.
        let uniform = self.scene.uniform(
            &self.camera.basis(self.accum_extent.width, self.accum_extent.height),
            self.settings.seed,
            self.settings.max_reflections,
        );
        self.uniform_buffer.write_bytes(bytemuck::bytes_of(&uniform));
        self.test_ray_buffer.write_bytes(bytemuck::cast_slice(rays));
        let result_stride = 80usize;
        self.test_result_buffer
            .write_bytes(&vec![0u8; rays.len() * result_stride]);

        let device = &self.ctx.device;
        let ray_buffer = self.test_ray_buffer.buffer;
        let result_buffer = self.test_result_buffer.buffer;
        let ray_count = rays.len() as u32;
        let pipeline = self.pipelines.intersect_test_pipeline;
        let layout = self.pipelines.trace_pipeline_layout;
        let set = self.pipelines.trace_set;
        self.ctx.one_shot(|cmd| unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                layout,
                0,
                &[set],
                &[],
            );
            device.cmd_push_constants(
                cmd,
                layout,
                vk::ShaderStageFlags::COMPUTE,
                0,
                bytemuck::bytes_of(&TracePushConstants {
                    sample_base: ray_count,
                    sample_count: 0,
                    total_samples: 0,
                    stratified: 0,
                }),
            );
            device.cmd_dispatch(cmd, ray_count.div_ceil(64), 1, 1);
        })?;

        let raw = self.test_result_buffer.read_bytes(rays.len() * result_stride);
        let mut results = Vec::with_capacity(rays.len());
        for chunk in raw.chunks_exact(result_stride) {
            // TestResult layout: 4 x vec4 (t_hit, position, normal, color) then
            // a uvec4 (prim_kind, prim_index, any_hit, unused).
            let floats: Vec<f32> = chunk[..64]
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            let prim_info: Vec<u32> = chunk[64..80]
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            results.push(TestResult {
                hit: floats[1] > 0.5,
                t: floats[0],
                position: [floats[4], floats[5], floats[6]],
                normal: [floats[8], floats[9], floats[10]],
                color: [floats[12], floats[13], floats[14]],
                prim_kind: prim_info[0],
                prim_index: prim_info[1],
                any_hit: prim_info[2] != 0,
                mode: floats[2] as u32,
            });
        }
        let _ = (ray_buffer, result_buffer);
        Ok(results)
    }

    pub fn wait_idle(&self) -> Result<()> {
        self.ctx.wait_idle()
    }

    /// Headless accumulation resolution.
    pub fn set_headless_extent(&mut self, width: u32, height: u32) {
        self.headless_extent = vk::Extent2D { width, height };
    }

    /// Forces the accumulation resolution (used by benchmark mode so the render
    /// resolution is independent of the window size) and clears the image.
    pub fn set_render_resolution(&mut self, width: u32, height: u32) -> Result<()> {
        self.headless_extent = vk::Extent2D { width, height };
        self.ensure_accum_extent(width, height)?;
        self.ensure_readback_capacity(vk::Extent2D { width, height })?;
        self.accumulated_samples = 0;
        self.render_started = None;
        self.gpu_time_ns = 0;
        Ok(())
    }

    pub fn accum_extent(&self) -> vk::Extent2D {
        self.accum_extent
    }

}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            let _ = self.ctx.device.device_wait_idle();
            for slot in &self.slots {
                self.ctx.destroy_fence(slot.fence);
                self.ctx.destroy_semaphore(slot.image_available);
                self.ctx.free_command_buffer(slot.command_buffer);
                self.ctx.free_command_buffer(slot.guard_command_buffer);
            }
            self.ctx.device.destroy_query_pool(self.query_pool, None);
            self.ctx.resources.destroyed(crate::vk::ResourceKind::QueryPool);
            self.ctx.device.destroy_sampler(self.sampler_nearest, None);
            self.ctx.resources.destroyed(crate::vk::ResourceKind::Sampler);
            if let Some(swapchain) = self.swapchain.take() {
                swapchain.destroy(&self.ctx);
            }
            self.pipelines.destroy(&self.ctx);
            self.accum.destroy(&self.ctx);
            self.font_atlas.destroy(&self.ctx);
            self.sphere_buffer.destroy(&self.ctx);
            self.triangle_buffer.destroy(&self.ctx);
            self.uniform_buffer.destroy(&self.ctx);
            self.test_ray_buffer.destroy(&self.ctx);
            self.test_result_buffer.destroy(&self.ctx);
            self.readback_buffer.destroy(&self.ctx);
            self.ui_vertex_buffer.destroy(&self.ctx);
            if let Some(surface) = self.surface.take() {
                surface.loader.destroy_surface(surface.surface, None);
            }
        }
    }
}

/// Picks the swapchain format up front so the render pass and graphics
/// pipelines can be built before the swapchain itself exists.
fn swapchain_format_hint(ctx: &Context, surface: Option<&SurfaceContext>) -> Result<vk::Format> {
    let surface = surface.ok_or_else(|| Error::Message("no surface".into()))?;
    let formats = unsafe {
        surface
            .loader
            .get_physical_device_surface_formats(ctx.physical_device, surface.surface)?
    };
    if formats.is_empty() {
        return msg("the surface reports no supported formats");
    }
    Ok(formats
        .iter()
        .find(|f| {
            f.format == vk::Format::B8G8R8A8_SRGB
                && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
        })
        .or_else(|| {
            formats
                .iter()
                .find(|f| f.format == vk::Format::R8G8B8A8_SRGB
                    && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR)
        })
        .or_else(|| {
            formats
                .iter()
                .find(|f| f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR)
        })
        .unwrap_or(&formats[0])
        .format)
}

fn readback_size(extent: vk::Extent2D) -> u64 {
    extent.width as u64 * extent.height as u64 * 16
}

fn timestamp_delta_ns(begin: u64, end: u64, period_ns: f32) -> u64 {
    ((end.saturating_sub(begin)) as f64 * period_ns as f64) as u64
}

pub fn validate_samples(samples: u32) -> Result<u32> {
    match samples {
        1 | 16 | 64 => Ok(samples),
        other => msg(format!(
            "unsupported samples-per-pixel preset {other}; use one of {SAMPLE_PRESETS:?}"
        )),
    }
}

/// Convenience: default renderer options with the benchmark settings applied.
pub fn benchmark_options() -> RendererOptions {
    RendererOptions {
        samples_per_pixel: Some(64),
        max_reflections: Some(4),
        exposure: Some(1.0),
        seed: Some(DEFAULT_SEED),
        ..Default::default()
    }
}

/// Context options mirroring the renderer options.
pub fn context_options(options: &RendererOptions, require_presentation: bool) -> ContextOptions {
    ContextOptions {
        validation: options.validation,
        gpu_name_filter: options.gpu_name_filter.clone(),
        require_presentation,
    }
}

/// Small helper used by the app to describe the scene for the UI/report.
pub fn scene_summary(scene: &Scene) -> String {
    format!(
        "{} spheres, {} triangles, light at ({:.1},{:.1},{:.1}) intensity {:.0}",
        scene.sphere_count(),
        scene.triangle_count(),
        scene.light_position.x,
        scene.light_position.y,
        scene.light_position.z,
        scene.light_intensity.x
    )
}

