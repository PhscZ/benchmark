# Flappy Rust — one gameplay core, native desktop + WebAssembly

A complete Flappy Bird-style game written in Rust. A single dependency-free
gameplay crate is shared verbatim by a native Windows build and a
`wasm32-unknown-unknown` build that runs in desktop and mobile browsers.

There is no backend, no account and no network traffic after the page loads.
Every visual and every sound is generated procedurally at runtime, so the
project ships **zero third-party assets** and has no licensing questions.

---

## Quick start

```bash
# Native (Windows)
cargo run --release

# Web
./scripts/build-web.ps1      # or scripts/build-web.sh
python web/serve.py          # http://127.0.0.1:8080/
```

Full commands, prerequisites and troubleshooting are in
[Building and running](#building-and-running).

---

## Controls

| Action | Desktop | Mobile browser |
|---|---|---|
| Flap / start / restart | `Space`, `↑`, `W`, left mouse click | Tap |
| Pause / resume | `P`, `Esc`, or the on-screen ⏸ button | On-screen ⏸ button, or tap while paused |
| Restart from game over | `Space`, `R`, `Enter`, or the **RESTART** button | **RESTART** button, or tap |
| Mute / unmute | `M`, or the on-screen 🔊 button | On-screen 🔊 button |

Notes:

- One physical press produces exactly one flap. OS key repeat is ignored, and
  the web build listens to **pointer events only** (never a mix of `mousedown`
  and `touchstart`), so a tap cannot fire twice.
- Pressing a button never also flaps: buttons are hit-tested before gameplay
  input, and a tap that resumes or restarts is consumed by that transition.
- Losing window focus or hiding the browser tab pauses the run. Returning does
  **not** auto-resume — the player must ask for it — and no accumulated time is
  ever applied as one large physics step.

---

## Framework and design choices

### No game engine

The game needs a rectangle blitter, three sounds and a state machine. Pulling in
`bevy`, `macroquad` or `ggez` would add tens of megabytes of dependency tree,
a much longer build, and — critically for this brief — **two different
platform layers to keep in sync**. Instead the project uses three small,
well-established, narrowly-scoped crates:

| Crate | Role | Why |
|---|---|---|
| `winit` | window + input | The de-facto Rust windowing crate; the same event model maps cleanly onto DOM events. |
| `softbuffer` | CPU presentation | Blits a `&[u32]` to the window. No GPU pipeline, no shader compilation, no driver-specific failure modes — and its `0x00RRGGBB` pixel layout is exactly what the core's framebuffer already uses, so the desktop present is a straight copy. |
| `rodio` | audio playback | Cross-platform `cpal` backend; plays the procedurally generated `f32` sample buffers directly. |

The web build uses only `wasm-bindgen` + `web-sys` (canvas 2D, Web Audio,
`localStorage`).

### One core, two thin backends

```
crates/flappy_core    <- all gameplay, rendering and audio synthesis. Pure std, zero deps.
crates/flappy_native  <- winit + softbuffer + rodio. ~600 lines.
crates/flappy_web     <- wasm-bindgen + web-sys. ~600 lines.
```

`flappy_core` owns the state machine, fixed-timestep physics, pipe generation
and recycling, collision, input arbitration, the software rasterizer, the
procedural sound synthesis and the persisted-settings format. A backend's entire
job is: open a surface, forward events, blit pixels, play the sounds the core
reports, and store a settings string.

This is what makes the "shared gameplay" requirement real rather than nominal:
there is exactly one implementation of gravity, one set of collision bounds and
one scoring rule, and it is compiled for both targets.

### Fixed logical resolution with letterboxing

Gameplay runs in a fixed **360×640** logical area regardless of window size,
canvas size or device pixel ratio. The core always renders that exact frame; the
backend scales it into the surface and letterboxes the remainder
(`flappy_core::Viewport`, shared by both backends).

Consequences:

- Resizing a desktop window or rotating a phone **cannot change difficulty** —
  the playfield is always 360×640 logical pixels with the same pipe gaps.
- Rendering cost is constant (230 400 pixels) no matter how large the window is.
- Pointer coordinates are converted back through the same viewport, so input
  stays exact under any scale factor.
- On high-DPI phones the canvas backing store is sized in *device* pixels, so
  the play area stays crisp instead of being upscaled from CSS pixels.

Scaling is continuous (not integer-only) to use the screen well, with
nearest-neighbour sampling that suits the flat, pixel-art-style rendering.

### Frame-rate independence

`Game::advance(dt)` takes real elapsed seconds and consumes them in fixed
**1/120 s** substeps. The simulation is therefore identical at 30, 60, 144 or
stuttering frame rates, and a long stall cannot teleport the bird through a
pipe:

- a single frame's `dt` is clamped to 0.25 s;
- at most 8 substeps (≈66 ms) are simulated per frame, and any remaining backlog
  is **discarded** rather than fast-forwarded;
- no backend ever passes a hardcoded `1/60` — both measure real time.

`game::tests::physics_are_frame_rate_independent` runs the same flap schedule at
four different frame rates and asserts identical scores, identical outcomes and
sub-pixel agreement on the bird's position.

### Bounded memory

At most 6 pipes exist at any moment (`MAX_PIPES`). Pipes are spawned just off
the right edge and dropped once fully past the left edge, so a run of any length
uses constant memory — there is no unbounded `Vec` growth and no per-frame
allocation in the simulation or the renderer. The framebuffer is allocated once.

### Collision bounds match the visible objects

Pipes are drawn as body + cap rectangles, and collision tests **exactly those
rectangles** — including the cap's 6 px overhang on each side (a broad-phase bug
that ignored the left overhang was caught by a test). The bird's hitbox is a
rotation-independent box 2 px inside the drawn ellipse on each axis, so hits are
fair and repeatable rather than depending on the current wing frame or rotation.

### Audio without asset files

Every effect is synthesised from oscillators and noise at startup
(`flappy_core::audio`) and validated by tests to start and end at silence (so
they never click) and stay inside `[-1, 1]`:

- **Flap** — a triangle wave sweeping 520→1420 Hz with a short noise transient.
- **Score** — a two-note square-wave blip (880 → 1318.5 Hz).
- **Hit** — a falling low-passed noise burst plus a descending thud.
- **Click** — a short square blip for UI actions.

The browser blocks audio until a user gesture, so the web build creates/resumes
its `AudioContext` on the first pointer or key event. If the context, an
`AudioBuffer`, or a native audio device is unavailable, the failure is logged
once and the game stays fully playable.

---

## Building and running

### Prerequisites

- **Rust 1.75+** (developed against 1.97). Install from <https://rustup.rs>.
- **Windows:** the GNU or MSVC toolchain both work; the GNU toolchain needs
  `gcc` on `PATH` (e.g. WinLibs/MinGW-w64).
- **Web only:** the `wasm32-unknown-unknown` target and a `wasm-bindgen` CLI
  whose version **matches the crate exactly** (pinned at `0.2.126` here):

  ```bash
  rustup target add wasm32-unknown-unknown
  cargo install wasm-bindgen-cli --version 0.2.126
  ```

  If `wasm-bindgen --version` reports anything other than `0.2.126`, either
  install the matching CLI or bump the `wasm-bindgen`/`js-sys`/`web-sys` pins in
  `Cargo.toml` together. A mismatch makes `wasm-bindgen` refuse to process the
  module.

### Native desktop (Windows)

```bash
cargo run --release                     # from the repository root
```

The binary is written to `target/release/flappy_bird.exe`. Run it directly, or:

```bash
cargo build --release
./target/release/flappy_bird.exe
```

Tests (no window is opened):

```bash
cargo test --workspace
```

### WebAssembly

```powershell
# PowerShell
./scripts/build-web.ps1
python web/serve.py
```

```bash
# bash
./scripts/build-web.sh
python3 web/serve.py
```

Then open <http://127.0.0.1:8080/>.

`python web/serve.py` is a small dependency-free static server. It is used
instead of `python -m http.server` for two reasons:

1. `http.server` does not always send `Content-Type: application/wasm`, and
   browsers refuse to stream-compile a module served with the wrong MIME type.
2. It binds `127.0.0.1:8080` explicitly, so the page is reachable from a phone
   on the same network only if you ask for it (see *Testing on a phone*).

Any static server works as long as it sets `application/wasm` for `.wasm`:

```bash
npx serve web -l 8080
```

**The page must be served over HTTP, not opened as a `file://` URL.** Browsers
block `fetch` of the `.wasm` module from `file://`, and `localStorage` is
restricted there too.

#### Testing on a phone

Mobile browsers require a secure context for some APIs, and `127.0.0.1` is not
reachable from another device. Serve on your LAN instead and use the machine's
IP address:

```bash
python web/serve.py --host 0.0.0.0 --port 8080
# then browse to http://<your-lan-ip>:8080/ on the phone
```

Everything the game needs works over plain HTTP on a LAN address; no HTTPS
certificate is required for canvas, pointer events, Web Audio or
`localStorage`. If your phone blocks it, put the page behind an HTTPS tunnel or
a local TLS proxy — that is a browser policy, not a limitation of this build.

---

## Persistence

| Platform | Location |
|---|---|
| Native Windows | `%APPDATA%\flappy_rust\settings.txt` (falls back to the system temp directory) |
| Browser | `localStorage`, key `flappy_rust.settings` |

The format is shared and deliberately forgiving:

```
best=12
muted=false
```

`Settings::parse` ignores unknown keys, tolerates CRLF and surrounding
whitespace, rejects negative/absurd scores (> 1 000 000) and unparseable values,
and falls back to safe defaults. Storage failures are never fatal:

- native — every file operation is best-effort; a missing directory, a
  read-only disk or a corrupt file just means defaults;
- web — `localStorage` access is wrapped in `try`/`catch` because it throws in
  some private-browsing modes; the game falls back to in-memory settings.

Only the **best score** and the **mute flag** are persisted. Restarting resets
all gameplay state in place — it never restarts the process or reloads the page.

---

## Project layout

```
Cargo.toml                     workspace + pinned dependency versions
crates/flappy_core/            shared gameplay (no platform dependencies)
  src/game.rs                  state machine, physics, pipes, collision, input
  src/draw.rs                  rendering of every screen
  src/gfx.rs                   framebuffer, shapes, text drawing
  src/font.rs                  original 5x7 bitmap font
  src/audio.rs                 procedural sound synthesis
  src/settings.rs              persisted-settings format
  src/viewport.rs              letterbox scaling shared by both backends
  src/rng.rs                   deterministic PCG random source
  examples/preview.rs          dev tool: dumps each screen to a .bmp for review
crates/flappy_native/          Windows front-end
crates/flappy_web/             wasm front-end
web/                           page, styles and the static server
scripts/                       web build scripts
```

### Visual regression without a GPU

`cargo run -p flappy_core --example preview -- <dir>` renders the ready,
playing, paused and game-over screens to `<dir>/*.bmp`. Useful for reviewing the
renderer on a machine where you would rather not open a window.

---

## Known platform limitations

- **Native build is Windows-only.** The core is platform-free and the backends
  are close to portable, but only the Windows desktop target is configured and
  tested. The wasm build covers all non-Windows desktop use.
- **No native mobile app.** Mobile is served by the browser build only, as
  specified.
- **`wasm-bindgen` version lock.** The CLI and the crate must match exactly; see
  the prerequisites above. This is the single most common build failure.
- **Audio needs a user gesture in browsers.** By policy, the first flap also
  starts the audio context. If you mute before the first interaction, no sound
  is heard and nothing is created — the game is unaffected.
- **Nearest-neighbour scaling is intentional.** On very large windows the
  play area is upscaled with hard pixel edges rather than smoothed. This keeps
  the art style coherent and the blit cheap; it is a deliberate look, not a
  filtering bug.
- **`localStorage` can be unavailable** in some private-browsing configurations.
  The game then runs with default settings for the session.
- **The letterbox area is inert.** Clicks in the bars are treated as gameplay
  taps (which is what a player expects when clicking near the playfield), but
  they can never hit a UI button.
- **No gamepad support.** Not requested; the input layer is small enough that
  adding it would be a single mapping function.
