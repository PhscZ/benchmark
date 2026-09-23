# vk-raytracer

A desktop GPU ray tracer written in Rust with Vulkan compute shaders.

The image is produced **entirely on the GPU** by `shaders/raytrace.comp`: ray
generation, analytic ray/sphere and ray/triangle intersection, direct lighting
with hard shadows, iterative mirror reflections, and progressive sample
accumulation. Rasterization is used *only* to present the finished image and the
UI overlay.

No hardware ray tracing is used or required: `VK_KHR_ray_tracing_pipeline` and
`VK_KHR_acceleration_structure` are never enabled. The scene is a fixed analytic
description (3 spheres + 10 triangles), intersected by brute force in the compute
shader.

```
cargo run --release                 # window mode, interactive accumulation
cargo run --release -- --benchmark  # fixed-camera benchmark + PNG + JSON report
cargo run --release -- --self-test --no-window   # GPU verification suite
```

---

## 1. Requirements

### Host

| Item | Requirement |
|---|---|
| OS | Windows x64 (the code also builds for Linux; the windowing layer is `winit`) |
| Rust | 1.87 or newer (edition 2021). No C/C++ toolchain and **no Vulkan SDK are needed to build** - shaders are compiled to SPIR-V by `naga`, a pure-Rust shader compiler, in `build.rs` |
| GPU driver | A Vulkan 1.2 (or newer) driver. Developed and verified on an **AMD Radeon RX 5700 XT**, driver `26.9.1` (Adrenalin), Vulkan `1.4.315`, `vulkan-1.dll` from the driver |
| Vulkan loader | `vulkan-1.dll` (installed by the GPU driver). The Vulkan SDK is only needed if you want validation layers |

### Device capabilities the renderer validates at startup

The renderer queries these and **fails with an explicit message** if any is
missing; it never silently falls back to CPU rendering.

| Capability | Requirement | Why |
|---|---|---|
| API version | >= 1.2 (instance + device) | `vkResetQueryPool`-free path uses 1.0/1.1 features, but 1.2 is requested and checked |
| Queue family | one family with `GRAPHICS \| COMPUTE` (used for compute, presentation and transfers) | single-queue synchronization model |
| `R32G32B32A32_SFLOAT` | `STORAGE_IMAGE \| SAMPLED_IMAGE \| TRANSFER_SRC` (optimal tiling) | the accumulation image (`imageLoad`/`imageStore`) |
| `R8_UNORM` | `SAMPLED_IMAGE` | bitmap-font atlas |
| `maxComputeWorkGroupInvocations` | >= 64 | 8x8 compute work groups |
| `maxComputeWorkGroupSize` | >= (8, 8, 1) | same |
| `maxImageDimension2D` | >= 4096 | accumulation/present resolution |
| `maxDescriptorSetStorageBuffers` | >= 4 (per stage too) | sphere/triangle/test buffers |
| `maxDescriptorSetStorageImages` | >= 1 | accumulation image |
| `maxPushConstantsSize` | >= 16 bytes | compute + UI push constants |
| `maxStorageBufferRange` | >= 4096 bytes | scene buffers |
| Surface usage | `COLOR_ATTACHMENT` | presentation |
| Queue `timestampValidBits` > 0 and `timestampPeriod` > 0 | *optional* | GPU timestamp queries; if absent the benchmark reports wall-clock time only and says so |

Validation layers (`VK_LAYER_KHRONOS_validation`, e.g. from the Vulkan SDK) are
optional; enable them with `--validation`. If they are requested but not
installed, the program reports that clearly and exits.

---

## 2. Build

```
cargo build --release
```

* Dependencies are pinned in `Cargo.toml` (`ash 0.38`, `ash-window 0.13`,
  `raw-window-handle 0.6`, `winit 0.30`, `glam 0.30`, `bytemuck 1`, `png 0.17`,
  `font8x8 0.3`, `serde 1`, `serde_json 1`, `sha2 0.10`, build-dependency
  `naga 30`). `Cargo.lock` is committed for reproducible builds.
* **Shaders are compiled as part of the build**: `build.rs` walks `shaders/`,
  expands `#include` directives, compiles each `.comp`/`.vert`/`.frag` to SPIR-V
  with `naga`, and emits `include_bytes!` constants. Compiling the shaders needs
  no external tool.
* `naga`'s GLSL front end expects the separate `texture2D`/`sampler` form (the
  same model `wgpu` uses), so the fragment shaders declare an image and a sampler
  as two descriptors rather than one combined sampler.

## 3. Running

### Window mode

```
cargo run --release -- [OPTIONS]
```

| Option | Meaning |
|---|---|
| `--validation` | enable the Vulkan validation layers |
| `--gpu <substring>` | pick a physical device whose name contains the substring |
| `--samples <1\|16\|64>` | initial samples-per-pixel preset (default 64) |
| `--reflections <0..4>` | initial reflection bounce limit (default 4) |
| `--exposure <float>` | initial exposure (default 1.0) |
| `--seed <u32>` | sampling seed (default 12345) |
| `--width/--height <px>` | window/render resolution (default 1280x720) |
| `--output-dir <path>` | directory for PNG exports and reports (default `renders/`) |
| `--benchmark` | run the fixed benchmark, save PNG + JSON report, exit |
| `--self-test` | run the GPU verification suite, exit (exit code 2 on failure) |
| both | run the self-test, then the benchmark, then exit |
| `--no-window` | no window (requires `--benchmark` and/or `--self-test`) |
| `-h`, `--help` | usage |

### Controls

| Input | Action | Limits |
|---|---|---|
| left-drag | orbit the camera | yaw unlimited, **pitch limited to +-89 degrees** (keeps the up vector well conditioned), 0.005 rad per pixel |
| mouse wheel | zoom (orbit distance) | **distance limited to 1.5 .. 40 world units**, x0.9 per notch (zoom in) |
| `1` / `2` / `3` | samples-per-pixel preset 1 / 16 / 64 | resets accumulation |
| `R` / `T` | reflection bounces -/+ | 0 .. 4, resets accumulation |
| `E` / `D` | exposure -/+ | x1.25 per step, clamped to 0.01 .. 100; **reuses accumulated samples** |
| `Space` | start / restart rendering | clears the accumulation image |
| `C` | reset camera to the default view | resets accumulation |
| `P` | save PNG at render resolution (no UI overlay) | reports partial vs. complete |
| `X` | pause / resume accumulation | |
| `B` | run the benchmark (stays in the app; only the `--benchmark` command-line flag exits) | |
| `Esc` | quit | |

The window shows the rendered image, accumulated sample count, resolution, GPU
name, driver version, elapsed render time, GPU time (from timestamp queries when
available) and the last batch time. The reported driver version comes from the
`VK_KHR_driver_properties` `driverInfo` string, because the packed
`driverVersion` integer uses a vendor-specific layout (AMD's packed value for
this driver decodes to `2.0.353` under the layout the spec describes for the
common case, while the vendor string correctly says `26.9.1`); the report records
`driver_version_source` and keeps the raw packed integer and its hex form as
evidence. Every control is also a clickable button.

`--self-test` and `--benchmark` are verification commands: given on the command
line they report and then **exit** (also in window mode, where the window is
still created because the resource-lifetime check exercises the real swapchain).
The `B` key runs the same benchmark interactively and returns to the render loop
instead of exiting.

Camera defaults: position `(0, 3, 10)`, target `(0, 2.5, 0)`, up `(0, 1, 0)`,
45 degree vertical field of view, 1280x720.

### Benchmark mode

`--benchmark` renders the default camera at **1280x720, 64 spp, 4 reflection
bounces, exposure 1.0, seed 12345**:

1. one unmeasured warm-up render (in window mode this is the interactive loop, so
   progress is visible),
2. accumulation cleared,
3. measured render with identical settings, split into 8 bounded dispatches
   (8 spp each) so that no single submission risks a Windows GPU timeout (TDR);
   the system timeout never has to be disabled or raised.

Reported timings (see `BenchReport`):

* `initialization_seconds` - instance, device and resource creation,
* `pipeline_creation_seconds` - shader modules + pipelines, measured separately,
* `measured.gpu_seconds` - sum of Vulkan **timestamp query** deltas around each
  compute dispatch (GPU execution time, not CPU submission time),
* `measured.wall_seconds` - wall clock from the first submission to the last
  fence wait, i.e. including waiting for GPU completion,
* `measured_primary_rays_per_second` - derived from the GPU time.

The measured phase does not present between dispatches, so wall-clock time is not
inflated by the presentation engine. PNG encoding and disk writes happen *after*
all timings are captured and are excluded from them.

**Reading the GPU time.** `measured.gpu_seconds` is the sum of the timestamp
query deltas around the compute dispatches, but a timestamp interval spans
whatever the GPU is doing in between, so in window mode it also absorbs
contention with desktop composition on the same GPU. Measured on the RX 5700 XT
with identical compute work:

| Mode | Measured GPU time (3 runs) |
|---|---|
| `--no-window` | 13.2 ms, 13.2 ms, 13.2 ms (stable) |
| windowed | 22.0 ms, 28.2 ms, 22.2 ms (higher and variable) |

The headless run is the uncontended GPU execution time; the windowed run is what
the GPU actually spends wall-clock time on while a desktop is composited. The
report always contains this caveat as a note, so the number is never presented
without its context.

Outputs: `renders/render_<UTC timestamp>_s64.png` and
`renders/benchmark_report_<UTC timestamp>.json` (GPU, driver, settings, completed
sample count, timings, PNG sha256). PNG export uses the same colour processing as
the display path.

### Exit codes

`0` success, `1` error/unsupported capability, `2` self-test failure,
`3` benchmark did not complete.

---

## 4. The fixed scene

Right-handed coordinates, +Y up, +Z toward the viewer. The room is open at
`Z = +3`.

| Primitive | Geometry | Base colour (linear RGB) | kd | ks | shininess | reflectivity |
|---|---|---|---|---|---|---|
| floor, ceiling, back wall | 2 triangles each (10 triangles total) | (0.75, 0.75, 0.75) | 1.0 | 0.0 | 1 | 0.0 |
| left wall `X = -3` | 2 triangles | (0.65, 0.05, 0.05) | 1.0 | 0.0 | 1 | 0.0 |
| right wall `X = +3` | 2 triangles | (0.05, 0.65, 0.05) | 1.0 | 0.0 | 1 | 0.0 |
| matte blue sphere, c = (-1.6, 1.0, -0.8), r = 1.0 | analytic | (0.05, 0.15, 0.8) | 1.0 | 0.0 | 1 | 0.0 |
| glossy gold sphere, c = (1.4, 1.0, -1.0), r = 1.0 | analytic | (0.8, 0.5, 0.1) | 0.8 | 0.5 | 64 | 0.15 |
| mirror sphere, c = (0.0, 0.75, 1.0), r = 0.75 | analytic | (1.0, 1.0, 1.0) | 0.0 | 0.0 | 1 | 1.0 |

Room extents: `X` in [-3, 3], `Y` in [0, 6], `Z` in [-3, 3]. One white point
light at `(0.0, 5.5, 1.0)` with RGB intensity `(60, 60, 60)`. The background
outside the room is black. No external models, textures or downloaded assets.

Sphere index 2 (the mirror) is tangent to the floor at `(0, 0, 1)`; the scene is
deliberately constructed to contain shared triangle edges and a sphere/floor
tangency, which the test suite uses to exercise tie-breaking.

---

## 5. Materials, lighting and colour

For a hit with oriented unit normal `N`, light direction `L`, view direction
`V = -rayDir`, half vector `H = normalize(L + V)`:

```
ambient   = 0.02 * baseColor
atten     = lightIntensity / distance(P, light)^2
diffuse   = baseColor * kd * max(dot(N, L), 0)
specular  = white * ks * pow(max(dot(N, H), 0), shininess)
local     = ambient + visibility * atten * (diffuse + specular)
final     = (1 - reflectivity) * local + reflectivity * reflectedColor
```

* Direct lighting is exactly zero when `dot(N, L) <= 0` (only ambient remains).
* A zero-length half vector yields zero specular (`|L + V| <= 1e-6`).
* Shadows are hard shadow rays with `tmax = distanceToLight - 1e-3`, so geometry
  behind the light cannot cast a shadow. Ambient stays present in shadow.
* All colour arithmetic is component-wise in linear RGB, in 32-bit floats.

Display/export transform (identical on GPU and CPU, `src/color.rs` mirrors
`shaders/present.frag`): average the accumulated samples, multiply by exposure,
Reinhard tone map `c / (1 + c)` component-wise, then convert to sRGB exactly
once. Exposure changes reuse the accumulated linear samples.

## 6. Sampling and accumulation

* One invocation per pixel, one pixel per invocation: no write races, no atomics.
* `1 spp` uses exact pixel-centre sampling; `16`/`64 spp` use deterministic
  stratified jitter: the sample index selects a stratum of a
  `ceil(sqrt(spp)) x ceil(sqrt(spp))` grid, and the offset inside the stratum is
  hashed from `(pixel.x, pixel.y, sample index, seed)` with an integer avalanche
  hash. The same settings and seed reproduce identical decoded pixels on the same
  build/GPU/driver (verified by the self-test, which hashes the decoded RGBA8
  pixels).
* Accumulation stops when the selected sample count is reached; the sample count
  is independent of the display frame rate (each displayed frame adds one sample
  per pixel; the counter is driven by dispatched samples, not by time).
* Accumulation resets on camera change, render-resolution change, sample-preset
  change and reflection-limit change; exposure changes do not reset it.
* Image row 0 is the top of the frame; the vertical camera coordinate is flipped
  accordingly, and the perspective rays use the render aspect ratio.

## 7. Vulkan implementation notes

* **Compute pipeline** (`shaders/raytrace.comp`): `local_size = 8x8`, one
  invocation per pixel, bounds-checked against `imageSize(accum_image)` so
  invocations past the right/bottom edge do nothing.
* **Descriptors** (set 0): storage image (accumulation), storage buffers
  (spheres, triangles, test rays, test results), uniform buffer (camera basis,
  light, primitive counts, seed, reflection limit). One layout serves both
  compute pipelines; the self-test ray/result buffers are always bound (they are
  tiny) so no pipeline-specific set juggling is needed. The present and UI passes
  use a second layout with a sampled image + sampler.
* **Image layouts and synchronization**: the accumulation image lives in
  `GENERAL` (storage image), transitions to `SHADER_READ_ONLY_OPTIMAL` for the
  present pass, to `TRANSFER_SRC_OPTIMAL` for readback, and to
  `TRANSFER_DST_OPTIMAL` when cleared. Every transition is paired with precise
  `srcStageMask`/`srcAccessMask`/`dstStageMask`/`dstAccessMask` matching the
  producers/consumers (compute writes, fragment reads, transfer reads).
* **Frame synchronization**: the render loop is deliberately synchronous. Each
  frame waits on a *guard fence* before recording. The guard fence is signalled
  by an empty command buffer submitted **after** `vkQueuePresentKHR`, so waiting
  on it proves that both the previous submission and the previous present
  operation (i.e. the `render_finished` semaphore wait) have completed. That is
  what makes per-swapchain-image semaphore reuse safe without
  `VK_KHR_present_wait`. Acquire semaphores are per frame slot, `render_finished`
  semaphores are per swapchain image.
* **Swapchain recreation**: on `VK_ERROR_OUT_OF_DATE_KHR`,
  `VK_SUBOPTIMAL_KHR`, resize or format change, the device is idled, the old
  swapchain is destroyed after the new one is created, the accumulation image and
  readback buffer are recreated when the size changed, descriptor sets are
  updated, and accumulation restarts. A zero-sized window (minimized) skips
  rendering and presentation entirely. Repeated resizing/restarting does not leak:
  every resource is destroyed exactly once (`Drop for Renderer` + explicit
  destroy paths).
* **Bounded dispatches**: interactive mode dispatches 1 spp per frame; the
  benchmark splits 64 spp into 8 dispatches of 8 spp. No submission is long
  enough to approach the 2 s Windows TDR limit.
* **Presentation**: a fullscreen triangle with a fragment shader that averages,
  exposes, tone maps and (for `UNORM` swapchains) encodes sRGB; the sRGB
  swapchain format does the encode in hardware. The UI is drawn as textured,
  alpha-blended quads from a bitmap-font atlas.
* **Timestamps**: `vkCmdWriteTimestamp` around each dispatch into a 128-query
  pool; results are read after the owning fence has signalled (never while the
  queries are in flight). The query pair is reset on the host
  (`vkResetQueryPool`, Vulkan 1.2) rather than inside the command buffer: an
  in-buffer reset introduces a command-processor sync point that measurably
  inflates the timestamped interval.
* **Resource accounting**: every Vulkan object whose lifetime this renderer
  manages is counted on create/destroy (`crate::vk::ResourceCounts`), which turns
  the no-leak requirement into an assertion the self-test performs rather than a
  claim.
* Unsupported capabilities are reported explicitly (`Error::Unsupported`) and the
  program exits; there is no CPU rendering fallback.

## 8. Verification

### `cargo test` (host unit tests)

Camera defaults/limits and aspect handling, colour processing (Reinhard bounds,
sRGB endpoints, average-before-exposure ordering), UTC timestamp formatting, and
the reference intersection semantics: inside-sphere exit points, tangent rays,
shadow rays stopping at the light, and the deterministic tie-break (identical
spheres, identical triangles, sphere-vs-triangle) with bit-exact ties.

### `--self-test` (GPU, results read back to the CPU)

1. **Intersection tests on the GPU**: 94 known rays are dispatched through
   `shaders/intersect_test.comp`, which `#include`s the *same*
   `shaders/lib/common.glsl` intersection and shading code as the production
   shader. Results (t, hit flag, primitive kind/index, hit position, oriented
   normal, shaded/path colour, any-hit) are read back and compared against the
   independent CPU implementation in `src/reference.rs`. The table covers:
   sphere front hits, rays starting inside a sphere, tangent rays, misses,
   shared-edge hits (floor diagonal), the sphere/floor tangency point,
   two-sided triangles from below, `t_min`/`t_max` rejection, rays parallel to
   the floor/ceiling, shadow rays (clear and occluded), a shadowed and a lit
   surface sample, mirror/gold/blue reflection paths, the zero-length
   half-vector guard plus a control that proves specular is visible when the half
   vector is short but non-zero, and a 5x5 grid of camera rays in intersection,
   shading and path mode.
2. **Determinism of the tie-break**: the same ray table is dispatched twice and
   every result must be bit-identical.
3. **Per-pixel render check**: a 1 spp render is read back and compared against
   the CPU reference traced with the identical camera ray, at 13 probe pixels
   (including 4 that hit spheres).
4. **Reproducibility**: the same settings and seed rendered twice must produce
   identical decoded RGBA8 pixels (sha256); accumulation must stop at the
   selected sample count; changing exposure must reuse the accumulated samples.
5. **Resource lifetime**: 12 rounds of render-resolution changes plus render
   restarts (and, in window mode, 12 real swapchain recreations driven through
   `resize_and_present`), followed by a second pass, must leave every live Vulkan
   object count exactly at its baseline. This check found and fixed two real
   bookkeeping omissions while it was being written, which is the point of having
   it.

Example (RX 5700 XT, Vulkan 1.4.315, window mode):

```
self-test: 9 checks, 0 failed        (window mode, ~3.7 s total, exits 0)
  [PASS] repeated intersection dispatch is bit-identical: 94 rays dispatched twice with identical results
  [PASS] GPU intersection tests match the CPU reference: 94 rays verified (75 camera grid rays)
  [PASS] ray case coverage: Coverage { hits: 59, sphere_hits: 12, triangle_hits: 47, misses: 33, shadowed_shade: 2, lit_shade: 15, reflective_path: 2, any_hit_checks: 55 }
  [PASS] 1 spp render matches the CPU reference per pixel: 13 probe pixels (4 on spheres), worst |gpu-cpu| = 1.5e-05
  [PASS] same settings + seed reproduce identical decoded pixels: 1280x720, 16/16 samples, sha256 c1001c6a... vs c1001c6a...
  [PASS] accumulation stops at the selected sample count: status Complete, accumulated 16 of 16
  [PASS] exposure change reuses accumulated samples: 921600 pixels compared at exposure 2.0, 16 samples kept
  [PASS] repeated resolution changes and restarts do not leak GPU resources: 12 rounds over 4 resolutions (swapchain recreation + accumulation image recreation); live objects 53 -> 53 (peak 53)
  [PASS] second resize pass is also leak-free: live objects steady at 53
```

### Other verification performed

* **Cross-path determinism**: the benchmark PNG exported from window mode and the
  one exported headless (same settings/seed) decode to the same sha256
  (`d91413c7...`), so presentation, swapchain format and window size do not
  perturb the image.
* **Reflection limit affects the image**: at 1280x720/16 spp, a 0-bounce and a
  4-bounce render differ on 28,578 of 921,600 pixels, all inside
  x 568..829, y 395..605 - the mirror sphere's silhouette. Bounce 0 is therefore
  genuinely "no reflection" and the difference is confined to reflective
  geometry.
* **Interactive controls** (driven with synthetic window input on Windows):
  orbit (drag) changes the camera position, wheel zoom clamps at exactly 1.5 and
  40.0 units, pitch clamps at +-89 degrees, `1`/`2`/`3` switch the spp preset
  (and reset accumulation), `C` restores `cam (0.00, 3.00, 10.00) d=10.01`, `P`
  writes a PNG at render resolution with no UI overlay, `Esc` exits with code 0.
* **Error paths**: `--validation` without the layer installed, `--gpu` matching
  nothing, and an invalid `--samples` value each exit 1 with a specific message
  (never a silent CPU fallback).
* **Command-line termination**: `--self-test`, `--benchmark`, and the two
  combined each terminate on their own (exit 0; exit 2 on self-test failure,
  exit 3 on an incomplete benchmark) in both window and headless mode, while
  pressing `B` interactively runs the benchmark and returns to the render loop.

---

## 9. Source layout

```
build.rs                  compiles shaders/ -> SPIR-V (naga) with a #include preprocessor
Cargo.toml, Cargo.lock    pinned dependencies
shaders/
  lib/common.glsl         scene structs, intersection, shading, path tracing (shared)
  raytrace.comp           production compute path tracer + accumulation
  intersect_test.comp     GPU intersection self-test (includes lib/common.glsl)
  present.vert/.frag      fullscreen present pass (average/exposure/tone map/sRGB)
  ui.vert/.frag           UI overlay (bitmap font atlas)
src/
  main.rs                 CLI parsing
  lib.rs                  module tree + compiled-shader constants
  app.rs                  winit window, input, on-screen UI, render loop
  render.rs               renderer: accumulation image, dispatch, present, readback, timestamps
  vk/mod.rs               instance/device selection + capability validation
  vk/resources.rs         buffers, images, samplers, layout barriers
  vk/pipelines.rs         shader modules, descriptor layouts/sets, pipelines, render pass
  vk/swapchain.rs         swapchain, framebuffers, per-image semaphores
  scene.rs                the fixed scene (GPU layout)
  camera.rs               orbit camera + ray-generation basis
  color.rs                average/exposure/Reinhard/sRGB (mirrors the present shader)
  reference.rs            CPU reference implementation (test oracle)
  self_test.rs            GPU verification suite (incl. resource-leak checks)
  bench.rs                benchmark mode + JSON report
  ui.rs                   bitmap-font atlas + immediate-mode UI
  util.rs                 UTC timestamps / duration formatting
```

## 10. Troubleshooting

* **"no suitable Vulkan device found"** - the message lists every enumerated
  device and what it lacked (queue families, formats, limits). Update the GPU
  driver.
* **"Vulkan 1.2 is required, but the installed loader only reports ..."** - the
  loader is older than 1.2; install a current driver.
* **`--validation` fails** - `VK_LAYER_KHRONOS_validation` is not installed.
  Install the Vulkan SDK (or the standalone validation-layer package) or drop the
  flag. Layer warnings/errors are printed to stderr.
* **Timestamps unavailable** - the benchmark prints a note and reports wall-clock
  time only; no fake GPU numbers are produced.
* **Very slow first frame** - the first dispatch includes pipeline/shader warm-up
  on the driver; the benchmark performs a warm-up render for exactly this reason.
