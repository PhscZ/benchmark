//! # flappy_core
//!
//! Platform-independent heart of the game, shared verbatim by the native
//! Windows build and the WebAssembly build.
//!
//! It owns:
//! * the state machine (`Ready` → `Playing` → `Paused` / `GameOver`);
//! * fixed-timestep physics, pipe spawning/recycling, scoring and collision;
//! * input arbitration (button hit-testing, key-repeat suppression, taps
//!   consumed by state transitions);
//! * a software rasterizer that produces one 360x640 RGB frame per tick;
//! * procedural sound synthesis and the persisted-settings format.
//!
//! Backends therefore only have to: open a surface, scale the logical frame
//! into it (letterboxed), forward input, play the sounds the core reports, and
//! store/load the settings string.
//!
//! ```no_run
//! use flappy_core::{Framebuffer, Game, Input, Key, LOGICAL_H, LOGICAL_W};
//!
//! let mut game = Game::new(0x1234_5678);
//! let mut fb = Framebuffer::new(LOGICAL_W, LOGICAL_H);
//! let sounds = game.input(Input::KeyDown(Key::Flap));
//! let sounds = game.advance(1.0 / 60.0);
//! if !sounds.is_empty() {
//!     // hand `sounds` to the platform audio layer
//! }
//! game.draw(&mut fb);
//! ```

pub mod audio;
pub mod draw;
pub mod font;
pub mod game;
pub mod gfx;
pub mod rng;
pub mod settings;
pub mod viewport;

pub use audio::{Sound, SoundBank, SoundSet, SAMPLE_RATE};
pub use game::{
    Button, Game, Input, Key, Phase, Pipe, Rect, BASE_SPEED, BIRD_HIT_HALF_H, BIRD_HIT_HALF_W,
    BIRD_X, BUTTON_SIZE, FIXED_DT, GAP_EDGE_MARGIN, GROUND_H, LOGICAL_H, LOGICAL_W, MAX_FRAME_DT,
    MAX_PIPES, MAX_SUBSTEPS, PIPE_GAP, PIPE_W, PLAY_H,
};
pub use gfx::Framebuffer;
pub use rng::Rng;
pub use settings::Settings;
pub use viewport::{Viewport, LETTERBOX};

/// Number of bytes in one RGBA8 frame at the logical resolution.
pub const RGBA_FRAME_BYTES: usize = (LOGICAL_W * LOGICAL_H * 4) as usize;

/// Converts the internal `0x00RRGGBB` framebuffer into RGBA8 bytes, as needed
/// by `ImageData`/`putImageData` on the web.
pub fn framebuffer_to_rgba(fb: &Framebuffer, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(RGBA_FRAME_BYTES);
    for px in &fb.px {
        out.push((px >> 16) as u8);
        out.push((px >> 8) as u8);
        out.push(*px as u8);
        out.push(255);
    }
}
