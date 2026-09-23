# AI Benchmark

An end-to-end coding benchmark comparing state-of-the-art AI models on software development tasks relevant to my use case.

Models are evaluated on whether their software runs and correctly completes the required tasks.

## Contents

- [Scoring](#scoring)
  - [Execution attempts](#execution-attempts)
  - [Functional tests](#functional-tests)
  - [Example](#example)
- [Evaluation consistency](#evaluation-consistency)
- [Challenges](#challenges)
  - [1. C Compiler in Rust](#1-c-compiler-in-rust)
  - [2. Glicko-2 REST API in Rust](#2-glicko-2-rest-api-in-rust)
  - [3. Cross-Platform Flappy Bird–Style Game in Rust](#3-cross-platform-flappy-birdstyle-game-in-rust)
  - [4. Desktop Text Editor in Rust](#4-desktop-text-editor-in-rust)
  - [5. GPU Ray Tracer using Vulkan in Rust](#5-gpu-ray-tracer-using-vulkan-in-rust)
  - [6. GPU Inference Engine in Rust](#6-gpu-inference-engine-in-rust)
  - [7. Near-Duplicate Image Finder in Python](#7-near-duplicate-image-finder-in-python)
  - [8. 3D Racing Game in JavaScript](#8-3d-racing-game-in-javascript)
  - [9. Browser Image Editor in JavaScript](#9-browser-image-editor-in-javascript)
  - [10. Memory Allocator in C](#10-memory-allocator-in-c)
  - [11. ZIP Archive Tool in Zig](#11-zip-archive-tool-in-zig)
  - [12. Regex Engine in C++](#12-regex-engine-in-c)
  - [13. Concurrent KV Store in Go](#13-concurrent-kv-store-in-go)

## Scoring

Each submission receives two separate scores:

- **Subjective (0–100):** My assessment of the implementation’s quality and how well it meets my needs.
- **Objective (0–100):** Calculated from the attempts required to run the software and its performance on five test tasks.

### Execution attempts

Each submission gets a maximum of **four attempts** to run.

| First successful run | Starting objective score |
|---|---:|
| Attempt 1 | 100 |
| Attempt 2 | 75 |
| Attempt 3 | 50 |
| Attempt 4 | 25 |
| Never runs | 0 |

Between attempts, I provide only the errors produced by the console.

### Functional tests

Once the software runs, it is evaluated against **five predefined test tasks**.

| Task result | Penalty |
|---|---|
| Passes fully | No deduction |
| Works partially, has missing functionality, or contains a functional bug | Deduct 10% of the remaining score |
| Fails | Deduct 20% of the remaining score |

**Penalties are multiplicative, not percentage-point deductions.** Passing tasks do not restore lost points.

```text
Objective score = starting score × 0.8^failed_tasks × 0.9^partial_tasks
```

### Example

A submission runs on its second attempt, fails one task, partially passes two, and fully passes two:

| Step | Calculation | Remaining score |
|---|---|---:|
| Initial score | — | 100 |
| Runs on attempt 2 | 100 × 0.75 | 75 |
| Task 1: fail | 75 × 0.80 | 60 |
| Task 2: partial | 60 × 0.90 | 54 |
| Task 3: partial | 54 × 0.90 | 48.6 |
| Task 4: pass | No deduction | 48.6 |
| Task 5: pass | No deduction | **48.6** |

## Evaluation consistency

- Each model receives the same task specification and test requirements.
- Submissions are evaluated in the same environment for each challenge (Windows, Zed, omp).
- Pass, partial-pass, and failure criteria are defined before evaluation.
- Subjective and objective scores are reported separately, alongside the attempt count and test results.

Results reflect performance on **my selected use cases**, not a universal ranking of coding ability.

## Challenges

### 1. C Compiler in Rust

```text
write a compiler in Rust that reads a subset of C from stdin and outputs
x86-64 assembly to stdout. target Windows x64 using the Windows x64 ABI
and GNU assembler Intel syntax, with MinGW-w64 GCC for assembling and
linking against its C runtime. provide complete source and build/run commands.

support:
- 32-bit signed int, 8-bit char, unsigned char, and void
- local/global variables, initialization, and lexical block scope
- pointers, pointer arithmetic, address-of, dereferencing, and indexing
- fixed-size arrays, array-to-pointer conversion, and pointer-to-pointer types
- functions, prototypes, recursion, parameters, and return
- int main(void) and int main(int argc, char **argv)
- if/else, while, for, do/while, break, and continue
- arithmetic, comparison, logical, and bitwise operators
- assignment, compound assignment, and prefix/postfix increment/decrement
- correct C operator precedence and short-circuit && and ||
- integer promotions and casts between supported integer types
- decimal/hexadecimal integer literals, character literals, string literals,
  and escape sequences including \n, \r, \t, \0, \\, \", and \'
- // and /* */ comments

no preprocessor or headers are required. recognize getchar, putchar,
and printf as known externals with their correct signatures, including
variadic calls to printf. getchar must return int so EOF (-1) remains
distinct from every input byte. string literals must be null-terminated.

the compiler reads C source from stdin; generated programs read their
own input from stdin in a separate execution. generated programs must
support binary stdin/stdout without Windows newline translation or
Ctrl-Z EOF handling; runtime initialization calls may be used for this.

no file arguments, utility flags, Unicode processing, or Base64 decoding
are required. handle empty input and non-ASCII bytes correctly.

the compiler must compile C implementations of simple utilities.
```

#### Test tasks

| Task | Required behavior |
|---|---|
| `cat` | Copy stdin to stdout. |
| `wc` | Count lines, words, and bytes from stdin. |
| `rev` | Reverse each line of input. |
| `base64` | Encode stdin as Base64. |
| `strings` | Extract printable character sequences from stdin. |

### 2. Glicko-2 REST API in Rust

```text
write a REST API in Rust that calculates Glicko-2 ratings for a game
from chronologically ordered events, such as player A kills player B.

use these defaults:
- rating: 1500
- rating deviation (RD): 350
- volatility (sigma): 0.06
- scale: 173.7178
- convergence tolerance (epsilon): 1e-6
- tau: 0.5

use player logins as identifiers throughout the system.
exclude any event where the killer or victim is ".nobody" or ".self" from
all rating calculations and statistics, this should be configurable.

a match is a race to 20 kills between two specific players, equivalent
to a best of 39. a player wins when they kill that opponent 20 times.

after a win, reset only that pair's match kill counters in the relevant
rating category. do not reset their counters against other opponents.
keep cumulative kills and deaths separately for statistics; these must
not reset when a match ends.

maintain three separate ratings and independent pairwise match counters:
- total: all eligible events
- 1v1: events with exactly two players currently playing
- pub: events with more than two players currently playing

the lobby is dynamic. classify each event using its recorded player count,
not the player count when a race started or ended. each eligible event
contributes to total and its applicable category (1v1 or pub).

if a third player joins a 1v1 lobby, subsequent events contribute to pub
instead of 1v1. existing category-specific counters remain stored and
resume when qualifying events occur; do not transfer or reset them
because the lobby size changes.

implement custom margin-of-victory scoring instead of standard
win/draw/loss values:

winScore = 0.5 + 0.5 * (winnerScore - loserScore) / winnerScore;
lossScore = 1.0 - winScore;

winnerScore and loserScore are the players' kill counts for that match.
a 20-19 win produces a result close to 0.5, while a dominant win
produces a result closer to 1.0. wins and losses must still be recorded
as actual match outcomes, not fractional values.

apply inactivity RD growth reactively before processing an eligible event,
for both the killer and victim, using event timestamps rather than the
current clock or calendar-day boundaries.

for each player:
- periods = floor((eventTime - lastActive) / 24 hours)
- phi = RD / 173.7178
- phiNew = sqrt(phi * phi + periods * sigma * sigma)
- RD = phiNew * 173.7178

use the player's current volatility (sigma), not the default volatility.
initialize lastActive on the player's first eligible event without
applying prior inactivity.

after each eligible event, set lastActive to its timestamp, even when
periods is zero. discard fractional days rather than carrying them over.
a gap of 23 hours and 59 minutes adds no inactivity RD; a gap of 49 hours
adds two periods. lastActive is the last registered event.

inactivity changes RD only, not rating or volatility. do not apply it
in background jobs or when serving API reads. apply it before any rating
update caused by the current event.

provide endpoints for:
- importing events from my existing system
- retrieving full and paginated leaderboards
- retrieving an individual player's information
- retrieving a player's matchups against other players, with pagination
- retrieving head-to-head statistics against one specified player
- retrieving the latest registered events, with pagination and filtering
  by player login, matching events where they were the killer or victim

leaderboards and individual player information must include:
- rating
- RD
- wins
- losses
- winrate
- kills
- deaths
- kill/death ratio

matchup and head-to-head responses must include kills, deaths,
and kill/death ratio against the relevant opponent.

support selecting total, 1v1, or pub statistics where applicable.

the API must be fully functional, fast, and data efficient when processing
the existing event history and serving queries. provide complete source,
build/run instructions, and API usage examples.

make sure it is easy to import events from an external .json file, with
formatting similar to:
[{"id": "e5", "time": "2026-05-16T18:36:04Z", "killer": "nicolas404", "victim": "orinslc", "player_count": 3},
{"id": "e10", "time": "2026-05-16T18:36:25Z", "killer": "nicolas404", "victim": "orinslc", "player_count": 3},
{"id": "e11", "time": "2026-05-16T18:36:25Z", "killer": "orinslc", "victim": "nicolas404", "player_count": 2}]

also add swagger UI support, so it is easy to check and test the
endpoints created by this project.

there should also be a way to feed alt-accounts for players, and separated
endpoints considering alt-accounts or not, add a prefix /noalt/ to the
endpoints that do not consider this system, it should also account for
players who are added to the system after getting kills and deaths with
such account, and it should recalculate accordingly.
this is an example of the alt JSON:
[{"alt": "fck", "id": 57, "main": "deadlyenergy"}, {"alt": "orinslc", "id": 56, "main": "orinslc56"}]
```

#### Test tasks

| Task | Required behavior |
|---|---|
| Event import | Parse and process all events stored in the current system. Excluding the excluded users and applying the alt system. |
| Leaderboard | Return a leaderboard page with the required ratings and statistics. |
| Individual player | Return the specified player's ratings and statistics. |
| Player matchups | Return paginated statistics against other players. |
| Head-to-head | Return the specified player's statistics against one specific opponent. |

### 3. Cross-Platform Flappy Bird–Style Game in Rust

```text
write a complete 2D Flappy Bird-style game in Rust, sharing the gameplay
code between a native Windows desktop build and a WebAssembly build
that runs in desktop and mobile browsers. no native mobile app is required.

provide complete source, pinned dependencies, and commands for building
and running both versions, including serving the web build locally.

gameplay:
- the player controls a bird that moves vertically under gravity
- each flap gives the bird an upward impulse
- pipes move from right to left with a gap for the bird to pass through
- generate randomized but reasonably playable pipe gaps
- award one point for each pipe pair successfully passed
- hitting a pipe, the ground, or the top boundary ends the run
- use consistent collision bounds that match the visible objects
- movement, spawning, and physics must be frame-rate independent
- remove or reuse off-screen pipes so memory usage remains bounded

game states:
- a ready screen with the title and control instructions
- active gameplay with the current score visible
- pause/resume
- a game-over screen showing the score, best score, and restart control
- restarting must reset all gameplay state without restarting the app
  or reloading the page

controls:
- desktop: space, up arrow, or left mouse click to flap
- mobile browser: tap to flap
- provide an on-screen pause/resume button usable with mouse or touch
- support P or Escape for pause/resume on desktop
- one physical input must produce only one flap; avoid keyboard-repeat
  flaps and duplicate touch/mouse events
- interacting with menu buttons must not also trigger a gameplay flap

display and mobile support:
- use a fixed logical gameplay area with responsive scaling and
  letterboxing so resizing does not change gameplay difficulty
- support desktop window resizing and mobile portrait/landscape layouts
- keep controls and score readable and within the visible screen
- prevent touch scrolling and zoom gestures on the game surface
  without disabling normal behavior elsewhere on the page
- pause when the native window loses focus or the browser tab is hidden
- do not advance gameplay while suspended or apply a large physics step
  when returning; require explicit resume

presentation:
- provide a cohesive visual style with a bird, pipes, background,
  ground, readable text, and visible buttons
- use original, procedurally generated, or permissively licensed assets;
  do not require copyrighted Flappy Bird assets or external asset downloads
- include flap, score, and collision sounds, plus a mute control
- initialize browser audio after a user interaction and handle unavailable
  audio gracefully without preventing gameplay

persistence:
- save the best score and mute setting between sessions
- use local storage in browsers and an appropriate local file natively
- if storage is unavailable or corrupted, use safe defaults and keep
  the game playable

the web build must work in current Chrome, Firefox, and Edge on desktop,
Chrome on Android, and Safari on iOS. no backend, account, or network
connection is required after the game assets have loaded.

aim for smooth 60 FPS gameplay on the specified evaluation devices.
document the build process, controls, framework choice, and any known
platform limitations. provide the web page and all assets needed to
serve the WebAssembly build.
```

#### Test tasks

| Task | Required behavior |
|---|---|
| Native desktop | Build and launch on Windows; complete a run using keyboard and mouse controls. |
| Desktop browser | Build and serve the WebAssembly version; complete a run with working graphics, audio after interaction, and controls. Google Chrome/Chromium. |
| Desktop browser | Build and serve the WebAssembly version; complete a run with working graphics, audio after interaction, and controls. Firefox. |
| Mobile browser | Play using touch on Android and iOS; resizing and orientation changes preserve usability without page scrolling or duplicate flaps. |
| Gameplay and lifecycle | Verify scoring, collisions, pause/resume, focus handling, and repeated restarts without stale state or accumulating off-screen objects. |

### 4. Desktop Text Editor in Rust

```text
write a desktop text editor in Rust for Windows with a custom text buffer
and custom editing behavior. use EGUI for the desktop interface, do not
use an existing text-editing widget or editor engine for the document area.

implement the document buffer yourself, such as a piece table, gap buffer,
or rope. do not wrap a ready-made editor buffer library. avoid copying
the entire document on every edit.

provide complete source, pinned dependencies, and build/run instructions.

file operations:
- create new documents and open existing files through a file dialog
- support save and save as
- support UTF-8 text
- clearly reject invalid UTF-8 rather than silently corrupting it
- show the filename and unsaved-change indicator in each tab
- prompt to save, discard, or cancel when closing an unsaved document
  or exiting with unsaved documents
- report file errors without crashing or losing the open document
- save safely through a temporary file and replacement so a failed write
  does not truncate the original file

editing and navigation:
- insert text, newlines, and tabs
- support Backspace, Delete, and clipboard copy/cut/paste
- support arrow keys, Home/End, Ctrl+Home/End, Page Up/Page Down,
  and Ctrl+Left/Right for word navigation
- clicking places the cursor at the corresponding text position
- support selection with mouse dragging, Shift+click, Shift+navigation,
  Ctrl+Shift+Left/Right, and Ctrl+A
- double-click selects a word
- typing or pasting replaces the current selection
- dragging beyond the viewport scrolls while extending selection
- keep the cursor visible while navigating and editing
- full bidirectional text layout is not required

undo and redo:
- support Ctrl+Z and Ctrl+Y, with separate history for each document
- restore text, cursor, and selection appropriately
- group consecutive typing into sensible undo steps
- treat paste and replace-all as single undoable actions
- editing after undo must discard the redo branch
- saving must not clear undo history
- undoing back to the saved state must clear the unsaved-change indicator

find and replace:
- provide a search bar with literal text search and a case-sensitive toggle
- support next/previous match, wraparound, and a match count
- highlight visible matches and distinguish the current match
- support replacing the current match and replacing all matches
- define safe behavior for an empty search query
- regex search is not required

interface:
- show line numbers in a gutter aligned with the text
- show cursor line/column, encoding, and line-ending style in a status bar
- support vertical and horizontal scrolling
- use a monospace font with consistent cursor and selection positioning
- soft wrapping is not required; long lines must scroll horizontally
- support multiple tabs, each retaining its own cursor, selection,
  scroll position, undo/redo history, and unsaved state
- provide visible menus or buttons for the main actions
- support Ctrl+N, Ctrl+O, Ctrl+S, Ctrl+Shift+S, Ctrl+W, Ctrl+F, Ctrl+H,
  and Ctrl+Tab for the corresponding actions

large-file behavior:
- handle the supplied UTF-8 fixtures up to 90 MiB, including a file
  with at least one million short lines
- render only visible content rather than laying out the whole document
  on every frame; long lines must not force full-line work on every frame
- loading, saving, searching, and replace-all must not block the UI thread
  for long periods; show progress or a busy indicator for lengthy work
- searches must be cancellable and must not display stale results after edits
- keep repainting and unrelated tabs responsive during background operations
- it is acceptable to temporarily disable editing in an affected document,
  but make this visible and do not silently drop input
- scrolling, cursor movement, and ordinary edits must remain responsive
  after loading; avoid full-document rescans for each keystroke
- if editing remains enabled during a save, save a consistent snapshot
  and keep later changes marked as unsaved
- document the buffer design, indexing strategy, and memory trade-offs

no syntax highlighting, language server, plugins, terminal, or rich-text
formatting is required. prioritize correct editing, data safety, and
large-file responsiveness.
```

#### Test tasks

| Task | Required behavior |
|---|---|
| Files and tabs | Open, create, save, and reopen documents correctly; preserve supported encoding/line endings and independent tab state; handle unsaved changes and file errors safely. |
| Editing and selection | Correct keyboard/mouse navigation, selection, insertion, deletion, and clipboard operations, including Unicode and multiline text. |
| Undo and redo | Restore edits, cursor, and selection correctly; handle grouped typing, paste, redo branching, and saved-state tracking. |
| Find and replace | Correct match counts, highlighting, navigation, case handling, replacement, and undoable replace-all. |
| Large-file responsiveness | Load and edit the large-file (90 MiB), scroll through them, and perform background operations without freezing the interface or corrupting data. |

### 5. GPU Ray Tracer using Vulkan in Rust

```text
write a desktop GPU ray tracer in Rust using Vulkan compute shaders.
render one fixed scene in a window; no scene loader is required.

target Windows x64 and an AMD Radeon RX 5700 XT with Vulkan 1.2 support.
hardware ray-tracing extensions must not be required.
do not use VK_KHR_ray_tracing_pipeline or VK_KHR_acceleration_structure.

implement ray generation, intersections, shading, shadows, reflections,
and sample accumulation on the GPU. do not render on the CPU or substitute
rasterization for ray tracing.

Rust Vulkan bindings, windowing, math, image encoding, interface, and
shader compilation libraries are allowed. do not embed an existing
renderer or game engine. GLSL compute shaders compiled to SPIR-V are
allowed; host code must be written in Rust.

provide complete source, pinned dependencies, shader sources, and
build/run instructions. compile shaders as part of the build.
document the required Vulkan SDK and driver capabilities.

fixed scene:
- use a right-handed coordinate system with Y pointing upward
- build an open-front room spanning:
  - X: -3 to 3
  - Y: 0 to 6
  - Z: -3 to 3
- leave the front at Z = 3 open
- construct the floor, ceiling, back wall, and two side walls from
  two triangles each, for ten triangles in total
- use these linear RGB base colors:
  - left wall at X = -3: (0.65, 0.05, 0.05)
  - right wall at X = 3: (0.05, 0.65, 0.05)
  - floor, ceiling, and back wall: (0.75, 0.75, 0.75)
- place three spheres:
  - matte blue: center (-1.6, 1.0, -0.8), radius 1.0
  - glossy gold: center (1.4, 1.0, -1.0), radius 1.0
  - mirror: center (0.0, 0.75, 1.0), radius 0.75
- use one white point light at (0.0, 5.5, 1.0),
  with RGB intensity (60.0, 60.0, 60.0)
- use a black background outside the room
- do not require external models, textures, or downloaded assets

camera:
- initial position: (0.0, 3.0, 10.0)
- look-at target: (0.0, 2.5, 0.0)
- up vector: (0.0, 1.0, 0.0)
- vertical field of view: 45 degrees
- initial render resolution: 1280x720
- generate perspective rays with the correct aspect ratio
- provide mouse orbit and zoom controls, with documented limits,
  plus a reset-to-default-camera control
- include a fixed-camera benchmark mode

GPU intersections:
- represent spheres analytically, not as tessellated meshes
- implement ray-sphere and ray-triangle intersections in compute shaders
- test all scene primitives and select the nearest valid intersection
- brute-force intersection testing is sufficient for this small scene;
  a BVH is not required
- accept hits only within a specified ray-distance interval
- handle rays originating inside spheres, tangent hits, parallel rays,
  and misses safely
- use two-sided triangles and orient shading normals against
  the incoming ray
- use deterministic primitive ordering to resolve equal-distance hits
- offset secondary-ray origins using a documented numerical tolerance
  to avoid self-intersection without visibly detached shadows
- provide GPU intersection tests using known rays with results read
  back to the CPU for verification

materials and lighting:
- support Lambert diffuse, Blinn-Phong specular, and perfect mirror reflection
- use ambient illumination of 0.02 multiplied by the base color
- for direct lighting, use:
  - N: oriented unit surface normal
  - L: normalized direction toward the light
  - V: normalized direction opposite the incoming ray
  - H: normalize(L + V)
  - attenuation: light intensity / squared distance to the light
  - diffuse: baseColor * kd * max(dot(N, L), 0)
  - specular: white * ks * pow(max(dot(N, H), 0), shininess)
- set direct lighting to zero when dot(N, L) is not positive
- handle a zero-length half-vector safely by using zero specular
- local color = ambient + visibility * attenuation * (diffuse + specular)
- final color = (1 - reflectivity) * local color
  + reflectivity * reflected color
- all color multiplications are component-wise

material parameters:
- walls: kd 1.0, ks 0.0, shininess 1, reflectivity 0.0
- blue sphere: baseColor (0.05, 0.15, 0.8),
  kd 1.0, ks 0.0, shininess 1, reflectivity 0.0
- gold sphere: baseColor (0.8, 0.5, 0.1),
  kd 0.8, ks 0.5, shininess 64, reflectivity 0.15
- mirror sphere: baseColor (1.0, 1.0, 1.0),
  kd 0.0, ks 0.0, shininess 1, reflectivity 1.0

shadows and reflections:
- cast hard-shadow rays with their maximum distance limited to the light
- geometry behind the light must not cast a shadow
- ambient illumination remains present in shadow
- implement reflections with an iterative shader loop, not recursion
- support zero through four reflection bounces after the primary ray
- weight reflected contributions according to the material reflectivity
- at the bounce limit, use black for the untraced reflected contribution
- terminate rays that miss the scene or have zero remaining contribution

color processing:
- compute and accumulate lighting in linear RGB using 32-bit floats
- average samples before applying exposure
- apply Reinhard tone mapping component-wise: c / (1 + c)
- convert the tone-mapped result to sRGB exactly once
- default exposure is 1.0
- use the same color processing for display and PNG export

sampling and accumulation:
- progressively accumulate samples in a floating-point GPU image
- provide 1, 16, and 64 samples-per-pixel presets
- use center-of-pixel sampling for the one-sample preset
- use deterministic seeded subpixel sampling for higher presets
- derive randomness from pixel coordinates, sample index, and seed
- the same settings and seed must reproduce the same decoded image
  pixels on the same build, GPU, and driver
- stop accumulation when the selected sample count is reached
- reset accumulation when the camera, render resolution, or sample
  preset or reflection limit changes
- exposure changes may reuse accumulated linear-color samples
- keep sample count independent of display frame rate

Vulkan implementation:
- use a compute pipeline to trace rays and update the accumulation image
- assign independent pixels to shader invocations without write races
- bounds-check invocations at image edges
- use a fullscreen presentation pass or image transfer to display the
  computed image; rasterization is allowed only for presentation and UI
- manage descriptors, image layouts, memory visibility, and synchronization
  correctly between compute, presentation, and readback
- validate required image-format features and device limits
- use manageable dispatch batches rather than one excessively long
  dispatch that risks a Windows GPU timeout
- do not require disabling or increasing the system GPU timeout
- correctly handle swapchain recreation and image-resource resizing
- wait for GPU work before destroying resources still in use
- provide an option to enable Vulkan validation layers
- report unsupported capabilities clearly without silently falling back
  to CPU rendering

window and controls:
- display the rendered image, accumulated sample count, resolution,
  GPU name, and elapsed render time
- provide controls for sample count, reflection limit, and exposure
- provide start/restart rendering, camera reset, and save-PNG controls
- save the image at its render resolution without interface overlays
- clearly report whether an export contains a partial or completed render
- handle resizing and minimization safely
- keep the interface responsive during accumulation
- repeated resizing and render restarts must not leak GPU resources

benchmark mode:
- render the default camera at 1280x720, 64 samples per pixel,
  four reflection bounces, exposure 1.0, and seed 12345
- perform one unmeasured warm-up render, clear accumulation, then perform
  the measured render using the same settings
- report initialization and pipeline-creation time separately
- use Vulkan GPU timestamp queries to measure compute rendering work
- also report wall-clock time to complete the measured accumulation,
  including waiting for GPU completion
- exclude PNG encoding and disk writes from rendering timings
- save the final PNG and a machine-readable report containing the GPU,
  driver, settings, completed sample count, and timing measurements
- do not claim CPU submission time is GPU execution time

global illumination, soft shadows, refraction, textures, denoising,
and real-time 60 FPS rendering are not required.
prioritize correct GPU ray tracing, Vulkan resource management,
and reproducible output.
```

#### Test tasks

| Task | Required behavior |
|---|---|
| Vulkan initialization and presentation | Launch on a RX 5700 XT, render using Vulkan compute shaders without hardware ray-tracing extensions, and display the fixed scene and controls. |
| Geometry and visibility | Pass known GPU ray-intersection tests and render the specified room and analytic spheres with correct perspective, nearest-hit visibility, normals, and silhouettes. |
| Lighting and reflections | Produce the specified diffuse/specular shading, hard shadows, mirror reflections, bounce-limit behavior, tone mapping, and sRGB conversion. |
| Sampling and interaction | Accumulate samples reproducibly; handle camera changes, settings, resizing, and restarts correctly; export PNGs without stale samples or interface overlays. |
| Vulkan reliability and benchmarking | Complete the fixed benchmark with valid GPU and wall-clock timings; pass repeated resize/restart tests without validation errors, GPU timeouts, or sustained GPU-memory growth. |


### 6. GPU Inference Engine in Rust

```text
write a custom LLM inference engine in Rust that loads GGUF files
directly and runs Qwen3-4B-Instruct-2507 on the GPU through Vulkan
compute shaders, in six quantizations: Q8_0, Q6_K, Q5_K_M, Q4_K_M,
Q3_K_M, and Q2_K.

do not use an existing inference engine, model runtime, or tensor
framework. llama.cpp, ggml, gguf crates, candle, burn, tch, torch,
onnxruntime, tract, mistral.rs, ollama, and vllm are not allowed, as
a dependency or as a subprocess.

raw Vulkan bindings such as ash or vulkano are allowed, as are
windowing, memory-mapping, math, shader compilation, CLI, and
serialization crates. wgpu and other cross-API abstraction layers are
not allowed; the implementation must target Vulkan directly and manage
its own descriptors, memory, and synchronization. GLSL or slang compute
shaders compiled to SPIR-V are allowed and must be compiled as part of
the build; host code must be written in Rust.

all tensor math for the transformer must execute in Vulkan compute
shaders. do not run the forward pass on the CPU, and do not substitute
a CPU BLAS for the GPU path. a CPU reference implementation is required
only for the verification tests described below.

provide complete source, pinned dependencies, shader sources,
build/run instructions, and the exact commands used to download each
quantization. document the required Vulkan SDK version and the driver
capabilities relied upon.

target machine:
- Windows x64, AMD Radeon RX 5700 XT, 8 GiB VRAM, Vulkan 1.2
- RDNA1 does not expose VK_KHR_cooperative_matrix; it must not be
  required, and neither may any hardware matrix or tensor extension
- ROCm and HIP do not support this GPU; do not depend on them
- 8-core AMD FX-8300, 12 GiB system RAM, so the host cannot hold a
  dequantized copy of the model
- query optional capabilities such as VK_KHR_shader_float16_int8,
  subgroup size, and shared-memory limits, and select a code path
  accordingly instead of assuming them
- report unsupported capabilities clearly; never silently fall back to
  CPU inference

GGUF loading:
- parse the GGUF container yourself: magic, version, tensor count,
  metadata key-value pairs of every GGUF type, tensor descriptors,
  alignment, and data offsets
- read model shape and hyperparameters from metadata, not hardcoded
  constants, and fail clearly on an unsupported architecture
- memory-map the file and stream tensor data to the GPU in bounded
  staging chunks; never read the whole file into host memory
- validate offsets, tensor dimensions, and block counts, and report
  truncated or corrupt files as errors rather than panicking
- print a model summary: architecture, parameter count, quantization
  mix per tensor type, context length, and file size

quantized weights on the GPU:
- upload weights in their quantized block form and dequantize inside
  the shaders; do not expand tensors to F16 or F32 on the host or store
  a dequantized copy of the model in VRAM
- implement GPU dequantization for F32, F16, Q8_0, Q6_K, Q5_K, Q4_K,
  Q3_K, and Q2_K, matching the ggml block layouts exactly, including
  superblock scales, minimums, and packed 6-bit and high-bit fields
- the _K_M mixes contain several tensor types in one file; dispatch the
  correct routine per tensor rather than assuming one format
- fuse dequantization into the matrix kernels so quantized bytes are
  read once per use, and document the block-to-invocation mapping
- respect the 8 GiB VRAM budget: compute and report the required
  capacity for weights, KV cache, and scratch buffers before
  allocating, and refuse or partially offload with a clear explanation
  rather than failing mid-load
- if partial offload is implemented, document the split policy and keep
  results consistent with full offload
- document the block layouts, buffer layouts, and memory strategy

model execution:
- implement the Qwen3 dense decoder yourself: token embedding,
  RMSNorm, grouped-query attention, per-head query and key RMSNorm,
  rotary position embeddings, SwiGLU feed-forward, final norm,
  and the output projection
- handle non-square projections where num_heads * head_dim differs
  from hidden_size
- handle tied input and output embeddings
- read rope theta, RMSNorm epsilon, and KV head count from metadata
- implement the batched matrix-multiply path used by prefill and the
  matrix-vector path used by decode; a single naive kernel for both is
  not acceptable
- accumulate in float32, use a numerically stable softmax, and apply
  rotary embeddings in float32 regardless of the storage precision
- keep the KV cache in GPU memory, preallocated, with a configurable
  context length supporting at least 8192 tokens, and report its cost
- attention must read the cache directly on the GPU; do not copy the
  cache to the host between tokens
- refuse to exceed the context window rather than corrupting the cache

tokenizer:
- build the byte-level BPE tokenizer from the GGUF vocabulary, merges,
  and token types; do not use an existing tokenizer crate
- round-trip arbitrary UTF-8, including emoji and CJK text
- handle the special tokens, and apply the chat template so multi-turn
  conversations match the model's expected format
- stream partial output without emitting broken UTF-8 for tokens that
  end mid-codepoint

generation:
- support greedy decoding, temperature, top-k, top-p, min-p, and
  repetition penalty
- stop on the end-of-turn token, on a token budget, and on Ctrl+C,
  releasing GPU resources cleanly
- a fixed seed, prompt, and settings must reproduce identical output
  on the same build, GPU, and driver; use a deterministic reduction
  order in the shaders and document where floating-point associativity
  could otherwise break reproducibility
- stream tokens to the terminal as they are produced
- provide a single-shot prompt mode, an interactive chat mode, and a
  machine-readable output mode

Vulkan implementation:
- use compute pipelines for every stage of the forward pass
- assign independent outputs to invocations without write races, and
  bounds-check invocations at buffer and tile edges
- manage descriptor sets, buffer memory, queue submission, barriers,
  and memory visibility correctly between dispatches, uploads, and
  readback
- split long work, including prefill of a long prompt, into dispatch
  batches sized to avoid a Windows GPU timeout; do not require
  disabling or increasing the system TDR delay
- wait for GPU work before destroying or reusing resources still in use
- provide an option to enable Vulkan validation layers, and run clean
  under them
- provide device enumeration and selection, and report the selected
  GPU name, driver version, and Vulkan version
- repeated generations, context resets, and model reloads must not leak
  GPU memory, descriptors, or command buffers

verification and measurement:
- provide a self-test that checks each GPU dequantization routine
  against known block bytes with expected float results, read back to
  the host for comparison
- provide a CPU reference implementation of the forward pass, used only
  to verify the GPU: compare logits for a fixed prompt within a stated
  tolerance and report the maximum deviation
- provide a command that reports perplexity over a supplied UTF-8 text
  file so quantizations can be compared
- provide a benchmark reporting initialization and pipeline-creation
  time separately, load time, prefill tokens per second, decode tokens
  per second, peak VRAM, and peak host memory
- use Vulkan GPU timestamp queries for the compute work, and also
  report wall-clock time including waiting for GPU completion; do not
  claim CPU submission time is GPU execution time
- save a machine-readable report containing GPU, driver, quantization,
  settings, token counts, and timing measurements
- report progress during model upload and long prompt prefill

no training, fine-tuning, LoRA, speculative decoding, concurrent
request batching, vision input, multi-GPU, or HTTP server is required.
prioritize correct quantized GPU inference, sound Vulkan resource
management, reproducible output, and honest measurement.
```

#### Test tasks

| Task | Required behavior |
|---|---|
| Model loading and Vulkan setup | Launch on the RX 5700 XT without cooperative-matrix or hardware matrix extensions, parse the GGUF file, report the correct architecture and quantization mix, and upload weights within the 8 GiB VRAM budget. |
| Quantization coverage | Run all six quantizations on the GPU; dequantization self-tests pass against known blocks and perplexity degrades in the expected order. |
| Generation correctness | GPU logits match the CPU reference within tolerance; produce coherent answers with a correct chat template, working stop tokens, and reproducible output for a fixed seed. |
| Sampling and context | Temperature, top-k, top-p, min-p, and repetition penalty materially change output; multi-turn chat and GPU KV-cache reuse stay correct up to the configured context. |
| Vulkan reliability and benchmarking | Complete the benchmark with valid GPU timestamp and wall-clock figures; survive repeated generations, resets, and reloads with no validation errors, GPU timeouts, or sustained VRAM growth. |

### 7. Near-Duplicate Image Finder in Python

```text
write a Python program that scans large image collections for exact,
near-duplicate, and transformed-duplicate images, using a persistent
index so repeat scans are incremental.

provide complete source, pinned dependencies, and setup/run instructions.
do not use an existing duplicate-finder library or service; imagededup,
difPy, czkawka, and equivalents are not allowed. a decoder such as
Pillow, a numeric library, and an approximate-nearest-neighbour library
are allowed.

input:
- accept one or more folder paths as command-line arguments
- support optional recursive scanning of subfolders
- support JPEG, PNG, WebP, GIF, and APNG; animated WebP, GIF, and APNG
  are compared on their first frame
- handle 8-bit, 16-bit, grayscale, palette, and CMYK images
- convert to sRGB using the embedded ICC profile when present
- handle paths containing spaces, Unicode, and long Windows paths
  beyond 260 characters
- do not follow directory symlinks, and do not rescan the same
  physical directory twice through junctions or reparse points
- guard against decompression bombs with a documented pixel limit

image comparison:
- detect byte-identical files and visually equivalent images stored
  with different compression, metadata, formats, or resolutions
- detect minor brightness, contrast, gamma, and saturation changes
  without treating unrelated images with similar colors as duplicates
- match the eight dihedral variants: rotations of 90, 180, and 270
  degrees and horizontal, vertical, and diagonal mirroring
- match crops that retain at least 70 percent of the original area,
  and letterboxed or padded versions of the same image
- match versions carrying a small watermark or logo overlay
- arbitrary small-angle rotation and heavy artistic filtering need not
  match; document exactly which transformations are in and out of scope
- normalize EXIF orientation before comparison, and do not apply it
  twice for images that also carry a rotated ICC or container hint
- handle transparency consistently and document the approach
- use image content rather than filenames, timestamps, or file sizes
- combine more than one descriptor, such as a perceptual hash plus a
  color or block descriptor, and document how they are fused into a
  single score
- expose a configurable similarity threshold with a documented default
  and explain whether higher values mean stricter or looser matching
- provide a documented way to tune the threshold from a labelled set

index and incremental scanning:
- store descriptors in a persistent local index, such as SQLite, keyed
  by a content hash and not by path alone
- a rescan must only decode files that are new or changed, detected by
  size and modification time with content-hash confirmation, and must
  reuse stored descriptors for everything else
- moving or renaming an indexed file must not trigger redecoding
- detect and prune index entries for deleted files
- version the index schema and descriptor parameters, and rebuild or
  migrate cleanly instead of mixing incompatible descriptors
- the index must survive an interrupted scan without corruption, and
  a resumed scan must continue rather than start over
- report index hit and miss counts for each scan

grouping:
- group matching images and choose one original to keep per group
- prefer the image with the largest pixel area, then the largest file
  size, then the lexicographically smallest full path to break ties
- each listed duplicate must meet the similarity threshold against
  its group's retained original; do not group unrelated endpoints
  solely through a chain of intermediate matches
- assign each file to at most one group
- produce identical results for the same files and settings regardless
  of scan order, worker count, or the order paths are supplied

output:
- list each duplicate group with the retained original clearly marked
- show each file's path, dimensions, and size in bytes
- show the similarity distance or score against the retained original,
  and which transformation was detected, such as identical, recompressed,
  rescaled, rotated, mirrored, cropped, or color-adjusted
- distinguish byte-identical duplicates from perceptual matches
- report scanned files, processed images, skipped files, reasons for
  skipping, duplicate groups, and duplicate files
- report the total size of duplicate files, excluding the one retained
  original in each group, in exact bytes and human-readable units
- describe it as potential savings based on logical file sizes, not
  guaranteed disk space recovered
- provide a stable machine-readable JSON or JSONL report alongside the
  terminal output, with a documented schema and a schema version
- provide an evaluation mode that takes a ground-truth grouping file
  and reports precision, recall, and the false positives and false
  negatives by path

performance and scale:
- handle a collection of at least 100,000 images
- decode in parallel across processes, with a configurable worker count
  defaulting to something sensible for an eight-core machine
- use approximate nearest-neighbour search, a metric tree, or LSH
  instead of full pairwise comparison, and report the number of
  candidate pairs actually scored so sublinear behavior is verifiable
- keep peak resident memory bounded and documented, well under 2 GiB
  for a 100,000-image scan, by streaming decodes and storing only
  compact descriptors
- show progress with throughput and an estimate of remaining time
- handle Ctrl+C promptly, leaving no orphaned worker processes
- document measured throughput, the recall trade-off of the chosen
  search structure, and known performance limitations

safety and reliability:
- this is a read-only tool; do not delete, rename, or modify any
  scanned file, and keep the index outside the scanned folders
- count hard links and identical inodes to the same underlying file
  only once
- skip unsupported, corrupted, truncated, or unreadable files with a
  warning, without terminating the entire scan, and survive a worker
  process that crashes or is killed while decoding
- handle empty folders, folders with a single image, and folders
  containing no duplicates
- never report a group whose retained original is missing or unreadable
```

#### Test tasks

| Task | Required behavior |
|---|---|
| Exact and near duplicates | Detect identical files with different names or locations, plus resized, recompressed, format-converted, and mildly color-adjusted copies. |
| Transformed duplicates | Detect the eight dihedral variants, crops retaining at least 70 percent of the area, letterboxed copies, and watermarked copies. |
| Accuracy scoring | Run the labelled fixture set in evaluation mode and meet the predefined precision and recall thresholds, keeping visually similar but distinct scenes separate. |
| Index and incrementality | Rescan an unchanged folder with no redecoding, handle renamed, deleted, and modified files correctly, and resume cleanly after an interrupted scan. |
| Scale and reliability | Scan the 100,000-image collection within the memory and candidate-pair budgets, with deterministic grouping across worker counts, Unicode and long paths, corrupted files, and Ctrl+C. |

### 8. 3D Racing Game in JavaScript

```text
write a complete browser-based 3D racing game in JavaScript with
believable vehicle handling, a working gearbox, and a playable race.

use Three.js or another JavaScript 3D rendering library. a physics
library is allowed, but do not embed an existing racing game.
provide complete source, pinned dependencies, and setup/run instructions.

target desktop browsers with keyboard controls. support current Chrome,
Firefox, and Edge. mobile support and multiplayer are not required.
no backend or external asset downloads should be needed during gameplay.

vehicle simulation:
- implement acceleration, braking, coasting, steering, and reverse
- model engine RPM, a torque curve, gear ratios, final drive,
  wheel radius, and aerodynamic and rolling resistance
- provide at least five forward gears, neutral, and reverse
- gear selection must affect acceleration, engine RPM, and top speed;
  do not implement gears as a cosmetic HUD change
- include engine idle, a redline, and a rev limiter
- implement a brief torque interruption during gear changes
- prevent unsafe shifts into reverse while moving forward
- support both automatic and sequential manual transmission
- manual clutch operation, engine damage, and stalling are not required
- use a simplified tire-grip model with believable lateral traction,
  speed-dependent steering, and reduced grip on grass
- allow loss of traction under excessive cornering or acceleration
- implement suspension response and visible body pitch/roll
- use a fixed physics timestep with rendering interpolation;
  handling must not depend on rendering frame rate

track and race:
- provide one complete closed circuit with straights, slow corners,
  fast corners, grass/runoff areas, and solid barriers
- provide a controllable car and at least three AI opponents
- AI cars must follow the circuit, brake for corners, and complete laps
  using the same vehicle physics and grip rules as the player
- include collisions with barriers and other cars without routinely
  allowing cars to pass through them
- provide a countdown followed by a three-lap race
- count laps using ordered checkpoints so reversing across the finish
  or cutting across the circuit does not award a lap
- show race position based on lap count and progress around the track
- track current lap time, last lap time, best lap time, and total race time
- end the race with a results screen and restart option
- provide a reset control for a stuck or overturned car, placing it
  safely near its last valid track position without advancing progress
- apply a visible five-second race-time penalty for each reset

controls and interface:
- W/Up: throttle
- S/Down: brake; in reverse gear, apply reverse throttle near standstill
- A/D or Left/Right: steer
- E/Q or shift/control: shift up/down in manual mode
- provide explicit controls for selecting neutral and reverse
- provide a control to switch automatic/manual transmission
- C: change camera
- R: reset car
- Escape: pause/resume
- display all controls on a help screen
- smooth keyboard steering and throttle inputs rather than applying
  instantaneous full steering angle

HUD:
- speed in km/h
- engine RPM and rev counter
- current gear and transmission mode
- race position and current lap
- current, last, and best lap times
- final race time

presentation:
- provide a cohesive, realistic visual style with correctly scaled cars,
  road markings, terrain, lighting, shadows, and trackside scenery
- include rotating wheels and visibly steered front wheels
- provide a smooth chase camera and a hood camera
- keep the chase camera usable near barriers and during collisions
- include engine audio whose pitch responds to RPM, plus braking/skid
  and collision sounds
- provide volume and mute controls
- initialize browser audio after user interaction and handle unavailable
  audio gracefully
- use original, procedurally generated, or permissively licensed assets,
  with attribution where required

lifecycle and performance:
- provide a start menu, pause menu, and race results screen
- pause when the browser tab is hidden or the window loses focus
- clear held inputs on focus loss and require explicit resume
- restarting must reset cars, timers, checkpoints, penalties, and race state
  without reloading the page
- support window resizing and configurable graphics quality
- aim for 60 FPS at 1920x1080 on the specified evaluation hardware
- repeated restarts must not accumulate objects, event handlers,
  audio sources, or GPU resources
- save best lap times and settings locally, and continue working if
  browser storage is unavailable

provide a short explanation of the vehicle model, units, gearbox,
AI driving logic, and known simplifications. include a debug overlay
showing speed, RPM, gear ratio, and physics timestep so the simulation
can be checked. prioritize a complete, believable racing experience
over claiming professional simulator accuracy. aim for at least 60 FPS.
```

#### Test tasks

| Task | Required behavior |
|---|---|
| Launch and presentation | Launch in the specified browsers with a complete 3D track, working menus, cameras, HUD, audio after interaction, and responsive resizing. |
| Vehicle and gearbox | Acceleration, braking, steering, grip, and collisions work believably; manual/automatic gears materially affect RPM and performance; neutral and reverse behave correctly. |
| Race and AI | Complete a three-lap race against three functioning opponents, with correct checkpoints, lap timing, position tracking, reset penalties, and results. |
| Controls and lifecycle | All documented controls work; pause, focus loss, resume, reset, and repeated restarts preserve correct state without stuck inputs or unintended progress. |
| Performance and persistence | Meet predefined frame-time limits on fixed hardware; repeated races do not cause sustained resource growth; settings and best laps persist, with graceful storage/audio failure handling. |

### 9. Browser Image Editor in JavaScript

```text
write a browser-based image editor in JavaScript with image loading,
transformations, drawing, color adjustments, undo/redo, and export.

use Canvas APIs or a suitable rendering library, but do not embed an
existing image editor. provide complete source, pinned dependencies,
and setup/run instructions.

support current Chrome, Firefox, and Edge on desktop. all image
processing must happen locally without uploading images to a server.

image loading:
- open JPEG, PNG, and WebP through a file picker or drag and drop
- correctly apply EXIF orientation without rotating the image twice
- preserve transparency where supported
- show image dimensions and the current zoom level
- report unsupported or corrupted files without losing the current image
- warn before replacing an image with unexported changes
- support images up to 4096x4096 pixels

viewport:
- provide zoom in/out, fit-to-window, and actual-size views
- support panning when the image extends beyond the viewport
- show a checkerboard behind transparent areas
- keep editing coordinates correct at every zoom and pan position
- zooming, panning, and resizing the browser window must not modify
  the image or create undo steps

transformations:
- crop using a draggable rectangle with resize handles
- support freeform and locked-aspect-ratio cropping
- allow entering exact crop coordinates and dimensions in image pixels
- resize to exact pixel dimensions, with optional aspect-ratio locking
- rotate 90 degrees clockwise, 90 degrees counterclockwise, and 180 degrees
- mirror horizontally by swapping left and right
- mirror vertically by swapping top and bottom
- apply transformations to the complete current image, including drawings
- update document dimensions correctly after cropping, resizing, or rotation
- allow crop previews to be applied or cancelled without unwanted changes

drawing tools:
- provide a freehand brush with adjustable size, color, and opacity
- provide an eraser that removes pixels to transparency rather than
  painting them white
- provide straight-line, rectangle, and ellipse tools
- allow shapes to use an outline, a fill, or both
- provide an eyedropper to sample a pixel's color
- show a tool cursor or preview that reflects brush size and placement
- treat one pointer drag as one undoable drawing action
- brush size must be defined in image pixels, independent of zoom

image adjustments:
- provide brightness and contrast controls with neutral defaults
- provide grayscale and color-inversion actions
- preserve alpha when changing colors
- show adjustment previews without repeatedly compounding the effect
- allow applying or cancelling adjustments
- document the adjustment formulas and slider ranges so results
  can be checked against reference calculations

undo and redo:
- support Ctrl+Z and Ctrl+Y, plus visible undo/redo buttons
- undo transformations, drawings, erasing, and applied adjustments
- restore pixel content and document dimensions correctly
- editing after undo must discard the redo branch
- support at least 20 undoable operations for images up to 1920x1080
- bound history memory usage and document the limit; for larger images,
  older history may be evicted with a visible notice
- export must not clear undo history

export:
- export the full edited image as PNG or JPEG, not a screenshot
  of the visible viewport
- preserve transparency in PNG
- for JPEG, flatten transparency onto a user-selected background color,
  defaulting to white
- provide a JPEG quality control and filename input
- ensure exported dimensions match the current document dimensions
- do not include selection handles, checkerboards, or other interface
  overlays in exported images
- allow exported files to be reopened for further editing

interface and reliability:
- provide clearly labeled tools, controls, and keyboard shortcuts
- show active tool, image dimensions, and zoom level
- disable unavailable actions rather than failing silently
- reject invalid dimensions and out-of-bounds crop values clearly
- provide progress or a busy state during expensive operations
- keep the interface responsive and prevent conflicting edits while
  an operation is running
- release obsolete image resources and object URLs when no longer needed
- repeated loading, editing, and exporting must not cause sustained
  memory growth outside the documented history budget

layers, text tools, animated image editing, RAW files, and advanced
color-profile management are not required. use a single raster document
and evaluate color operations using supplied sRGB fixtures.
```

#### Test tasks

| Task | Required behavior |
|---|---|
| Image loading and viewport | Open supplied images with correct dimensions, orientation, and transparency; zoom and pan without altering pixels; handle invalid files safely. |
| Transformations | Crop and resize to exact dimensions; rotate correctly; mirror left/right and top/bottom correctly, including existing drawings. |
| Drawing and adjustments | Apply brushes, erasing, shapes, and color adjustments correctly at different zoom levels; cancelled previews leave the image unchanged. |
| Undo and redo | Restore pixels and dimensions through a mixed sequence of operations; correctly handle redo branching, cancelled actions, and history limits. |
| Export and reliability | Export and reopen PNG/JPEG with correct dimensions, transparency or background flattening, and no UI overlays; handle large images and repeated operations within predefined resource limits. |

### 10. Memory Allocator in C

```text
write a memory allocator in C implementing malloc, free, and realloc
behavior without using the C runtime allocator internally.

target 64-bit Windows with C17 and MinGW-w64 GCC.
provide complete source, build/run instructions, and automated tests.

expose this API:
- void *my_malloc(size_t size);
- void my_free(void *ptr);
- void *my_realloc(void *ptr, size_t size);

use prefixed names so the test harness can use the standard allocator
independently. replacing the process-wide allocator is not required.

memory acquisition:
- acquire and release memory through VirtualAlloc and VirtualFree
- do not use malloc, calloc, realloc, free, HeapAlloc, or equivalent
  allocator libraries inside the implementation
- do not perform one operating-system allocation per small allocation;
  acquire larger regions and manage blocks within them
- dedicated regions for large allocations are allowed
- store allocator metadata in memory you manage
- check operating-system failures and arithmetic overflow

allocation:
- return a pointer to at least the requested number of usable bytes
- align returned pointers to _Alignof(max_align_t)
- live allocations must never overlap
- allocated memory does not need to be zero-initialized
- return NULL if an allocation cannot be satisfied
- define my_malloc(0) to return NULL
- reject impossible sizes safely instead of wrapping size calculations
- do not impose a small fixed limit on allocation count or total capacity

freeing:
- my_free(NULL) must do nothing
- make freed blocks available for reuse
- split oversized free blocks when the remainder can hold a valid block
- coalesce physically adjacent free blocks within the same region
- do not merge blocks across unrelated regions
- release completely unused regions back to the operating system;
  retaining at most one empty normal-sized region for reuse is allowed
- double-free and invalid pointers are outside the required contract;
  document them as undefined behavior

reallocation:
- my_realloc(NULL, size) must behave like my_malloc(size)
- my_realloc(ptr, 0) must free ptr and return NULL
- preserve the first min(old_requested_size, new_size) bytes
- shrinking must keep the same pointer and make a sufficiently large
  remainder available for reuse
- support growing in place when the immediately following free block
  provides enough space
- otherwise allocate another block, copy the preserved bytes,
  and free the original block
- if growth fails, return NULL and leave the original allocation
  and its contents valid and unchanged
- a failed realloc must not leak a temporary allocation

design and diagnostics:
- implement and document the block layout, alignment rules,
  free-block search strategy, splitting, and coalescing
- single-threaded operation is sufficient; document that the allocator
  is not thread-safe
- provide debug-only validation of region boundaries, block alignment,
  free-list consistency, and nonoverlapping blocks
- provide debug statistics for live allocations, requested live bytes,
  managed free bytes, and operating-system region allocations/releases
- keep debug validation separate from release performance measurements
- make the operating-system allocation layer injectable in tests so
  allocation failures can be triggered deterministically

tests:
- test tiny allocations, alignment boundaries, large allocations,
  zero sizes, and requests near SIZE_MAX
- fill allocations with known byte patterns and check that unrelated
  allocations and reallocations do not corrupt them
- test freeing blocks in different orders, reuse, splitting,
  adjacent-block coalescing, and empty-region release
- test realloc shrinking, in-place growth, moved growth, and failure
- include a deterministic randomized sequence of allocations, frees,
  and reallocations with integrity checks after each operation
- include a benchmark reporting operation throughput, peak managed
  memory, and operating-system allocation counts

prioritize correctness and memory reuse over matching the performance
of a production allocator. calloc, over-aligned allocations, garbage
collection, and multithreaded support are not required.
```

#### Test tasks

| Task | Required behavior |
|---|---|
| Allocation and alignment | Allocate writable, correctly aligned, nonoverlapping blocks across a range of sizes. |
| Freeing and reuse | Reuse freed memory, split blocks, coalesce adjacent blocks, and release empty regions according to the specified policy. |
| Reallocation | Correctly shrink, grow in place, and move allocations while preserving their contents. |
| Integrity under stress | Pass a fixed-seed randomized sequence of allocations, frees, and reallocations without corruption. |
| Edge cases and failure safety | Handle zero sizes, null, impossible sizes, and injected OS allocation failures without crashes, leaks, or invalidating existing allocations. |

### 11. ZIP Archive Tool in Zig

```text
write a ZIP archive tool in Zig that lists, tests, extracts, creates,
and updates .zip archives, reading and writing the ZIP container
format directly and implementing DEFLATE compression yourself.

target 64-bit Windows. provide complete source, build/run
instructions, and automated tests. do not use Zig's standard library
compression or archive modules (std.compress, std.flate, and any
DEFLATE or ZIP support) or any external compression or archive
library; zlib, miniz, libzip, libarchive, and equivalents are not
allowed, as a dependency or as a subprocess. invoking an external zip
tool at runtime is also forbidden; such tools may be used only to
prepare fixtures.

manage all memory through explicit allocators. the test suite must run
leak-free under std.testing.allocator and the release build must run
under a leak-detecting allocator such as std.heap.GeneralPurposeAllocator
without reporting leaks. document the ownership of every buffer.

provide these subcommands: list (detailed), test (integrity),
extract, create, and add/update.

container format:
- locate the end of central directory record and parse the ZIP64 EOCD
  locator and record when the 0xFFFFFFFF/0xFFFF sentinel values appear;
  support archives with more than 65535 entries and members larger than
  4 GiB
- read the central directory as the authoritative index, and never
  extract from local file headers alone; document this choice
- support methods 0 (store) and 8 (deflate); report other methods
  clearly without crashing
- honor data descriptors: when the local header's general-purpose
  flag 3 is set, read sizes and CRC from the central directory, not
  the local header
- decode UTF-8 names via general-purpose flag 11, and document the
  handling of non-UTF-8 names and of the Info-ZIP Unicode path extra
  field
- apply the DOS timestamp field to extracted files
- on extraction, restore Unix permission bits from the external
  attributes when present
- reject archive bombs by a documented, configurable ratio and total
  size limit, applied during listing before any extraction
- never write outside the destination directory: reject absolute
  paths, drive letters, and ".." traversal in member names
- detect and report encrypted entries without attempting decryption

DEFLATE implementation:
- implement decompression (inflate) covering stored blocks, fixed
  Huffman tables, and dynamic Huffman tables, including the second
  code-length alphabet order and repeat codes 16, 17, and 18
- implement compression (deflate) producing blocks a standard
  decompressor accepts, with at minimum a stored fallback, a fixed
  Huffman mode, and a dynamic Huffman mode using length-distance
  matching such as a hash chain over a 32 KiB window
- offer selectable compression levels that trade speed for ratio, and
  never corrupt data at any level
- compression may be slower than zlib but must beat store on
  compressible fixtures by a documented margin
- compute CRC-32 for every written entry with a table-driven
  implementation; verify CRC-32 and sizes on read and report the
  failing member

operations:
- list: show compressed and uncompressed sizes, ratio, method, CRC,
  timestamp, and name per entry, plus archive totals
- test: fully decompress every entry, verify CRC and sizes, and report
  each failing entry by name without stopping at the first failure
- extract: support extracting all entries or a named subset, with a
  documented overwrite policy and an option to strip directory
  structure
- create: build archives containing empty files, empty directories,
  nested paths, and zero-length members, with correct external
  attributes
- add/update: append or replace members in an existing archive without
  rebuilding entries that did not change, updating the central
  directory and ZIP64 structures accordingly
- large members must stream: neither input files nor archives may be
  loaded entirely into memory, and extraction must stay within a
  documented memory budget for multi-GiB members

robustness:
- treat archives as untrusted input: validate every offset, length,
  and count against the file size before use
- report truncated archives, bad signatures, overlapping structures,
  cyclic or out-of-range data descriptors, and CRC mismatches as clean
  errors with exit codes, never as crashes or silent truncation
- handle archives with prepended data, such as self-extracting stubs,
  by locating the EOCD from the end of the file
- use the 64 KiB EOCD search window correctly and handle trailing junk
  after the central directory

automated tests:
- round-trip create/extract across file trees containing empty,
  binary, sparse, and multi-GiB files with a byte-exact comparison
- interop fixtures: archives written by this tool must pass a standard
  tool's integrity test, and archives written by standard tools,
  including stored, deflated, data-descriptor, and ZIP64 variants,
  must extract byte-exactly
- fixed known-answer tests for fixed and dynamic Huffman decoding
- adversarial fixtures: truncated archives, bad CRCs, traversal names,
  bombs, and prepended data, each with a required error behavior
- a benchmark reporting compression and extraction throughput and the
  achieved ratio on a fixed corpus

no encryption, multi-disk spanning, or formats other than ZIP are
required. prioritize byte-exact correctness, safe handling of hostile
archives, and interop with standard tools.
```

#### Test tasks

| Task | Required behavior |
|---|---|
| Container parsing | Locate and parse the central directory, ZIP64 structures, data descriptors, and UTF-8 names across the supplied fixture archives, including self-extracting and trailing-junk variants. |
| DEFLATE correctness | Pass the fixed and dynamic Huffman known-answer tests, inflate standard-tool archives byte-exactly, and produce deflate output that a standard tool accepts. |
| Round-trip fidelity | Extract all fixture sets byte-exactly with correct timestamps, permissions, and structure, and create/update archives that pass a standard tool's integrity test, covering empty files, nested paths, multi-GiB members, and ZIP64. |
| Hostile input safety | Reject traversal, absolute paths, and bombs; report truncated, corrupt, and CRC-failing archives with clean errors and exit codes, never crashes or partial silent output. |
| Memory discipline | Run the full test suite leak-free under std.testing.allocator and the release build under a leak-detecting allocator, using explicit allocators throughout, with no hidden global allocation. |

### 12. Regex Engine in C++

```text
write a regular-expression engine in modern C++ that compiles a pattern
into an internal form and reports matches with capture-group spans,
without delegating matching to an existing regex library.

target 64-bit Windows with C++20 and MSVC or MinGW-w64 GCC/Clang.
provide complete source, build/run instructions, and automated tests.
std::regex, std::regex_search/match, boost.regex, PCRE, RE2, and any
other regex engine are not allowed, as a dependency or as a
subprocess. standard containers, std::string_view, std::variant, and
the rest of the standard library are allowed.

the engine must be value-semantic and exception-safe, own its compiled
form through RAII, and match over std::string_view without copying the
subject. matching a compiled pattern against many inputs must not
reallocate per call. document the matching strategy and its complexity.

pattern syntax (all required):
- literals, the any-character dot, and escaping of metacharacters
- character classes: ranges, negation, escapes within classes, and the
  predefined classes \d \D \s \S \w \W
- anchors ^ and $, and the word-boundary anchors \b and \B
- alternation with the lowest precedence, and grouping with ( )
- capturing groups with numbered access, and non-capturing (?: )
- greedy quantifiers * + ? and {m} {m,} {m,n}, and their lazy forms
  *? +? ?? {m,n}?
- backreferences \1 through \9 to earlier capture groups

matching semantics:
- leftmost match, with greedy quantifiers preferring the longest match
  and lazy quantifiers the shortest, exactly as backtracking defines;
  document the chosen rule and make it observable
- capture groups report byte offsets into the subject, including
  unmatched optional groups as a distinct "did not participate" state
- find the first match, iterate all non-overlapping matches, and split
  or replace with capture-group references in the replacement
- handle empty matches without infinite loops when iterating
- patterns must compile once and be matched many times; compilation
  errors must be reported with a position, not crashed on

backtracking safety:
- provide a match-time budget or a step counter that aborts pathological
  backtracking and reports it, so patterns such as nested quantifiers
  over overlapping alternation cannot hang the process
- document the worst-case behavior and the mechanism that bounds it
- the classic exponential patterns must fail fast and cleanly under the
  budget rather than hang or crash

unicode:
- match over UTF-8 bytes by default, and provide a mode that decodes
  UTF-8 and treats dot, classes, and case as code points
- handle invalid UTF-8 without crashing; document the behavior
- case-insensitive matching for ASCII is required; Unicode case folding
  is optional and must be documented if provided

automated tests:
- a known-answer suite of (pattern, subject, expected spans and capture
  groups) cases covering every required feature, greedy versus lazy,
  alternation precedence, anchors, backreferences, and empty matches
- adversarial backtracking cases that must terminate within the budget
- UTF-8 fixtures including multibyte, combining, and invalid sequences
- a randomized differential mode that compares against a fixed reference
  model over generated patterns and subjects
- a benchmark reporting compile time and match throughput on a fixed
  corpus, and the memory used per compiled pattern
- the suite must build and run clean under AddressSanitizer and
  UndefinedBehaviorSanitizer

no JIT, look-around, possessive quantifiers, atomic groups, or
recursive patterns are required. prioritize correct leftmost-greedy
semantics, bounded backtracking, exact capture spans, and leak-free
value-semantic design over raw throughput.
```

#### Test tasks

| Task | Required behavior |
|---|---|
| Syntax and matching | Compile and match the full required syntax — classes, anchors, alternation, grouping, greedy and lazy quantifiers, and backreferences — with correct leftmost-greedy results. |
| Captures and spans | Report exact byte offsets for every capture group, distinguish non-participating groups, iterate non-overlapping matches without looping on empty matches, and split/replace with group references. |
| Backtracking safety | Terminate the classic exponential patterns within the stated budget with a clean abort, never hanging or crashing, and document the bounding mechanism. |
| Unicode handling | Match UTF-8 correctly in both byte and code-point modes, and handle multibyte, combining, and invalid sequences without crashing. |
| Correctness and hygiene | Pass the known-answer and differential suites, report the benchmark, and run clean under ASan/UBSan with a value-semantic, leak-free, non-copying design. |

### 13. Concurrent KV Store in Go

```text
write a networked in-memory key-value store in Go, in the spirit of a
simplified Redis, that serves many concurrent clients over TCP with
correct shared-state semantics, expiry, eviction, and crash recovery.

do not use an existing in-memory database, cache, or its client/server
libraries. running against a real Redis or memcached is not allowed.
the standard library is allowed, but implement the wire protocol, the
storage engine, and the concurrency control yourself; third-party
packages for argument parsing, logging, and testing are fine.

provide complete source, a go.mod with pinned toolchain, build/run
instructions, and an automated test suite that exercises the
concurrency requirements below.

wire protocol:
- implement a simple text or RESP-style protocol over TCP, documented
  precisely, that a scripted client can drive; support pipelined
  requests and concurrent connections on one port
- return well-defined replies for success, nil/absent, and errors,
  with a documented encoding, and never corrupt interleaved replies
  from concurrent clients

commands (all required):
- GET, SET, DEL, EXISTS, INCR, DECR, APPEND, STRLEN
- MGET / MSET for multi-key access
- EXPIRE, TTL, PERSIST for per-key time-to-live in seconds
- INCR/DECR and APPEND must be atomic per key under concurrency: N
  concurrent increments must produce exactly N added, with no lost
  updates, and APPEND must concatenate without interleaving corruption
- MSET and multi-key reads must behave consistently under concurrent
  writers; document the isolation level chosen and make it observable
- KEYS or SCAN to enumerate keys with a documented, deterministic order

expiry:
- a key with a TTL must become absent at its deadline; reads after
  expiry observe it as gone and TTL reports remaining time correctly
- expiry must be exact under load: implement lazy deletion on access
  plus active background expiry, and an expired key must never
  resurrect through INCR, APPEND, or eviction accounting
- PERSIST must cancel a pending TTL without disturbing other keys

memory and eviction:
- enforce a configurable max-entries or max-bytes limit
- on exceeding the limit, evict by least-recently-used, updating
  recency on reads and writes
- eviction must be exact: pinned hot keys must survive while cold keys
  are evicted, and the eviction count must be reported correctly
- TTL expiry and LRU eviction must not double-count or leak entries

persistence and crash recovery:
- periodically snapshot or append to a log so state survives restart
- after a clean restart, reload the exact prior state
- after a hard kill mid-write, recover a consistent state containing
  exactly the committed prefix of writes; never a torn or duplicated
  entry, and never lose an acknowledged write that the log guarantees
- recovery must reconcile TTLs so expired-at-kill keys stay expired

concurrency control:
- serve many clients concurrently with per-key or sharded locking, not
  one global lock that serializes unrelated keys; document the strategy
- no data races, no deadlocks, and no goroutine or connection leaks
  under connect/disconnect churn and pipelined load
- handle slow and misbehaving clients without stalling other clients
- graceful shutdown must stop accepting, drain in-flight commands,
  flush persistence, and exit cleanly within a documented bound

observability:
- report uptime, connected clients, command counts, keyspace size,
  hit/miss counts, eviction count, and expired-key count
- the suite must run clean under the Go race detector and must not
  leak goroutines (checked with a leak detector after tests)

automated tests:
- a scripted concurrent client harness that fires parallel GET/SET/
  INCR/APPEND load and asserts exact final state and counters
- TTL correctness tests across expiry boundaries under concurrent load
- eviction tests proving exact LRU order and correct counts
- crash tests that kill the process mid-write and assert the committed
  prefix is recovered and expired keys stay expired
- a benchmark reporting throughput and latency percentiles under a
  fixed concurrent workload, plus peak memory

no replication, clustering, pub/sub, Lua scripting, or multiple data
structures are required; strings plus these commands are sufficient.
prioritize correct atomic semantics under concurrency, exact expiry
and eviction, crash-safe persistence, and a clean race-detector run
over raw throughput.
```

#### Test tasks

| Task | Required behavior |
|---|---|
| Protocol and commands | Serve the full command set over the documented protocol to concurrent clients, with correct replies, pipelining, and no corrupted interleaving. |
| Atomic semantics | Execute INCR/DECR/APPEND/MSET atomically per key under parallel load, with exact lost-update-free counts and the documented isolation level. |
| Expiry and eviction | Apply TTL lazily and actively without resurrection, and evict exact least-recently-used entries with correct counts under a memory limit. |
| Crash recovery | Recover the committed prefix after a hard kill with no torn, duplicated, or lost acknowledged writes, and keep expired keys expired. |
| Concurrency hygiene | Run the full suite clean under the race detector with no goroutine or connection leaks, graceful bounded shutdown, and the reported benchmark. |
