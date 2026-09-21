//! WebAssembly front-end.
//!
//! Mirrors the native backend's responsibilities exactly, against browser APIs:
//!
//! * own a canvas and an offscreen 360x640 buffer;
//! * run a `requestAnimationFrame` loop driven by *real* elapsed time;
//! * translate pointer/keyboard events into `flappy_core::Input`;
//! * blit the core's logical frame, letterboxed and crisp, into the canvas;
//! * play the sounds the core reports through Web Audio, and persist best score
//!   and mute in `localStorage`.
//!
//! Nothing here decides how the game behaves — that all lives in `flappy_core`.
//!
//! Two browser-specific constraints shape this file:
//!
//! * **Audio needs a user gesture.** The `AudioContext` is created on the first
//!   pointer/key event, not at load. Until then the game is silent but fully
//!   playable, and any audio failure is logged once and then ignored.
//! * **Listeners must outlive the call that registers them.** Every `Closure`
//!   is parked in [`KEEP`] for the lifetime of the page; dropping one would
//!   silently unsubscribe the handler.

use std::any::Any;
use std::cell::RefCell;

use flappy_core::{
    framebuffer_to_rgba, Framebuffer, Game, Input, Key, Settings, Sound, SoundBank, SoundSet,
    Viewport, LETTERBOX, LOGICAL_H, LOGICAL_W,
};

use wasm_bindgen::convert::FromWasmAbi;
use wasm_bindgen::prelude::*;
use wasm_bindgen::Clamped;
use web_sys::{
    AudioBuffer, AudioContext, CanvasRenderingContext2d, Event, EventTarget, GainNode,
    HtmlCanvasElement, ImageData, KeyboardEvent, MouseEvent, PointerEvent,
};

/// `localStorage` key holding the serialised [`Settings`].
const SETTINGS_KEY: &str = "flappy_rust.settings";

/// The canvas backing store is capped at 2x the CSS size and at this many
/// pixels. Nearest-neighbour pixel art gains nothing from a 3x backing store,
/// and the cap keeps memory sane on a 4K display or a high-DPI phone.
const MAX_BACKING_PIXELS: f64 = 4_000_000.0;
const MAX_DEVICE_PIXEL_RATIO: f64 = 2.0;

thread_local! {
    static APP: RefCell<Option<App>> = RefCell::new(None);
    static RAF: RefCell<Option<Closure<dyn FnMut(f64)>>> = RefCell::new(None);
    /// Keeps every registered event `Closure` alive for the page's lifetime.
    static KEEP: RefCell<Vec<Box<dyn Any>>> = RefCell::new(Vec::new());
}

/// Entry point called from `index.html` once the module has loaded.
#[wasm_bindgen]
pub fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let app = App::new()?;
    APP.with(|slot| *slot.borrow_mut() = Some(app));
    install_listeners()?;
    start_loop();
    Ok(())
}

/// Runs `f` against the live app, if one exists.
///
/// Uses `try_borrow_mut` so a re-entrant call (a listener firing while a frame
/// is being drawn) is dropped instead of panicking.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|slot| {
        slot.try_borrow_mut()
            .ok()
            .and_then(|mut guard| guard.as_mut().map(f))
    })
}

fn keep<T: ?Sized + 'static>(closure: Closure<T>) {
    KEEP.with(|k| k.borrow_mut().push(Box::new(closure)));
}

/// Registers `handler` on `target` and keeps it alive.
///
/// `FromWasmAbi` is what lets the closure receive a typed DOM event directly
/// instead of a bare `JsValue`.
fn on<T>(target: &EventTarget, event: &str, handler: impl FnMut(T) + 'static)
where
    T: JsCast + FromWasmAbi + 'static,
{
    let closure = Closure::new(handler);
    let _ = target.add_event_listener_with_callback(event, closure.as_ref().unchecked_ref());
    keep(closure);
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

/// Web Audio playback of the core's synthesised buffers.
///
/// `ctx` stays `None` until the first user gesture. Every step is fallible in a
/// browser, so all failures collapse into `failed`: the game then runs silently
/// rather than not at all.
struct Audio {
    bank: SoundBank,
    ctx: Option<AudioContext>,
    master: Option<GainNode>,
    buffers: [Option<AudioBuffer>; 4],
    failed: bool,
}

impl Audio {
    fn new() -> Self {
        Audio {
            bank: SoundBank::generate(flappy_core::SAMPLE_RATE),
            ctx: None,
            master: None,
            buffers: [None, None, None, None],
            failed: false,
        }
    }

    /// Creates/resumes the context. Called on the first user gesture, because
    /// browsers block audio until then.
    fn unlock(&mut self) {
        if self.failed {
            return;
        }
        if self.ctx.is_none() {
            if let Err(err) = self.build() {
                self.fail("could not initialise Web Audio", &err);
                return;
            }
        }
        if let Some(ctx) = self.ctx.as_ref() {
            // `resume` returns a promise; a rejection just means we stay silent.
            if ctx.state() == web_sys::AudioContextState::Suspended {
                let _ = ctx.resume();
            }
        }
    }

    fn build(&mut self) -> Result<(), JsValue> {
        let ctx = AudioContext::new()?;
        // The context picks its own rate (often 48 kHz), so the buffers are
        // resampled to match rather than assuming the synthesis rate.
        let rate = ctx.sample_rate();

        let master = ctx.create_gain()?;
        master.connect_with_audio_node(&ctx.destination())?;
        master.gain().set_value(1.0);

        let mut buffers: [Option<AudioBuffer>; 4] = [None, None, None, None];
        for (slot, sound) in buffers.iter_mut().zip(Sound::ALL) {
            let samples = self.bank.get(sound);
            let len = ((samples.len() as f64) * (rate as f64)
                / (self.bank.sample_rate as f64))
                .round()
                .max(1.0) as u32;
            let buffer = ctx.create_buffer(1, len, rate)?;
            // Resample by nearest sample: the effects are short and noisy, so
            // linear interpolation would buy nothing audible.
            let mut resampled = Vec::with_capacity(len as usize);
            for i in 0..len {
                let src = (i as f64 * self.bank.sample_rate as f64 / rate as f64) as usize;
                resampled.push(*samples.get(src).unwrap_or(&0.0));
            }
            buffer.copy_to_channel(&resampled, 0)?;
            *slot = Some(buffer);
        }

        self.master = Some(master);
        self.ctx = Some(ctx);
        Ok(())
    }

    fn fail(&mut self, what: &str, err: &JsValue) {
        if !self.failed {
            web_sys::console::warn_2(&JsValue::from_str(what), err);
            self.failed = true;
        }
        self.ctx = None;
        self.master = None;
    }

    /// Applies the mute flag to the master gain.
    fn set_muted(&self, muted: bool) {
        if let Some(master) = self.master.as_ref() {
            master.gain().set_value(if muted { 0.0 } else { 1.0 });
        }
    }

    fn play(&mut self, sound: Sound) {
        if self.failed {
            return;
        }
        let (Some(ctx), Some(master), Some(buffer)) = (
            self.ctx.as_ref(),
            self.master.as_ref(),
            self.buffers[sound as usize].as_ref(),
        ) else {
            return;
        };
        match ctx.create_buffer_source() {
            Ok(source) => {
                source.set_buffer(Some(buffer));
                if let Err(err) = source.connect_with_audio_node(master) {
                    self.fail("could not route audio", &err);
                    return;
                }
                // The node is garbage collected once it finishes.
                if let Err(err) = source.start() {
                    self.fail("could not start audio", &err);
                }
            }
            Err(err) => self.fail("could not create an audio source", &err),
        }
    }

    fn play_set(&mut self, set: SoundSet, muted: bool) {
        if set.is_empty() || muted {
            return;
        }
        for sound in Sound::ALL {
            if set.contains(sound) {
                self.play(sound);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

fn storage() -> Option<web_sys::Storage> {
    // Throws in some private-browsing modes; `ok().flatten()` covers both.
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
}

fn load_settings() -> Settings {
    storage()
        .and_then(|s| s.get_item(SETTINGS_KEY).ok().flatten())
        .map(|text| Settings::parse(&text))
        .unwrap_or_default()
}

fn save_settings(settings: Settings) {
    if let Some(store) = storage() {
        // Quota errors and disabled storage are both non-fatal.
        let _ = store.set_item(SETTINGS_KEY, &settings.serialize());
    }
}

// ---------------------------------------------------------------------------
// Input mapping
// ---------------------------------------------------------------------------

/// Maps a DOM `KeyboardEvent::key` onto a gameplay key.
fn map_key(key: &str) -> Option<Key> {
    match key {
        " " | "Spacebar" | "Up" | "ArrowUp" => Some(Key::Flap),
        "p" | "P" | "Escape" | "Esc" => Some(Key::Pause),
        "r" | "R" | "Enter" => Some(Key::Restart),
        "m" | "M" => Some(Key::Mute),
        "w" | "W" => Some(Key::Flap),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

struct App {
    game: Game,
    framebuffer: Framebuffer,
    /// Reused RGBA scratch buffer; one allocation for the whole session.
    rgba: Vec<u8>,
    screen: HtmlCanvasElement,
    screen_ctx: CanvasRenderingContext2d,
    blit: HtmlCanvasElement,
    blit_ctx: CanvasRenderingContext2d,
    audio: Audio,
    /// Timestamp of the previous frame, in milliseconds. `None` until the first
    /// frame, and reset whenever the page is hidden so no large `dt` is applied.
    last_ms: Option<f64>,
}

impl App {
    fn new() -> Result<App, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
        let document = window
            .document()
            .ok_or_else(|| JsValue::from_str("no document"))?;

        let screen: HtmlCanvasElement = document
            .get_element_by_id("game")
            .ok_or_else(|| JsValue::from_str("page is missing <canvas id=\"game\">"))?
            .dyn_into()?;
        let screen_ctx: CanvasRenderingContext2d = screen
            .get_context("2d")?
            .ok_or_else(|| JsValue::from_str("2D canvas is unavailable"))?
            .dyn_into()?;

        // Offscreen buffer at the fixed logical size. Created detached from the
        // DOM; every browser we target can draw a detached canvas.
        let blit: HtmlCanvasElement = document.create_element("canvas")?.dyn_into()?;
        blit.set_width(LOGICAL_W as u32);
        blit.set_height(LOGICAL_H as u32);
        let blit_ctx: CanvasRenderingContext2d = blit
            .get_context("2d")?
            .ok_or_else(|| JsValue::from_str("2D canvas is unavailable"))?
            .dyn_into()?;

        let mut game = Game::new(seed());
        game.apply_settings(load_settings());

        let app = App {
            game,
            framebuffer: Framebuffer::new(LOGICAL_W, LOGICAL_H),
            rgba: Vec::new(),
            screen,
            screen_ctx,
            blit,
            blit_ctx,
            audio: Audio::new(),
            last_ms: None,
        };
        app.audio.set_muted(app.game.muted());
        app.apply_canvas_settings();
        Ok(app)
    }

    /// Canvas state that is reset whenever the backing store is resized.
    fn apply_canvas_settings(&self) {
        // Hard pixel edges: the game is authored at 360x640 and scaled up.
        self.screen_ctx.set_image_smoothing_enabled(false);
        self.screen_ctx.set_fill_style_str(&letterbox_css());
    }

    /// Sizes the backing store in device pixels, capped for memory, and returns
    /// the resulting physical size.
    fn sync_size(&self) -> (u32, u32) {
        let css_w = self.screen.client_width().max(1) as f64;
        let css_h = self.screen.client_height().max(1) as f64;
        let mut dpr = web_sys::window()
            .map(|w| w.device_pixel_ratio())
            .unwrap_or(1.0)
            .clamp(1.0, MAX_DEVICE_PIXEL_RATIO);

        let mut phys_w = (css_w * dpr).round().max(1.0);
        let mut phys_h = (css_h * dpr).round().max(1.0);
        let pixels = phys_w * phys_h;
        if pixels > MAX_BACKING_PIXELS {
            let shrink = (MAX_BACKING_PIXELS / pixels).sqrt();
            dpr *= shrink;
            phys_w = (css_w * dpr).round().max(1.0);
            phys_h = (css_h * dpr).round().max(1.0);
        }

        let (w, h) = (phys_w as u32, phys_h as u32);
        if self.screen.width() != w || self.screen.height() != h {
            self.screen.set_width(w);
            self.screen.set_height(h);
            // Resizing a canvas resets its context, so reapply the settings.
            self.apply_canvas_settings();
        }
        (w, h)
    }

    /// One animation frame.
    fn tick(&mut self, timestamp_ms: f64) {
        let dt = match self.last_ms {
            // Real elapsed time, so the game runs at the same speed on a 60 Hz
            // phone and a 144 Hz monitor. `advance` clamps and substeps.
            Some(previous) => ((timestamp_ms - previous) / 1000.0) as f32,
            None => 0.0,
        };
        self.last_ms = Some(timestamp_ms);

        // Re-checks the canvas size every frame: this covers window resizes and
        // phone orientation changes without a separate resize listener.
        self.sync_size();

        let sounds = self.game.advance(dt);
        let muted = self.game.muted();
        self.audio.play_set(sounds, muted);

        if self.game.take_settings_dirty() {
            save_settings(self.game.settings());
        }

        self.game.draw(&mut self.framebuffer);
        self.present();
    }

    /// Blits the logical frame into the visible canvas, letterboxed.
    fn present(&mut self) {
        let (phys_w, phys_h) = (self.screen.width(), self.screen.height());
        if phys_w == 0 || phys_h == 0 {
            return;
        }

        framebuffer_to_rgba(&self.framebuffer, &mut self.rgba);
        let image = match ImageData::new_with_u8_clamped_array_and_sh(
            Clamped(&self.rgba),
            LOGICAL_W as u32,
            LOGICAL_H as u32,
        ) {
            Ok(image) => image,
            Err(_) => return,
        };
        if self.blit_ctx.put_image_data(&image, 0.0, 0.0).is_err() {
            return;
        }

        let viewport = Viewport::new(phys_w, phys_h);
        // Clear first: the letterbox bars are part of the frame.
        self.screen_ctx
            .fill_rect(0.0, 0.0, phys_w as f64, phys_h as f64);
        let _ = self.screen_ctx.draw_image_with_html_canvas_element_and_dw_and_dh(
            &self.blit,
            viewport.dst_x as f64,
            viewport.dst_y as f64,
            viewport.dst_w as f64,
            viewport.dst_h as f64,
        );
    }

    /// Converts a pointer event's viewport-relative CSS coordinates into logical
    /// play-area coordinates.
    fn pointer_logical(&self, event: &MouseEvent) -> (f32, f32) {
        let rect = self.screen.get_bounding_client_rect();
        let css_x = event.client_x() as f64 - rect.x();
        let css_y = event.client_y() as f64 - rect.y();
        // The ratio actually in use, which accounts for the memory cap above.
        let scale = if rect.width() > 0.0 {
            self.screen.width() as f64 / rect.width()
        } else {
            1.0
        };
        Viewport::new(self.screen.width(), self.screen.height())
            .to_logical((css_x * scale) as f32, (css_y * scale) as f32)
    }

    fn feed(&mut self, input: Input) {
        let sounds = self.game.input(input);
        let muted = self.game.muted();
        self.audio.play_set(sounds, muted);
        if self.game.take_settings_dirty() {
            save_settings(self.game.settings());
        }
    }

    /// Window blur / tab hidden. Pauses a running game, drops the frame clock so
    /// returning cannot apply a large step, and never auto-resumes.
    fn set_suspended(&mut self, suspended: bool) {
        self.game.set_suspended(suspended);
        self.last_ms = None;
    }
}

/// Time-based seed so consecutive sessions get different pipe layouts.
fn seed() -> u64 {
    (js_sys::Date::now() * 1000.0) as u64
}

/// The letterbox colour as a CSS string, derived from the shared constant so
/// the bars and the native build always match.
fn letterbox_css() -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (LETTERBOX >> 16) & 0xff,
        (LETTERBOX >> 8) & 0xff,
        LETTERBOX & 0xff
    )
}

// ---------------------------------------------------------------------------
// Loop and listeners
// ---------------------------------------------------------------------------

/// Starts the `requestAnimationFrame` loop.
///
/// The closure re-registers *itself* through [`RAF`], so it stays alive for the
/// page's lifetime without leaking a new closure every frame.
fn start_loop() {
    let closure = Closure::new(|timestamp: f64| {
        with_app(|app| app.tick(timestamp));
        schedule_frame();
    });
    RAF.with(|slot| *slot.borrow_mut() = Some(closure));
    schedule_frame();
}

fn schedule_frame() {
    let callback = RAF.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|c| c.as_ref().unchecked_ref::<js_sys::Function>().clone())
    });
    if let (Some(callback), Some(window)) = (callback, web_sys::window()) {
        let _ = window.request_animation_frame(&callback);
    }
}

fn install_listeners() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let canvas: HtmlCanvasElement = document
        .get_element_by_id("game")
        .ok_or_else(|| JsValue::from_str("page is missing <canvas id=\"game\">"))?
        .dyn_into()?;

    // --- pointer -----------------------------------------------------------
    // Pointer events only. Registering both touch and mouse handlers would make
    // a single tap fire twice on browsers that synthesise a compatibility mouse
    // event after a touch.
    on::<PointerEvent>(&canvas, "pointerdown", |event| {
        // Stops scroll/zoom from starting on the game surface.
        event.unchecked_ref::<Event>().prevent_default();
        with_app(|app| {
            app.audio.unlock();
            let mouse = event.unchecked_ref::<MouseEvent>();
            let (x, y) = app.pointer_logical(mouse);
            app.feed(Input::PointerDown { x, y });
        });
    });
    on::<PointerEvent>(&canvas, "pointermove", |event| {
        with_app(|app| {
            let mouse = event.unchecked_ref::<MouseEvent>();
            let (x, y) = app.pointer_logical(mouse);
            app.feed(Input::PointerMove { x, y });
        });
    });
    on::<PointerEvent>(&canvas, "pointerup", |event| {
        with_app(|app| {
            let mouse = event.unchecked_ref::<MouseEvent>();
            let (x, y) = app.pointer_logical(mouse);
            app.feed(Input::PointerUp { x, y });
        });
    });
    // A cancelled pointer (a system gesture taking over) still has to release
    // the press, or the next press would look like a repeat.
    on::<PointerEvent>(&canvas, "pointercancel", |event| {
        with_app(|app| {
            let mouse = event.unchecked_ref::<MouseEvent>();
            let (x, y) = app.pointer_logical(mouse);
            app.feed(Input::PointerUp { x, y });
        });
    });

    // --- keyboard ----------------------------------------------------------
    let key_target: EventTarget = window.clone().into();
    on::<KeyboardEvent>(&key_target, "keydown", |event| {
        // OS auto-repeat would turn one held key into a flap every few ms.
        if event.repeat() {
            return;
        }
        let Some(key) = map_key(&event.key()) else {
            return;
        };
        // Only swallow keys the game actually uses, so the rest of the page
        // (and the browser's own shortcuts) behave normally.
        event.unchecked_ref::<Event>().prevent_default();
        with_app(|app| {
            app.audio.unlock();
            app.feed(Input::KeyDown(key));
        });
    });
    on::<KeyboardEvent>(&key_target, "keyup", |event| {
        let Some(key) = map_key(&event.key()) else {
            return;
        };
        with_app(|app| app.feed(Input::KeyUp(key)));
    });

    // --- suspension --------------------------------------------------------
    let doc_target: EventTarget = document.clone().into();
    on::<Event>(&doc_target, "visibilitychange", |_| {
        with_app(|app| {
            let hidden = web_sys::window()
                .and_then(|w| w.document())
                .map(|d| d.visibility_state() == web_sys::VisibilityState::Hidden)
                .unwrap_or(false);
            app.set_suspended(hidden);
        });
    });
    let win_target: EventTarget = window.clone().into();
    on::<Event>(&win_target, "blur", |_| {
        with_app(|app| app.set_suspended(true));
    });
    // Regaining focus lifts the suspension flag but does not resume the run:
    // the player resumes explicitly, exactly as on the desktop build.
    on::<Event>(&win_target, "focus", |_| {
        with_app(|app| app.set_suspended(false));
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// Snapshot of the gameplay state, for diagnostics and automated checks.
///
/// The wasm build has no debugger and reading state back out of the canvas is
/// lossy, so this exposes the authoritative numbers instead: the phase, the
/// score, the bird's collision box, and every live pipe's x and gap bounds.
/// Everything here is read-only and has no effect on the simulation.
#[wasm_bindgen]
pub fn debug_state() -> JsValue {
    let state = with_app(|app| {
        let (x0, y0, x1, y1) = app.game.bird_hitbox();
        let state = js_sys::Object::new();
        let set = |key: &str, value: JsValue| {
            let _ = js_sys::Reflect::set(&state, &JsValue::from_str(key), &value);
        };
        set("phase", JsValue::from_str(&format!("{:?}", app.game.phase())));
        set("score", JsValue::from_f64(app.game.score() as f64));
        set("best", JsValue::from_f64(app.game.best() as f64));
        set("birdY", JsValue::from_f64(((y0 + y1) / 2.0) as f64));
        set("boxTop", JsValue::from_f64(y0 as f64));
        set("boxBottom", JsValue::from_f64(y1 as f64));
        set("boxLeft", JsValue::from_f64(x0 as f64));
        set("boxRight", JsValue::from_f64(x1 as f64));

        let pipes = js_sys::Array::new();
        for pipe in app.game.pipes() {
            let entry = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&entry, &"x".into(), &JsValue::from_f64(pipe.x as f64));
            let _ = js_sys::Reflect::set(
                &entry,
                &"gapTop".into(),
                &JsValue::from_f64(pipe.gap_top() as f64),
            );
            let _ = js_sys::Reflect::set(
                &entry,
                &"gapBottom".into(),
                &JsValue::from_f64(pipe.gap_bottom() as f64),
            );
            pipes.push(&entry);
        }
        set("pipes", pipes.into());
        state.into()
    });
    state.unwrap_or(JsValue::NULL)
}
