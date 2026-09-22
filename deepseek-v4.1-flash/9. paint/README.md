# Raster Editor

A browser-based raster image editor: one canvas document, local-only processing,
no uploads, no build step and no runtime dependencies. The editor is written from
scratch against the Canvas 2D API — it does not embed or wrap an existing editor.

![The editor with a brush stroke on a transparent area and an eraser stroke cut through opaque red](docs/screenshot-tools.png)

---

## 1. Setup and run

Requirements: **Node.js 18+** (only for the static file server and the test
runners) and a current desktop Chrome, Firefox or Edge. Python 3 with numpy and
Pillow is needed **only** to regenerate the test fixtures.

```bash
# 1. install the pinned dev dependency (playwright-core, used by the e2e runner)
npm install

# 2. serve the project (ES modules need http(s), not file://)
npm start                 # -> http://127.0.0.1:8080/

# 3. open http://127.0.0.1:8080/ in Chrome, Firefox or Edge
```

Any static file server works; `npm start` runs `tools/serve.mjs`, a ~60-line
static server with no dependencies. Pinned dependency: `playwright-core@1.63.0`
(dev only). There are **no runtime dependencies** — `src/**` imports nothing
outside the project.

### Tests

```bash
npm test                  # unit tests (Node): colour formulas, EXIF, history, crop, transforms
npm run test:e2e          # end-to-end tests in a real browser (Chromium/Chrome/Edge)
npm run test:smoke        # native file chooser -> load path
npm run test:all          # all three

node test/e2e/runner.mjs --headed                 # watch the suite run
node test/e2e/runner.mjs --only="EXIF"            # run matching tests only
node test/e2e/runner.mjs --browser-path="/path/to/chrome"

npm run fixtures          # regenerate test/fixtures (Python 3 + numpy + Pillow)
```

The e2e runner reuses an installed Chrome or Edge (`playwright-core` never
downloads a browser). The suite can also be run manually by opening
`test/e2e/harness.html` — it prints results in the page.

---

## 2. Feature overview

| Area | What is implemented |
| --- | --- |
| Loading | JPEG/PNG/WebP via file picker or drag and drop; EXIF orientation applied exactly once; alpha preserved; dimensions and zoom shown; unsupported/corrupted files reported without losing the open image; warning before replacing an image with unexported changes; up to 4096×4096 |
| Viewport | Zoom in/out, fit-to-window, actual size; panning when the image overflows; checkerboard behind transparency; view changes never modify pixels or history |
| Transform | Drag-crop with 8 handles, freeform and locked ratios, exact numeric crop; resize to exact pixels with optional ratio lock; rotate 90° CW/CCW and 180°; mirror horizontally/vertically; transforms include drawings and update document dimensions |
| Drawing | Freehand brush (size/colour/opacity), eraser (removes to transparency), line, rectangle, ellipse (outline/fill/both), eyedropper, size-accurate cursor ring; one drag = one undo step; brush size in image pixels |
| Adjustments | Brightness and contrast sliders, grayscale, invert; non-compounding preview; apply or cancel; alpha preserved |
| History | Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z plus buttons; undo of transforms, drawings, erasing and adjustments restores pixels *and* dimensions; redo branch discarded on new edit; ≥20 undo steps at 1920×1080 within a documented 256 MiB budget, with visible eviction notice beyond it |
| Export | Full-resolution PNG or JPEG (never a viewport screenshot); PNG keeps transparency; JPEG flattens onto a selectable background (white by default) with a quality slider; filename input; exported dimensions equal document dimensions; no overlays in the output; the export can be reopened for further editing |

Not implemented (explicitly out of scope): layers, text tools, animated images,
RAW files, advanced colour-profile management.

### Keyboard shortcuts

| Shortcut | Action | | Shortcut | Action |
| --- | --- | --- | --- | --- |
| `Ctrl+Z` | Undo | | `B` | Brush |
| `Ctrl+Y`, `Ctrl+Shift+Z` | Redo | | `E` | Eraser |
| `Ctrl+O` | Open image | | `L` | Line |
| `Ctrl+S` | Export | | `R` | Rectangle |
| `Ctrl+0` | Fit to window | | `O` | Ellipse |
| `Ctrl+1` | Actual size (100%) | | `I` | Eyedropper |
| `Ctrl++` / `Ctrl+-` | Zoom in / out | | `C` | Crop |
| `[` / `]` | Brush size −/+ (`Shift` = 10 px) | | `H` | Pan |
| `Space` (hold) | Temporary pan | | `Enter` | Apply crop / apply adjustments |
| `Esc` | Cancel crop / cancel adjustment preview | | `Shift` (hold) | Constrain shapes to square, circle or 45° line |
| Mouse wheel | Zoom at pointer (`Shift`+wheel pans) | | Middle-drag | Pan |

---

## 3. Architecture

```
index.html          markup: toolbar, tool panel, viewport, options, status bar, dialog
styles.css          layout and theming
src/
  main.js           Editor controller: UI wiring, shortcuts, enable/disable rules, busy guards
  config.js         every limit and default (documented in section 5)
  doc.js            RasterDocument: the single raster document (one canvas, RGBA)
  loader.js         file -> RasterDocument, validation and error messages
  exif.js           container parsing, EXIF orientation, orientation stripping
  viewport.js       zoom/pan, coordinate mapping, checkerboard, overlay canvas
  paint.js          brush, eraser, line, rectangle, ellipse rendering
  crop.js           crop rectangle state machine, numeric validation, overlay
  transform.js      rotate / mirror / resize of the whole document
  adjust.js         pure colour operations (unit-tested against numpy)
  adjustments.js    adjustment preview buffer and progress chunking
  history.js        bounded undo/redo stack of document snapshots
  export.js         PNG/JPEG encoding, flattening, download
  busy.js           busy state and progress reporting
  util.js           small helpers (colour parsing, filenames, canvas creation)
tools/serve.mjs     dependency-free static server
test/
  fixtures/         generated fixtures + independent expected values
  unit/             Node unit tests
  e2e/              browser harness (run.js), page (harness.html), runner (runner.mjs)
```

Key design decisions:

* **One document canvas.** `RasterDocument` owns a single canvas holding the
  full-resolution RGBA image. Drawing, transforms and adjustments all read and
  write it; the viewport only ever *draws* it, which is why zooming, panning and
  window resizing cannot change pixels or create history entries.
* **Two viewport canvases.** The image layer (`#viewportCanvas`) and a separate
  interaction layer (`#overlayCanvas`) hold the crop handles, the rule-of-thirds
  grid and the brush cursor ring. Because overlays live outside the document,
  exports can never contain them.
* **Scratch layers for strokes.** A drag renders into a scratch layer at full
  alpha, which is composited onto the document exactly once with the tool
  opacity. A self-overlapping freehand stroke therefore keeps a uniform opacity,
  and the eraser is a true `destination-out` removal to transparency.
* **Snapshot history.** States are full RGBA snapshots, which makes undo of
  dimension-changing operations (crop, resize, rotate) exact. Memory is bounded
  by a byte budget, not a state count — see section 5.
* **Pure colour code.** `src/adjust.js` has no DOM dependency, so the formulas
  run in Node against reference values computed by an independent numpy
  implementation.

---

## 4. Adjustment formulas (reference)

All adjustments are computed from the **base image**, never from the previous
preview, so dragging a slider cannot compound the effect. Alpha is never touched,
which is what preserves transparency.

Pipeline order: **brightness → contrast → grayscale → invert**.

| Step | Range | Neutral | Formula |
| --- | --- | --- | --- |
| Brightness | −100 … +100 (step 1) | 0 | `v' = clamp(round(v + k × 2.55))` |
| Contrast | −100 … +100 (step 1) | 0 | `f = (100 + k) / 100` ; `v' = clamp(round((v − 127.5) × f + 127.5))` |
| Grayscale | on/off | off | `Y = clamp(round(0.2126·R + 0.7152·G + 0.0722·B))` ; `R = G = B = Y` (Rec. 709) |
| Invert | on/off | off | `v' = 255 − v` |

Details needed to reproduce results exactly:

* `clamp` bounds each channel to 0…255; `round` is half-up
  (`Math.round`, i.e. `floor(x + 0.5)`), matching the numpy reference
  (`np.floor(v + 0.5)`).
* Brightness and contrast are one stage: both are applied in floating point and
  **rounded once** at the end of the stage.
* Grayscale rounds once, then assigns the same value to all three channels.
* Invert operates on the already-rounded 8-bit values.

Worked example (brightness `+10`, contrast `0`): `v = 100` →
`100 + 10 × 2.55 = 125.5` → `round → 126`. The fixture case
`brightness+10-halfstep` pins this half-step behaviour.

The reference implementation and its expected outputs live in
`test/fixtures/generate.py` (numpy) and `test/fixtures/color_fixture.expected.json`.
`test/unit/adjust.test.js` asserts the JavaScript implementation against those
values for 13 parameter combinations over an sRGB fixture containing clamping
and rounding boundaries plus alpha values 0, 37 and 128.

Verification commands:

```bash
npm test                                     # formula parity, alpha preservation, chunking
node test/e2e/runner.mjs --only="adjustment"  # same formulas driven through the real UI
```

---

## 5. Limits, budgets and behaviour

| Limit | Value | Notes |
| --- | --- | --- |
| Maximum document size | 4096 × 4096 px | larger images are rejected with an explanation; the open image is kept |
| Minimum document size | 1 × 1 px | resize rejects 0, negative, fractional and non-finite values |
| Maximum source file size | 64 MiB | defensive pre-decode check |
| Zoom range | 5% … 3200% | zoom step ×1.25 |
| Brush size | 1 … 200 image px | measured in image pixels, independent of zoom |
| Tool opacity | 5% … 100% | applied once per drag |
| JPEG quality | 5% … 100% (default 92%) | export only |
| Undo history budget | 256 MiB | see below |
| Undo depth at 1920 × 1080 | ≥ 30 states (≥ 20 undo steps) | one state = 8.3 MiB, so the budget holds 30 |
| Undo depth at 4096 × 4096 | 3 states | one state = 67.1 MiB; oldest states are evicted with a visible notice |

**History budget.** A state is a full-resolution RGBA snapshot, costing
`width × height × 4` bytes (8.3 MiB at 1920×1080, 67.1 MiB at 4096×4096).
`HISTORY_BUDGET_BYTES` is 256 MiB, which satisfies the ≥ 20 undo steps
requirement at 1920×1080 with room to spare. When a commit would exceed the
budget the oldest states are evicted — never below two, so the current content
and one undo step always survive — and the status bar shows a persistent
`History limit reached: …` notice. Current usage is always visible as
`Memory <used> of <budget>` in the status bar. Exporting does not clear history.

**EXIF orientation.** Browsers disagree about when a decoder applies EXIF
orientation, and Chromium applies it for JPEG and PNG even when
`createImageBitmap(..., { imageOrientation: 'none' })` is requested (measured on
Chromium 153; WebP was *not* oriented by the decoder). Rather than rely on
browser-specific behaviour, the loader strips the orientation metadata from the
container (JPEG APP1/Exif, PNG `eXIf`, WebP `EXIF` chunk, with the VP8X flag and
RIFF size kept consistent) and then applies the orientation transform itself.
The result is exactly one rotation on every browser, covered by tests for
orientations 1, 3, 6 and 8 across all three formats.

**Transparency.** PNG export keeps the alpha channel. JPEG cannot store alpha, so
transparency is flattened onto the selected background colour (white by default)
using `source-over` compositing before encoding.

**Memory hygiene.** Scratch buffers (stroke layer, adjustment preview) are
released when a document is replaced, when a preview is cancelled and after a
stroke commits; the brush cursor is drawn on the overlay canvas instead of
allocating per-move canvases; object URLs for downloads are revoked on the next
task; decoded `ImageBitmap`s are closed after use. Repeated load/edit/export
cycles are asserted in the e2e suite to keep history memory inside the budget
and to leave no scratch canvases allocated.

---

## 6. Test fixtures

`test/fixtures/generate.py` writes everything the tests need and, importantly,
computes expected colour values with numpy — an independent implementation of
the documented formulas rather than a recording of the JavaScript output.

| Fixture | Purpose |
| --- | --- |
| `orient{1,3,6,8}.{jpg,png,webp}` | EXIF orientation, including the "not rotated twice" check |
| `alpha.png`, `alpha64.png` | transparency preservation and checkerboard |
| `quadrants.png` | asymmetric four-quadrant image for exact rotate/mirror/crop geometry |
| `solid.png` | opaque canvas for drawing and resize tests |
| `color_fixture.png` + `color_fixture.expected.json` | 13 adjustment cases with reference RGB output |
| `unsupported.bmp`, `corrupt.jpg` | error paths |

Regenerate with `npm run fixtures`. The committed fixtures are the ones the
tests use, so regenerating is only necessary when changing the generator.

---

## 7. Browser support

Verified against Chromium 153, the engine in current desktop Chrome and Edge
(the e2e suite runs in a real browser). Firefox is targeted by the same code
paths and the same standards, but no Firefox build was available in the
development environment, so it has not been executed there; the only
browser-specific behaviour the code compensates for is EXIF handling, which is
neutralised by stripping the metadata instead of trusting the decoder. Required
platform features, all long-standing in current desktop Chrome, Firefox and
Edge: Canvas 2D, `File`/`Blob`, `createImageBitmap`, `ResizeObserver`, Pointer
Events and ES modules.

Deliberate compatibility choices:

* `OffscreenCanvas` is used when available and falls back to a detached
  `<canvas>` element otherwise (`src/util.js`).
* `createImageBitmap` is preferred for decoding, with an `<img>` + object URL
  fallback that revokes the URL immediately.
* No `willReadFrequently`-only paths, no `ImageDecoder`, no WebGL.

---

## 8. Verified behaviour

`npm run test:e2e` drives the real application in a real browser and asserts the
user-visible requirements end to end, including: opening via the file picker,
EXIF orientation in all three formats, transparency, error paths, 4096×4096
acceptance, the discard-changes dialog, coordinate correctness at four
zoom/pan combinations, checkerboard rendering, window resizing, all five
geometric transforms (pixel layout *and* undo), crop by handles and by exact
numbers, resize validation, brush/eraser/shape/eyedropper behaviour, per-drag
opacity, brush-size independence from zoom, adjustment formulas through the UI,
preview non-compounding, undo/redo including redo-branch discarding, ≥20 undo
steps at 1920×1080, history eviction notice, PNG/JPEG export (transparency,
flattening, dimensions, overlays absent, reopen, quality control effect),
partial-opacity erasing, crop validity after undo, busy-state blocking, resource
release across repeated cycles, and panel visibility. `npm run test:smoke`
additionally exercises the real native file chooser.

Current status in this repository: **69 unit assertions and 56 end-to-end tests
pass**, plus the file-chooser smoke check.
