//! Native Windows front-end.
//!
//! Responsibilities are deliberately narrow, because everything that affects
//! gameplay lives in `flappy_core`:
//!
//! * open a resizable `winit` window and a `softbuffer` surface for it;
//! * run a ~60 FPS loop driven by *real* elapsed time;
//! * translate window events into `flappy_core::Input`;
//! * blit the core's logical 360x640 frame, letterboxed, into the surface;
//! * play the sounds the core reports, and persist best score + mute.
//!
//! Nothing here decides how the game behaves.

use std::num::{NonZeroU16, NonZeroU32};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flappy_core::{
    Framebuffer, Game, Input, Key, Settings, Sound, SoundBank, SoundSet, Viewport, LETTERBOX,
    LOGICAL_H, LOGICAL_W, SAMPLE_RATE,
};

use rodio::buffer::SamplesBuffer;
use rodio::{ChannelCount, DeviceSinkBuilder, MixerDeviceSink, Player, SampleRate};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WinitKey, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

/// Target cadence. The simulation itself is frame-rate independent; this only
/// paces presentation so the game does not spin a core at 1000 FPS.
const FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// Initial window size: the logical area at 1.5x, which is comfortable on a
/// 1080p display and still shows the letterboxing behaviour.
const INITIAL_SIZE: LogicalSize<f64> = LogicalSize::new(540.0, 960.0);
const MIN_SIZE: LogicalSize<f64> = LogicalSize::new(240.0, 320.0);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::new()?;
    let mut app = App::new();
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// A time-based seed, so consecutive runs get different pipe layouts.
fn seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5EED_1234_ABCD)
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

/// Owns the OS audio stream and the synthesised buffers.
///
/// Every failure path here is non-fatal: a machine with no output device (or a
/// locked one) simply plays no sound, and the game is unaffected.
struct Audio {
    /// Must be kept alive for the stream to stay open.
    sink: Option<MixerDeviceSink>,
    bank: SoundBank,
    channels: ChannelCount,
    rate: SampleRate,
    unavailable: bool,
}

impl Audio {
    fn new() -> Self {
        let bank = SoundBank::generate(SAMPLE_RATE);
        let channels = NonZeroU16::new(1).expect("1 != 0");
        let rate = NonZeroU32::new(SAMPLE_RATE).expect("sample rate != 0");

        let sink = match DeviceSinkBuilder::open_default_sink() {
            Ok(sink) => Some(sink),
            Err(err) => {
                eprintln!("audio disabled: no usable output device ({err})");
                None
            }
        };
        Audio {
            unavailable: sink.is_none(),
            sink,
            bank,
            channels,
            rate,
        }
    }

    fn play(&self, sound: Sound) {
        if self.unavailable {
            return;
        }
        let Some(sink) = self.sink.as_ref() else {
            return;
        };
        // A fresh detached player per effect lets sounds overlap (a score blip
        // under the next flap) instead of queueing behind each other.
        let player = Player::connect_new(sink.mixer());
        player.append(SamplesBuffer::new(
            self.channels,
            self.rate,
            self.bank.get(sound),
        ));
        player.detach();
    }

    /// Plays everything the core reported for this frame.
    fn play_set(&self, set: SoundSet) {
        for sound in Sound::ALL {
            if set.contains(sound) {
                self.play(sound);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Settings file
// ---------------------------------------------------------------------------

/// `%APPDATA%\flappy_rust\settings.txt`, falling back to the temp directory.
/// Returns `None` only if neither location can be created.
fn settings_path() -> Option<PathBuf> {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let dir = PathBuf::from(appdata).join("flappy_rust");
        if std::fs::create_dir_all(&dir).is_ok() {
            return Some(dir.join("settings.txt"));
        }
    }
    let dir = std::env::temp_dir().join("flappy_rust");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("settings.txt"))
}

fn load_settings(path: Option<&PathBuf>) -> Settings {
    // A missing or corrupt file is expected and harmless: `parse` falls back to
    // defaults for anything it cannot understand.
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| Settings::parse(&text))
        .unwrap_or_default()
}

fn save_settings(path: Option<&PathBuf>, settings: Settings) {
    if let Some(path) = path {
        if let Err(err) = std::fs::write(path, settings.serialize()) {
            eprintln!("could not save settings to {}: {err}", path.display());
        }
    }
}

// ---------------------------------------------------------------------------
// Input mapping
// ---------------------------------------------------------------------------

/// Maps a physical/logical key onto a gameplay key. Layout-aware where it
/// matters: letters come from `logical_key`, so the same physical position
/// works on non-QWERTY layouts.
fn map_key(key: &WinitKey) -> Option<Key> {
    match key {
        WinitKey::Named(NamedKey::Space) | WinitKey::Named(NamedKey::ArrowUp) => Some(Key::Flap),
        WinitKey::Named(NamedKey::Escape) => Some(Key::Pause),
        WinitKey::Named(NamedKey::Enter) => Some(Key::Restart),
        WinitKey::Character(text) => match text.chars().next()?.to_ascii_uppercase() {
            'W' => Some(Key::Flap),
            'P' => Some(Key::Pause),
            'R' => Some(Key::Restart),
            'M' => Some(Key::Mute),
            _ => None,
        },
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

struct App {
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    /// Kept so the surface's context outlives it.
    _context: Option<softbuffer::Context<Arc<Window>>>,

    framebuffer: Framebuffer,
    game: Game,
    audio: Audio,
    settings_path: Option<PathBuf>,

    /// Latest cursor position in *physical* surface pixels.
    cursor: PhysicalPosition<f64>,
    surface_size: (u32, u32),

    last_tick: Instant,
    next_frame: Instant,
}

impl App {
    fn new() -> Self {
        let settings_path = settings_path();
        let mut game = Game::new(seed());
        game.apply_settings(load_settings(settings_path.as_ref()));

        let now = Instant::now();
        App {
            window: None,
            surface: None,
            _context: None,
            framebuffer: Framebuffer::new(LOGICAL_W, LOGICAL_H),
            game,
            audio: Audio::new(),
            settings_path,
            cursor: PhysicalPosition::new(0.0, 0.0),
            surface_size: (0, 0),
            last_tick: now,
            next_frame: now,
        }
    }

    /// Current mapping from physical surface pixels to logical play-area pixels.
    fn viewport(&self) -> Viewport {
        Viewport::new(self.surface_size.0, self.surface_size.1)
    }

    /// Feeds one core input event and plays whatever it triggered.
    fn feed(&mut self, input: Input) {
        let sounds = self.game.input(input);
        self.audio.play_set(sounds);
    }

    /// Feeds a pointer event at the current cursor position.
    fn feed_pointer(&mut self, pressed: bool) {
        let (x, y) = self
            .viewport()
            .to_logical(self.cursor.x as f32, self.cursor.y as f32);
        self.feed(if pressed {
            Input::PointerDown { x, y }
        } else {
            Input::PointerUp { x, y }
        });
    }

    fn resize_surface(&mut self, width: u32, height: u32) {
        // Windows reports 0x0 while minimising; `NonZeroU32` would panic.
        if width == 0 || height == 0 {
            return;
        }
        self.surface_size = (width, height);
        if let Some(surface) = self.surface.as_mut() {
            let w = NonZeroU32::new(width).expect("checked above");
            let h = NonZeroU32::new(height).expect("checked above");
            if let Err(err) = surface.resize(w, h) {
                eprintln!("could not resize the surface: {err}");
            }
        }
    }

    fn update_and_render(&mut self) {
        let now = Instant::now();
        // Real elapsed time, so the game runs at the same speed on a 60, 120 or
        // 144 Hz display. `advance` clamps and substeps internally.
        let dt = now.duration_since(self.last_tick).as_secs_f32();
        self.last_tick = now;

        let sounds = self.game.advance(dt);
        self.audio.play_set(sounds);
        if self.game.take_settings_dirty() {
            save_settings(self.settings_path.as_ref(), self.game.settings());
        }

        self.game.draw(&mut self.framebuffer);

        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        let (w, h) = self.surface_size;
        if w == 0 || h == 0 {
            return;
        }
        let viewport = Viewport::new(w, h);
        match surface.buffer_mut() {
            Ok(mut buffer) => {
                viewport.blit_rgb(&self.framebuffer, &mut buffer, w as usize, LETTERBOX);
                if let Err(err) = buffer.present() {
                    eprintln!("could not present the frame: {err}");
                }
            }
            Err(err) => eprintln!("could not acquire the back buffer: {err}"),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => self.resize_surface(size.width, size.height),

            WindowEvent::Focused(focused) => {
                // Losing focus pauses a running game; regaining it never
                // auto-resumes, and no accumulated time is applied.
                self.game.set_suspended(!focused);
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                let (x, y) = self.viewport().to_logical(position.x as f32, position.y as f32);
                self.feed(Input::PointerMove { x, y });
            }

            WindowEvent::MouseInput { state, button, .. } if button == MouseButton::Left => {
                self.feed_pointer(state == ElementState::Pressed);
            }

            WindowEvent::KeyboardInput { event, .. } => {
                // `repeat` is the OS auto-repeat; letting it through would turn
                // one held key into a flap every few milliseconds.
                if event.repeat {
                    return;
                }
                let Some(key) = map_key(&event.logical_key) else {
                    return;
                };
                self.feed(match event.state {
                    ElementState::Pressed => Input::KeyDown(key),
                    ElementState::Released => Input::KeyUp(key),
                });
            }

            WindowEvent::RedrawRequested => self.update_and_render(),

            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = WindowAttributes::default()
            .with_title("Flappy Rust")
            .with_inner_size(INITIAL_SIZE)
            .with_min_inner_size(MIN_SIZE);
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                eprintln!("could not create a window: {err}");
                event_loop.exit();
                return;
            }
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(context) => context,
            Err(err) => {
                eprintln!("could not create a drawing context: {err}");
                event_loop.exit();
                return;
            }
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(err) => {
                eprintln!("could not create a drawing surface: {err}");
                event_loop.exit();
                return;
            }
        };

        let size = window.inner_size();
        self.window = Some(window);
        self.surface = Some(surface);
        self._context = Some(context);
        self.resize_surface(size.width, size.height);

        let now = Instant::now();
        self.last_tick = now;
        self.next_frame = now;
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        self.window_event(event_loop, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if now >= self.next_frame {
            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
            // Pace from `now` so a slow frame does not queue a burst of
            // catch-up redraws.
            self.next_frame = now + FRAME_INTERVAL;
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
    }
}
