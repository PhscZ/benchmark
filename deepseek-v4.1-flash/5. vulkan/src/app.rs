//! Window, input handling, on-screen interface and the interactive render loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use ash::vk;
use glam::Vec2;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::Window;

use crate::bench::{self, BenchConfig, BenchReport, PhaseTiming};
use crate::error::{Error, Result};
use crate::render::{
    self, RenderStatus, Renderer, RendererOptions,
};
use crate::ui::{Action, UiBuilder};
use crate::util;
use crate::vk::Context;

/// Exit codes.
pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_SELF_TEST_FAILED: i32 = 2;
pub const EXIT_BENCHMARK_FAILED: i32 = 3;

/// Bitmap-font scale: the atlas font is 8x8, so 1.0 = 8 px tall, 2.0 = 16 px.
/// Chosen from the framebuffer width so the panel always fits on screen.
fn ui_scale(width: f32) -> f32 {
    if width >= 1700.0 {
        2.0
    } else {
        1.0
    }
}
const PANEL_MARGIN: f32 = 12.0;
const BUTTON_GAP: f32 = 8.0;
const BUTTON_PADDING: f32 = 6.0;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub renderer: RendererOptions,
    pub benchmark: Option<BenchConfig>,
    pub self_test: bool,
    pub headless: bool,
    pub width: u32,
    pub height: u32,
    pub output_dir: PathBuf,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            renderer: RendererOptions::default(),
            benchmark: None,
            self_test: false,
            headless: false,
            width: render::DEFAULT_WIDTH,
            height: render::DEFAULT_HEIGHT,
            output_dir: PathBuf::from("renders"),
        }
    }
}

/// Entry point: runs headless or windowed depending on the configuration.
pub fn run(config: AppConfig) -> i32 {
    match run_inner(config) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            EXIT_ERROR
        }
    }
}

fn run_inner(config: AppConfig) -> Result<i32> {
    if config.headless {
        return run_headless(config);
    }
    let event_loop = EventLoop::new().map_err(|e| Error::Message(format!("event loop: {e}")))?;
    let mut app = App::new(config);
    event_loop
        .run_app(&mut app)
        .map_err(|e| Error::Message(format!("event loop: {e}")))?;
    Ok(app.exit_code)
}

/// Creates the context and renderer without any window/surface.
fn create_headless_renderer(config: &AppConfig) -> Result<Renderer> {
    let context_options = render::context_options(&config.renderer, false);
    let ctx = Context::new(&[], &context_options, None::<fn(&ash::Entry, &ash::Instance) -> Result<vk::SurfaceKHR>>)?;
    let mut renderer = Renderer::new(ctx, None, &config.renderer)?;
    renderer.set_headless_extent(config.width, config.height);
    renderer.set_render_resolution(config.width, config.height)?;
    Ok(renderer)
}

fn run_headless(config: AppConfig) -> Result<i32> {
    let init_start = Instant::now();
    let mut renderer = create_headless_renderer(&config)?;
    // Instance, device and resource creation, excluding shader/pipeline creation
    // (which `Renderer` reports separately).
    let initialization_seconds =
        (init_start.elapsed().as_secs_f64() - renderer.pipeline_creation_seconds()).max(0.0);
    println!(
        "GPU: {} | driver {} | Vulkan {} | {}",
        renderer.gpu_info().device_name,
        renderer.gpu_info().driver_version,
        renderer.gpu_info().api_version,
        renderer.gpu_info().device_type
    );

    let mut exit = EXIT_OK;
    if config.self_test {
        let report = crate::self_test::run(&mut renderer)?;
        print!("{}", report.to_text());
        if !report.passed() {
            exit = EXIT_SELF_TEST_FAILED;
        }
    }
    if let Some(bench_config) = &config.benchmark {
        let report = run_headless_benchmark(&mut renderer, bench_config, initialization_seconds)?;
        print!("{}", report.to_text());
        if !report.render_complete {
            exit = EXIT_BENCHMARK_FAILED;
        }
    }
    renderer.wait_idle()?;
    Ok(exit)
}

fn run_headless_benchmark(
    renderer: &mut Renderer,
    config: &BenchConfig,
    initialization_seconds: f64,
) -> Result<BenchReport> {
    config.validate()?;
    renderer.set_samples_per_pixel(config.samples_per_pixel)?;
    renderer.set_reflections(config.max_reflections)?;
    renderer.set_exposure(config.exposure);
    renderer.set_seed(config.seed)?;
    renderer.reset_camera()?;
    renderer.set_render_resolution(config.width, config.height)?;

    // Warm-up (unmeasured), then clear and measure.
    let warmup_start = Instant::now();
    let batch = config.samples_per_pixel.div_ceil(render::BENCHMARK_DISPATCHES).max(1);
    let warmup_stats = renderer.dispatch_batches(config.samples_per_pixel, batch, true)?;
    let warmup = PhaseTiming {
        samples: warmup_stats.dispatched_samples,
        dispatches: warmup_stats.dispatches,
        gpu_seconds: warmup_stats.gpu_ns.map(|ns| ns as f64 / 1e9),
        wall_seconds: warmup_start.elapsed().as_secs_f64(),
    };

    bench::run_measured(
        renderer,
        config,
        initialization_seconds,
        renderer.pipeline_creation_seconds(),
        warmup,
        vec!["headless run: no swapchain/presentation involved".to_string()],
    )
}

struct BenchRuntime {
    config: BenchConfig,
    phase: BenchPhase,
    warmup_start: Instant,
    warmup_dispatches: u32,
    report: Option<BenchReport>,
}

#[derive(PartialEq)]
enum BenchPhase {
    Warmup,
    Measure,
    Done,
}

struct Running {
    window: Arc<Window>,
    renderer: Renderer,
    ui: UiBuilder,
    message: String,
    message_time: Instant,
    dragging: bool,
    cursor: [f32; 2],
    last_cursor: [f32; 2],
    init_seconds: f64,
    bench: Option<BenchRuntime>,
    /// Set when the camera/settings changed and accumulation must restart.
    accum_needs_reset: bool,
    last_title: String,
}

pub struct App {
    config: AppConfig,
    running: Option<Running>,
    exit_code: i32,
    startup_done: bool,
    /// Set when the self-test should run once the renderer exists.
    self_test_pending: bool,
}

impl App {
    fn new(config: AppConfig) -> Self {
        let self_test_pending = config.self_test;
        Self {
            config,
            running: None,
            exit_code: EXIT_OK,
            startup_done: false,
            self_test_pending,
        }
    }

    fn create_running(&mut self, event_loop: &ActiveEventLoop) -> Result<Running> {
        let attributes = Window::default_attributes()
            .with_title("vk-raytracer - Vulkan compute ray tracer")
            .with_inner_size(LogicalSize::new(
                self.config.width as f64,
                self.config.height as f64,
            ))
            .with_min_inner_size(LogicalSize::new(320.0, 240.0));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|e| Error::Message(format!("cannot create a window: {e}")))?,
        );

        let display_handle = window
            .display_handle()
            .map_err(|e| Error::Message(format!("no display handle: {e}")))?
            .as_raw();
        let window_handle = window
            .window_handle()
            .map_err(|e| Error::Message(format!("no window handle: {e}")))?
            .as_raw();

        let instance_extensions = ash_window::enumerate_required_extensions(display_handle)
            .map_err(|e| Error::Message(format!("surface extensions: {e}")))?;

        let init_start = Instant::now();
        let context_options = render::context_options(&self.config.renderer, true);
        // The surface is created from inside context setup, before device
        // selection, so the chosen device is known to support presentation.
        let ctx = Context::new(instance_extensions, &context_options, Some(
            |entry: &ash::Entry, instance: &ash::Instance| -> Result<vk::SurfaceKHR> {
                Ok(unsafe {
                    ash_window::create_surface(entry, instance, display_handle, window_handle, None)?
                })
            },
        ))?;
        let size = window.inner_size();
        let renderer = Renderer::new(
            ctx,
            Some(vk::Extent2D {
                width: size.width,
                height: size.height,
            }),
            &self.config.renderer,
        )?;
        let init_seconds = init_start.elapsed().as_secs_f64();

        println!(
            "GPU: {} | driver {} | Vulkan {} | {}",
            renderer.gpu_info().device_name,
            renderer.gpu_info().driver_version,
            renderer.gpu_info().api_version,
            renderer.gpu_info().device_type
        );
        if let Some(description) = renderer.swapchain_description() {
            println!(
                "swapchain: {description} ({})",
                renderer.present_mode().unwrap_or("?")
            );
        }
        if !renderer.timestamps_supported() {
            println!("note: GPU timestamp queries are unavailable; only wall-clock timings will be reported");
        }

        let message = format!(
            "Rendering {} spp with {} reflection bounces. Drag to orbit, wheel to zoom.",
            renderer.settings().samples_per_pixel,
            renderer.settings().max_reflections
        );
        Ok(Running {
            window,
            renderer,
            ui: UiBuilder::new(size.width as f32, size.height as f32),
            message,
            message_time: Instant::now(),
            dragging: false,
            cursor: [0.0, 0.0],
            last_cursor: [0.0, 0.0],
            init_seconds,
            bench: None,
            accum_needs_reset: false,
            last_title: String::new(),
        })
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.startup_done {
            return;
        }
        self.startup_done = true;
        let running = match self.create_running(event_loop) {
            Ok(running) => running,
            Err(e) => {
                // Report the unsupported capability clearly; never fall back to
                // CPU rendering.
                eprintln!("error: {e}");
                self.exit_code = EXIT_ERROR;
                event_loop.exit();
                return;
            }
        };
        self.running = Some(running);

        if self.self_test_pending {
            self.self_test_pending = false;
            let result = self
                .running
                .as_mut()
                .expect("running")
                .renderer
                .wait_idle()
                .and_then(|_| {
                    let running = self.running.as_mut().expect("running");
                    crate::self_test::run(&mut running.renderer)
                });
            match result {
                Ok(report) => {
                    print!("{}", report.to_text());
                    if !report.passed() {
                        self.exit_code = EXIT_SELF_TEST_FAILED;
                        event_loop.exit();
                        return;
                    }
                    // `--self-test` is a verification command: report and exit,
                    // unless a benchmark was requested in the same run.
                    if self.config.benchmark.is_none() {
                        event_loop.exit();
                        return;
                    }
                }
                Err(e) => {
                    eprintln!("error: self-test failed to run: {e}");
                    self.exit_code = EXIT_ERROR;
                    event_loop.exit();
                    return;
                }
            }
        }

        if let Some(bench_config) = self.config.benchmark.clone() {
            if let Some(running) = self.running.as_mut() {
                Self::start_benchmark(running, bench_config);
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        let Some(running) = self.running.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                running
                    .renderer
                    .resize_surface(size.width, size.height);
                running.ui.reset(size.width as f32, size.height as f32);
                running.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = running.window.inner_size();
                running.renderer.resize_surface(size.width, size.height);
                running.window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                running.cursor = [position.x as f32, position.y as f32];
                if running.dragging {
                    let delta = Vec2::new(
                        running.cursor[0] - running.last_cursor[0],
                        running.cursor[1] - running.last_cursor[1],
                    );
                    if delta != Vec2::ZERO {
                        running.renderer.camera_mut().orbit(delta);
                        running.accum_needs_reset = true;
                    }
                }
                running.last_cursor = running.cursor;
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Left {
                    match state {
                        ElementState::Pressed => {
                            if let Some(action) = running.ui.hit_test(running.cursor) {
                                self.apply_action(action, event_loop);
                            } else {
                                running.dragging = true;
                            }
                        }
                        ElementState::Released => {
                            running.dragging = false;
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(position) => position.y as f32 / 50.0,
                };
                if notches != 0.0 {
                    running.renderer.camera_mut().zoom(notches);
                    running.accum_needs_reset = true;
                    running.window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    if let PhysicalKey::Code(code) = event.physical_key {
                        self.handle_key(code, event.repeat, event_loop);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.redraw(event_loop) {
                    eprintln!("error: {e}");
                    self.exit_code = EXIT_ERROR;
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(running) = self.running.as_ref() else {
            return;
        };
        let busy = running.renderer.status() != RenderStatus::Complete
            || running
                .bench
                .as_ref()
                .is_some_and(|b| b.phase != BenchPhase::Done);
        if busy {
            event_loop.set_control_flow(ControlFlow::Poll);
            running.window.request_redraw();
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

impl App {
    fn handle_key(&mut self, code: KeyCode, repeat: bool, event_loop: &ActiveEventLoop) {
        let action = match code {
            KeyCode::Digit1 => Some(Action::SetSamples(1)),
            KeyCode::Digit2 => Some(Action::SetSamples(16)),
            KeyCode::Digit3 => Some(Action::SetSamples(64)),
            KeyCode::KeyR => Some(Action::ReflectionsDelta(-1)),
            KeyCode::KeyT => Some(Action::ReflectionsDelta(1)),
            KeyCode::KeyE => Some(Action::ExposureDelta(-1)),
            KeyCode::KeyD => Some(Action::ExposureDelta(1)),
            KeyCode::Space => Some(Action::Restart),
            KeyCode::KeyC => Some(Action::ResetCamera),
            KeyCode::KeyP => Some(Action::SavePng),
            KeyCode::KeyX => Some(Action::TogglePause),
            KeyCode::KeyB => Some(Action::RunBenchmark),
            KeyCode::Escape => Some(Action::Quit),
            _ => None,
        };
        let Some(action) = action else { return };
        if repeat && !matches!(action, Action::ExposureDelta(_) | Action::ReflectionsDelta(_)) {
            return;
        }
        self.apply_action(action, event_loop);
    }

    fn start_benchmark(running: &mut Running, config: BenchConfig) {
        let setup = (|| -> Result<()> {
            running.renderer.set_samples_per_pixel(config.samples_per_pixel)?;
            running.renderer.set_reflections(config.max_reflections)?;
            running.renderer.set_exposure(config.exposure);
            running.renderer.set_seed(config.seed)?;
            running.renderer.reset_camera()?;
            running.renderer.set_render_resolution(config.width, config.height)?;
            running.renderer.restart()
        })();
        match setup {
            Ok(()) => {
                running.bench = Some(BenchRuntime {
                    config,
                    phase: BenchPhase::Warmup,
                    warmup_start: Instant::now(),
                    warmup_dispatches: 0,
                    report: None,
                });
                running.message =
                    "benchmark: warm-up render (unmeasured) at 1280x720, 64 spp, 4 bounces, seed 12345"
                    .to_string();
                running.message_time = Instant::now();
                running.window.request_redraw();
            }
            Err(e) => {
                running.message = format!("benchmark could not start: {e}");
                running.message_time = Instant::now();
            }
        }
    }

    fn apply_action(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        let Some(running) = self.running.as_mut() else {
            return;
        };
        let mut redraw = true;
        let result: Result<()> = match action {
            Action::SetSamples(samples) => running.renderer.set_samples_per_pixel(samples).map(|_| {
                running.message = format!("samples-per-pixel preset: {samples} (accumulation reset)");
            }),
            Action::ReflectionsDelta(delta) => running.renderer.adjust_reflections(delta).map(|_| {
                running.message = format!(
                    "reflection bounces: {} (accumulation reset)",
                    running.renderer.settings().max_reflections
                );
            }),
            Action::ExposureDelta(steps) => {
                running.renderer.adjust_exposure(steps);
                running.message = format!(
                    "exposure: {:.3} (accumulated samples reused)",
                    running.renderer.settings().exposure
                );
                Ok(())
            }
            Action::Restart => running.renderer.restart().map(|_| {
                running.message = "rendering restarted".to_string();
            }),
            Action::ResetCamera => running.renderer.reset_camera().map(|_| {
                running.message = "camera reset to the default view".to_string();
            }),
            Action::TogglePause => {
                running.renderer.toggle_paused();
                running.message = if running.renderer.is_paused() {
                    "accumulation paused".to_string()
                } else {
                    "accumulation resumed".to_string()
                };
                Ok(())
            }
            Action::SavePng => {
                let dir = self.config.output_dir.clone();
                match running.renderer.save_png(&dir) {
                    Ok(info) => {
                        running.message = format!(
                            "saved {} ({}x{}, {}/{} samples, {}) sha256 {}",
                            info.path.display(),
                            info.width,
                            info.height,
                            info.accumulated_samples,
                            info.target_samples,
                            if info.complete {
                                "complete render"
                            } else {
                                "PARTIAL render"
                            },
                            &info.decoded_pixel_sha256[..16]
                        );
                        println!("{}", running.message);
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }
            Action::RunBenchmark => {
                let config = self.config.benchmark.clone().unwrap_or_default();
                redraw = false;
                Self::start_benchmark(running, config);
                Ok(())
            }
            Action::Quit => {
                event_loop.exit();
                redraw = false;
                Ok(())
            }
        };
        if let Err(e) = result {
            running.message = format!("error: {e}");
            running.message_time = Instant::now();
        } else if running.message_time.elapsed().as_secs_f32() >= 0.0 {
            running.message_time = Instant::now();
        }
        if redraw {
            running.window.request_redraw();
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let Some(running) = self.running.as_mut() else {
            return Ok(());
        };

        // Advance benchmark phases.
        if let Some(bench) = running.bench.as_mut() {
            match bench.phase {
                BenchPhase::Warmup => {
                    if running.renderer.status() == RenderStatus::Complete {
                        bench.phase = BenchPhase::Measure;
                    }
                }
                BenchPhase::Measure => {
                    let warmup = PhaseTiming {
                        samples: running.renderer.accumulated_samples(),
                        dispatches: bench.warmup_dispatches,
                        gpu_seconds: Some(running.renderer.gpu_time_ns() as f64 / 1e9),
                        wall_seconds: bench.warmup_start.elapsed().as_secs_f64(),
                    };
                    let config = bench.config.clone();
                    let init = running.init_seconds;
                    let pipelines = running.renderer.pipeline_creation_seconds();
                    let report = bench::run_measured(
                        &mut running.renderer,
                        &config,
                        init,
                        pipelines,
                        warmup,
                        vec![
                            "window mode: the measured phase does not present between dispatches, so wall-clock time is not inflated by presentation".to_string(),
                            "window mode: the GPU timestamp interval spans contention with desktop composition on the same GPU, so it is higher and less stable than the headless figure (measured 22-28 ms windowed vs 13.2 ms headless for identical compute work); run --no-window for the uncontended GPU time".to_string(),
                        ],
                    )?;
                    print!("{}", report.to_text());
                    let summary = format!(
                        "benchmark: GPU {:.4} s, wall {:.4} s, {} dispatches, PNG {}",
                        report.measured.gpu_seconds.unwrap_or(0.0),
                        report.measured.wall_seconds,
                        report.measured.dispatches,
                        report.png_path
                    );
                    bench.report = Some(report);
                    bench.phase = BenchPhase::Done;
                    running.message = summary;
                    running.message_time = Instant::now();
                    if running
                        .bench
                        .as_ref()
                        .and_then(|b| b.report.as_ref())
                        .is_some_and(|r| !r.render_complete)
                    {
                        self.exit_code = EXIT_BENCHMARK_FAILED;
                    }
                    // A benchmark requested from the command line exits once the
                    // report is written; pressing `B` in the window does not.
                    if self.config.benchmark.is_some() {
                        event_loop.exit();
                        return Ok(());
                    }
                }
                BenchPhase::Done => {}
            }
        }

        if running.accum_needs_reset {
            running.renderer.restart()?;
            running.accum_needs_reset = false;
        }

        let size = running.window.inner_size();
        if size.width == 0 || size.height == 0 {
            // Minimized: nothing to draw and nothing to present.
            return Ok(());
        }

        build_interface(
            &mut running.ui,
            &running.renderer,
            &running.message,
            size.width as f32,
            size.height as f32,
        );
        let viewport = running.ui.viewport();
        let outcome = running.renderer.frame(&running.ui.vertices, viewport)?;
        if outcome.dispatched_samples > 0 {
            if let Some(bench) = running.bench.as_mut() {
                if bench.phase == BenchPhase::Warmup {
                    bench.warmup_dispatches += 1;
                }
            }
        }

        let title = format!(
            "vk-raytracer - {} - {} spp {}/{} - {}x{}",
            running.renderer.gpu_info().device_name,
            running.renderer.settings().samples_per_pixel,
            running.renderer.accumulated_samples(),
            running.renderer.settings().samples_per_pixel,
            outcome.extent.width,
            outcome.extent.height
        );
        if title != running.last_title {
            running.window.set_title(&title);
            running.last_title = title;
        }

        if running.bench.as_ref().is_some_and(|b| b.phase == BenchPhase::Warmup)
            && running.renderer.status() == RenderStatus::Complete
        {
            running.window.request_redraw();
        }
        let _ = event_loop;
        Ok(())
    }
}

/// Draws the information panel, the controls and the status message.
fn build_interface(ui: &mut UiBuilder, renderer: &Renderer, message: &str, width: f32, height: f32) {
    ui.reset(width, height);
    let scale = ui_scale(width);
    // Characters that fit inside the margins at this scale.
    let max_chars = (((width - 2.0 * PANEL_MARGIN - 16.0) / (8.0 * scale)).floor()).max(16.0) as usize;
    let clip = |text: String| -> String {
        if text.chars().count() > max_chars {
            let mut out: String = text.chars().take(max_chars.saturating_sub(3)).collect();
            out.push_str("...");
            out
        } else {
            text
        }
    };
    let settings = renderer.settings();
    let extent = renderer.accum_extent();
    let status = renderer.status();
    let info = renderer.gpu_info();

    let mut lines = vec![
        format!("GPU: {}", info.device_name),
        format!(
            "Driver: {} {} (Vulkan {})",
            info.driver_name, info.driver_version, info.api_version
        ),
        format!(
            "Device: {} | {} | timestamps: {}",
            info.device_type,
            renderer.swapchain_description().unwrap_or_else(|| "headless".to_string()),
            if renderer.timestamps_supported() { "yes" } else { "no" }
        ),
        format!(
            "Resolution: {}x{} | samples: {}/{} ({})",
            extent.width,
            extent.height,
            renderer.accumulated_samples(),
            settings.samples_per_pixel,
            status.label()
        ),
        format!(
            "Reflections: {} | Exposure: {:.2} | Seed: {}",
            settings.max_reflections, settings.exposure, settings.seed
        ),
        format!(
            "Render time: {} | GPU total: {} | last batch: {}",
            util::format_seconds(renderer.elapsed_seconds()),
            util::format_ns(renderer.gpu_time_ns()),
            renderer
                .last_batch_ns()
                .map(util::format_ns)
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "Camera: {} | drag = orbit, wheel = zoom (limits {:.1}-{:.1} units, +-{:.0} deg)",
            renderer.camera().describe(),
            crate::camera::MIN_DISTANCE,
            crate::camera::MAX_DISTANCE,
            crate::camera::MAX_PITCH_RADIANS.to_degrees()
        ),
    ];
    if renderer.is_paused() {
        lines.push("PAUSED - press X to resume".to_string());
    }
    let lines: Vec<String> = lines.into_iter().map(clip).collect();
    ui.panel(PANEL_MARGIN, PANEL_MARGIN, scale, &lines, 10.0 * scale);

    // Controls, laid out from the bottom up.
    let button_height = 8.0 * scale + BUTTON_PADDING * 2.0;
    let rows: [&[(Action, &str)]; 5] = [
        &[
            (Action::SetSamples(1), "1: 1 spp"),
            (Action::SetSamples(16), "2: 16 spp"),
            (Action::SetSamples(64), "3: 64 spp"),
        ],
        &[
            (Action::ReflectionsDelta(-1), "R: bounces -"),
            (Action::ReflectionsDelta(1), "T: bounces +"),
        ],
        &[
            (Action::ExposureDelta(-1), "E: exposure -"),
            (Action::ExposureDelta(1), "D: exposure +"),
        ],
        &[
            (Action::Restart, "Space: start/restart"),
            (Action::ResetCamera, "C: reset camera"),
            (Action::SavePng, "P: save PNG"),
        ],
        &[
            (Action::TogglePause, "X: pause/resume"),
            (Action::RunBenchmark, "B: benchmark"),
            (Action::Quit, "Esc: quit"),
        ],
    ];
    let row_gap = 6.0;
    let total_height = rows.len() as f32 * button_height + (rows.len() - 1) as f32 * row_gap;
    // The widest row must fit between the margins.
    let widest: f32 = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|(_, label)| {
                    label.chars().count() as f32 * 8.0 * scale + BUTTON_PADDING * 2.0
                })
                .sum::<f32>()
                + BUTTON_GAP * (row.len() as f32 - 1.0)
        })
        .fold(0.0f32, f32::max);
    debug_assert!(
        widest <= width - 2.0 * PANEL_MARGIN || scale <= 1.0,
        "button row of width {widest} does not fit in {width}"
    );
    let mut y = (height - PANEL_MARGIN - total_height).max(PANEL_MARGIN);
    let highlighted = |action: Action| match action {
        Action::SetSamples(n) => n == settings.samples_per_pixel,
        _ => false,
    };
    for row in rows {
        let mut x = PANEL_MARGIN;
        for (action, label) in row {
            let width_used = ui.button(x, y, scale, label, *action, highlighted(*action));
            x += width_used + BUTTON_GAP;
        }
        y += button_height + row_gap;
    }

    // Status line just above the controls.
    let status_line = format!(
        "{message}  |  {}/{} samples, {} bounces, exposure {:.2}",
        renderer.accumulated_samples(),
        settings.samples_per_pixel,
        settings.max_reflections,
        settings.exposure
    );
    let status_y = (height - PANEL_MARGIN - total_height - 8.0 * scale - 8.0).max(PANEL_MARGIN);
    ui.text(
        PANEL_MARGIN,
        status_y,
        scale,
        [1.0, 0.95, 0.6, 1.0],
        &clip(status_line),
    );
}

