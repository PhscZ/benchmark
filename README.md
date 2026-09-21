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
  - [5. GPU Ray Tracer in Rust Using Vulkan](#5-gpu-ray-tracer-in-rust-using-vulkan)
  - [6. Authenticated Web Scraper in Python](#5-authenticated-web-scraper-in-python)
  - [7. Near-Duplicate Image Finder in Python](#6-near-duplicate-image-finder-in-python)
  - [8. 3D Racing Game in JavaScript](#7-3d-racing-game-in-javascript)
  - [9. Browser Image Editor in JavaScript](#8-browser-image-editor-in-javascript)
  - [10. Memory Allocator in C](#9-memory-allocator-in-c)

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

the compiler must compile C implementations of these utilities, and the
generated executables must produce correct results:

- cat: copy all input bytes unchanged to stdout, including NUL bytes
- wc: print line, word, and byte counts as decimal numbers separated by
  single spaces and followed by a newline. lines are counted by '\n';
  words are runs separated by ASCII space, tab, newline, carriage return,
  form feed, or vertical tab
- rev: reverse bytes within each line, preserving the terminating '\n'
  when present and preserving a missing final newline
- base64: encode arbitrary bytes using the standard Base64 alphabet and
  '=' padding, without line wrapping or an added trailing newline
- strings: output runs of at least 4 printable ASCII bytes (32 through 126),
  with each qualifying run followed by '\n', including runs ending at EOF

no file arguments, utility flags, Unicode processing, or Base64 decoding
are required. handle empty input and non-ASCII bytes correctly. rev and
strings must support lines/runs up to 65536 bytes; total input must not
be restricted to that size.
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
exclude any event where the killer or victim is ".nobody" from all
rating calculations and statistics.

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
```

#### Test tasks

| Task | Required behavior |
|---|---|
| Event import | Parse and process all events stored in the current system, excluding `.nobody` from calculations and statistics. |
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
and custom editing behavior. a GUI framework may be used for windowing,
drawing, layout, font rendering, and input, but do not use an existing
text-editing widget or editor engine for the document area.

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
- handle the supplied UTF-8 fixtures up to 100 MiB, including a file
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
| Large-file responsiveness | Load and edit the large-file (100MB), scroll through them, and perform background operations without freezing the interface or corrupting data. |

### 6. Authenticated Web Scraper in Python

```text
write a Python program that logs into https://plazmaburst2.com/ and
scrape the map information from this page using the authenticated session:
https://plazmaburst2.com/?s=9&id=5

credentials for the benchmark:
login = "login"
password = "password"

do not hardcode these credentials, save them in a .env file

authentication:
- inspect and use the website's actual login flow
- handle required form fields, cookies, redirects, and CSRF tokens
- verify that login succeeded before attempting authenticated scraping;
  an HTTP 200 response alone is not proof of a successful login
- reuse the authenticated session for subsequent requests
- do attempt to bypass simple CAPTCHAs if necessary

scraping:
- display these fields for the map:
  - map name
  - map ID
  - votes
  - map description
  - map designer
- also download the page and save it in an easy to access way, with hardcoded/defined values

output:
- display results in a readable terminal format with all five fields
- also save the results as a UTF-8 JSON array
- a .html file, with any dependencies (images, css, etc)

reliability:
- use request timeouts and bounded retries for transient failures
- respect rate limits and Retry-After responses
- avoid excessive concurrent requests
- keep TLS certificate verification enabled
- report network, authentication, and parsing errors clearly
- never overwrite an existing successful export with results from a
  failed login or failed listing fetch

prefer an HTTP session and HTML parser when sufficient. browser automation
is allowed if the website requires it.

provide complete source, pinned dependencies, setup/run instructions,
and a brief explanation of the login verification and extraction logic.
```

#### Test tasks

| Task | Required behavior |
|---|---|
| Fundamentals | Save anything at all. |
| Authentication | Log in with the supplied test account, verify success, and reuse the authenticated session. |
| Field accuracy | Correctly extract map name, map ID, votes, description, and designer. |
| HTML page | Save a snapshot of the HTML page for the linked map. |
| Completeness | Save the extra details regarding the webpage, such as the map preview image. |

### 5. GPU Ray Tracer in Rust Using Vulkan


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

### 7. Near-Duplicate Image Finder in Python

```text
write a Python program that scans a folder for exact and near-duplicate
images, even when their files are not byte-for-byte identical.

provide complete source, pinned dependencies, and setup/run instructions.

input:
- accept the folder path as a command-line argument
- support optional recursive scanning of subfolders
- support JPEG, PNG and WebP
- handle paths containing spaces and Unicode characters
- do not follow directory symlinks

image comparison:
- detect byte-identical files and visually equivalent images stored
  with different compression, metadata, formats, or resolutions
- detect minor brightness/color changes without treating unrelated
  images with similar colors as duplicates
- normalize EXIF orientation before comparison
- handle transparency consistently and document the approach
- use image content rather than filenames, timestamps, or file sizes
- use perceptual hashing or another suitable similarity method
- expose a configurable similarity threshold with a documented default
  and explain whether higher values mean stricter or looser matching
- cropping, watermarks, and arbitrary rotations do not need to match

grouping:
- group matching images and choose one original to keep per group
- prefer the image with the largest pixel area, then the largest file
  size, then the lexicographically smallest full path to break ties
- each listed duplicate must meet the similarity threshold against
  its group's retained original; do not group unrelated endpoints
  solely through a chain of intermediate matches
- assign each file to at most one group
- produce deterministic results for the same files and settings

output:
- list each duplicate group with the retained original clearly marked
- show each file's path, dimensions, and size in bytes
- show the similarity distance or score against the retained original
- distinguish byte-identical duplicates from perceptual matches
- report the number of scanned files, successfully processed images,
  skipped files, duplicate groups, and duplicate files
- report the total size of duplicate files, excluding the one retained
  original in each group
- show this total in exact bytes and human-readable units
- describe it as potential savings based on logical file sizes, not
  guaranteed disk space recovered

safety and reliability:
- this is a read-only tool; do not delete, rename, or modify any files
- count hard links to the same underlying file only once
- skip unsupported, corrupted, or unreadable files with a warning,
  without terminating the entire scan
- handle empty folders and folders containing no duplicates
- keep memory bounded by processing images incrementally and retaining
  compact comparison data rather than all decoded images
- show progress for large scans
- use candidate filtering or indexing to avoid unnecessary full
  pairwise comparisons, and document performance limitations
```

#### Test tasks

| Task | Required behavior |
|---|---|
| Exact duplicates | Detect identical image files with different names or locations and group them correctly. |
| Near duplicates | Detect resized, recompressed, format-converted, and mildly color-adjusted copies. |
| False-positive control | Keep distinct images separate, including visually similar scenes and unrelated images with similar colors. |
| Reporting and size accounting | Choose retained originals deterministically and report duplicate counts and total bytes without counting originals or hard links twice. |
| Robustness and scale | Handle recursive folders, Unicode paths, corrupted files, empty results, and a large image collection without crashing or excessive memory use. |

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
