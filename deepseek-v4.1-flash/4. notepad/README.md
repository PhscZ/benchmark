# Notepad — a desktop text editor in Rust with a custom text buffer

A Windows desktop text editor built on **egui/eframe** for the interface, with the
document buffer, the editing behaviour, and the text rendering all written from
scratch. No editor widget, no editor engine, and no ready-made buffer library is
used for the document.

The design goal is: correct editing and data safety first, then responsiveness on
files up to 100 MiB including a million-line file.

---

## Build and run

### Prerequisites

- **Rust stable**, 1.82 or newer (developed and verified on 1.97.1,
  `x86_64-pc-windows-gnu`).
- A GPU/driver capable of OpenGL (the default `glow` backend).
- Windows 10 or later. The code also builds on Linux/macOS; the atomic-replace
  path has a Windows-specific branch (see *File safety*).

### Commands

```sh
# debug build (recommended while developing)
cargo build

# release build — use this for the large-file fixtures
cargo build --release

# run
cargo run --release

# run with a file open
cargo run --release -- fixtures/large_100mb.txt

# unit + integration tests
cargo test --release
```

The binary is `target/release/notepad.exe`.

### Pinned dependencies

Versions are pinned exactly in `Cargo.toml`; `Cargo.lock` is committed.

| Crate | Version | Role |
|---|---|---|
| `eframe` | `=0.36.2` | Windowing, event loop, OpenGL backend (`glow`), fonts |
| `egui` | `=0.36.2` | Immediate-mode UI, painter, input events |
| `rfd` | `=0.17.2` | Native file open/save dialogs |
| `memchr` | `=2.8.3` | SIMD byte scanning for newline counts and literal search |
| `arboard` | `=3.6.1` | Clipboard read for the Edit ▸ Paste menu item |

`arboard` is already a transitive dependency of `egui-winit`, so depending on it
directly adds no extra build cost. egui only *writes* the clipboard; reading it
requires a direct handle.

> **Note on egui 0.36.** This version is a "Ui-first" API: `eframe::App` requires
> `fn ui(&mut self, ui: &mut Ui, frame: &mut Frame)` (there is no `update`),
> `egui::Panel::top(id)` replaces `TopBottomPanel`, and `egui::MenuBar::new().ui(..)`
> replaces `egui::menu::bar(..)`. The code is written against those signatures.

---

## Test fixtures

The fixtures referenced by the brief were not present in the workspace, so a
generator produces equivalents:

```sh
python tools/make_fixtures.py
```

| File | Size | Contents |
|---|---|---|
| `fixtures/million_lines.txt` | 37.2 MiB | 1,000,000 lines, 20–60 chars each, LF |
| `fixtures/large_100mb.txt` | 100.19 MiB | 1,008,492 lines, ~10% non-ASCII (Cyrillic, Greek, CJK, emoji), 50 lines of ≥200,000 bytes, 5,000 × `SEARCHABLE_TOKEN`, 100 × `Ünïcödé_Töken` |
| `fixtures/small_utf8.txt` | 417 B | Tabs, trailing spaces, CJK, emoji, combining marks, 3 × `TODO` |
| `fixtures/crlf.txt` | 258 B | CRLF throughout, 2 × `TODO` |
| `fixtures/invalid_utf8.bin` | 267 B | Lone continuations, truncated and overlong sequences, `0xFF` |
| `fixtures/empty.txt` | 0 B | Zero bytes |
| `fixtures/bom_utf8.txt` | 13 B | UTF-8 BOM then `Hello BOM\n` |

The generator is deterministic and prints byte sizes, line counts, and token
counts, asserting its own thresholds so drift fails loudly.

---

## Buffer design

### Why a piece table

A piece table keeps the document as a **sequence of views into immutable byte
sources** rather than as one contiguous array:

```text
  original : Arc<Vec<u8>>   the bytes exactly as loaded, never mutated
  chunks   : ChunkStore     append-only source for every inserted byte
  tree     : Treap          pieces in document order, each {src, start, len, nl}
```

An edit is a splice of the piece list plus an append to the added-text source. It
never moves the existing document bytes. A keystroke in a 100 MiB file therefore
costs the same as a keystroke in a 100-byte file — the requirement to *avoid
copying the entire document on every edit* is satisfied structurally, not by
tuning.

The alternative designs were considered and rejected:

- **Gap buffer** — `O(1)` at the gap, but moving the gap is `O(n)`. With multiple
  cursors or a jump to the end of a large file, edits become document-sized
  copies. It also makes a cheap immutable snapshot impossible, which the
  background-job design depends on.
- **Rope** — good asymptotics, but each node typically copies its text on edit,
  and split/merge allocate constantly. A piece table stores *references*, so an
  edit allocates a couple of 16-byte descriptors and no text at all.

### Why the original is sliced into pieces

The loaded file is cut into pieces of at most `CHUNK_CAP` (64 KiB) instead of
being one giant piece. This bound matters: when an edit cuts a piece in half, the
newline counts of the two halves are recovered by **scanning them**, and the bound
keeps that scan at 64 KiB rather than "the whole file". A 100 MiB file loads as
about 1,600 pieces.

### The chunk store

`ChunkStore` is an append-only `Vec<Arc<Vec<u8>>>`. Every byte ever inserted lives
there; edits append, they never rewrite. Two properties follow:

1. **Cloning the store is `O(#chunks)` pointer copies.** A background job takes a
   snapshot of a 100 MiB document for the price of a few thousand `Arc`
   increments.
2. **Copy-on-write is bounded.** `Arc::make_mut` copies a chunk only the first time
   the live buffer appends to a chunk that a snapshot still holds, and that copy is
   at most 64 KiB.

### Indexing strategy

The treap is **implicit**: nodes are addressed by byte position, and each node
caches two subtree aggregates — `sub_len` (bytes) and `sub_nl` (newlines). Both
directions of the line index fall out of that one structure:

| Operation | How | Cost |
|---|---|---|
| `line_of_byte(pos)` | descent accumulating left-subtree newline counts | `O(log n)` |
| `line_start(line)` | descent for the k-th newline, then one bounded scan inside the piece that holds it | `O(log n)` + ≤64 KiB scan |
| byte at `pos` | treap seek, then read | `O(log n)` |
| character step | 1–4 byte read through the piece stream | `O(log n)` |

There is **no separate line-start array** to keep in sync, and **no full-document
rescan per keystroke**. That is the requirement to *avoid full-document rescans for
each keystroke*, met by construction.

Nodes live in a flat `Vec` addressed by `u32` with a `NIL` sentinel, so a node is
32 bytes and the hot paths avoid `Option` branching. The arena is only ever
borrowed one field at a time, so **the buffer contains no `unsafe` code**. Nodes
are recycled through a free list, so a long session does not leak one allocation
per keystroke. Priorities come from a fixed-seed xorshift, which keeps the tree
shape deterministic across runs — useful for reproducible benchmarks and bug
reports.

### Rendering only what is visible

Rendering never touches the document as a whole:

1. `first_line = scroll_y / line_height`; the paint loop runs for
   `ceil(view_h / line_height) + 1` lines and stops.
2. Each visible line's byte span comes from the treap line index in `O(log n)`.
3. A line's bytes are read in one contiguous window of at most 8 KiB, positioned
   by the horizontal scroll offset. **A 200,000-byte line costs the same per frame
   as a 20-byte one**, so long lines do not force full-line work on every frame.
4. Long lines additionally get a checkpoint index every 4,096 bytes (always at
   character boundaries), so locating the byte under a pixel is a bounded walk
   rather than a scan of the whole line. The cache is dropped whenever the
   buffer's generation counter changes.

### Character positioning

Every glyph is assumed to advance by exactly one `char_w`. This is the central
simplifying decision:

- Cursor and selection positions are **exact** and mutually consistent, because
  byte→x and x→byte are inverse scans over the same per-byte advance function.
- No text shaper is needed on the hot path.
- Columns never drift on wide glyphs (CJK, emoji): a wide glyph overhangs its cell
  but still occupies exactly one column.

Full bidirectional layout is explicitly out of scope per the brief. Combining marks
are laid out as their own cells rather than being composed.

### Memory trade-offs

| Component | Cost |
|---|---|
| Original file bytes | 1× file size, resident, shared with every snapshot |
| Inserted bytes | 1× everything ever typed, in 64 KiB chunks |
| Live piece | 16 bytes, plus a 32-byte node while live |
| Piece count | Starts at `file_size / 64 KiB`; grows by at most 2 per edit |
| Snapshot | `O(#chunks)` `Arc` increments + `O(#pieces)` bytes |

The **undo stack is the main unbounded growth vector**: deleted bytes stay resident
as long as an undo record references them. It is therefore capped
(`DEFAULT_HISTORY_BUDGET`, 256 MiB by default); the oldest steps are evicted first,
which only costs the ability to undo that far back.

Document size is capped at `u32::MAX` bytes (4 GiB − 1) because piece offsets are
`u32`. Exceeding it is a clean error at load and at insert, never a silent wrap.

---

## Editing model

### Undo/redo

An undo record is a **piece-level delta**, not a text copy:

```text
  removed  : the pieces that used to occupy [pos, pos + removed_len)
  inserted : the pieces that now occupy     [pos, pos + inserted_len)
```

Because pieces point into immutable sources, undoing a 100 MiB deletion is a
splice of the piece list — no bytes are copied.

**Saved-state tracking** uses monotonic serials. Each edit carries a serial;
`applied` is the serial on top, `saved` is the serial on top at the last
successful write. The document is dirty exactly when `applied != saved`. This makes
*"undoing back to the saved state clears the unsaved-change indicator"* fall out
for free, including save → undo past the save point → redo onto it.

**Grouping.** Consecutive keystrokes coalesce into one undo step while the user
keeps doing the same thing at the same place within 700 ms. A cursor move, a save,
a paste, an undo/redo, or a newline closes the group. Closing on save is what keeps
"type, save, type" as two undo steps, so undoing after a save lands exactly on the
saved state. Backspace and Delete coalesce into their own backward- and
forward-growing runs. Paste, cut, replace, and replace-all are each recorded as a
single step. Any new edit discards the redo branch.

### Motions

Motions and word boundaries are byte-oriented. This is safe for UTF-8 as long as
the predicates treat every byte `>= 0x80` as "not whitespace, not punctuation":
continuation bytes and lead bytes are then classified the same way as the
character they belong to, so a scan can never stop mid-character. Word
segmentation is the usual three-class model (word / whitespace / punctuation).

Home follows the common editor convention: first non-blank, then column 0 on a
second press. Up/Down and PageUp/PageDown preserve the desired column across short
lines.

### Selection

The selection is an anchor plus a caret, so Shift+click, Shift+arrows,
Ctrl+Shift+Left/Right, mouse drag, and double-click-drag (word-at-a-time) are all
the same mechanism with a different way of computing the caret. A plain arrow
collapses a selection to the appropriate edge. Dragging past the viewport edge
scrolls and keeps extending, with the pointer position clamped into the view.

---

## File safety

**Saving never truncates the destination.** The bytes go to a temporary file in the
*same directory*, are flushed, `sync_all`'d, and only then renamed over the target.
A failure at any point before the rename leaves the original untouched; on failure
the temp file is removed. On Windows `fs::rename` refuses to overwrite, so the
replacement falls back to remove-then-rename — the temp file holds the full
contents throughout, so there is no window in which the data exists only in
memory. Symlinks are resolved first so the rename replaces the real file rather
than the link.

**Invalid UTF-8 is rejected, never silently corrupted.** Loading validates the whole
file and reports the exact byte offset and nature of the first bad sequence
("stray continuation byte 0x80", "truncated 3-byte sequence at end of file", and so
on). The document is not opened. This is deliberate: opening lossily and saving
back would rewrite the user's bytes, which is exactly the corruption the brief
prohibits. A UTF-8 BOM is accepted, remembered, and written back on save so
round-tripping does not change the file.

**Errors never lose the document.** Every failure path — unreadable file, invalid
UTF-8, oversized file, failed write — reports the error and leaves the open
document and its undo history untouched.

Line endings are detected on load (LF / CRLF / Mixed) and shown in the status bar;
new lines use the detected style. CR is excluded from line spans, so columns and
rendering never see it.

---

## Threading and large files

The UI thread never performs I/O and never scans a document.

| Operation | Runs on | Locks editing? |
|---|---|---|
| Load | worker | yes, visible in the tab and status bar |
| Save | worker (against a snapshot) | **no** |
| Search | worker (against a snapshot) | **no** |
| Replace-all planning | worker (against a snapshot) | yes |

- **Save does not lock editing.** The worker writes a consistent snapshot, and
  edits made meanwhile leave the tab dirty. The saved serial is recorded, so
  undoing back to it correctly clears the indicator.
- **Search does not lock editing.** It scans a snapshot, so it can never observe a
  half-applied edit. Any edit bumps the document's epoch, which cancels the scan
  and makes the UI **discard the stale result** rather than highlight text that has
  moved. Scans are cancellable from the status bar and the find bar.
- **Progress is shown** in the status bar as a phase label, a progress bar (or a
  spinner when the total is not yet known), and a Cancel button. The app requests
  repaints while any operation is in flight, so unrelated tabs stay responsive.
- **Editing is disabled only while loading or replace-all is running**, and that
  state is visible: the status bar shows the operation, and an attempted edit
  produces an explicit notice saying input was not applied. Input is never
  silently dropped.
- **Replace-all is planned, not executed, on the worker.** The worker returns a
  segment list in which unchanged regions are carried as 16-byte pieces referring
  to immutable sources. Only the replacement text is copied, so replacing across a
  100 MiB document does not copy the document. The plan is materialised into the
  document's own chunk store on the main thread, which keeps undo records valid,
  and lands as **one undo step**.

---

## Find and replace

- Literal search only; regex is not implemented (not required).
- Case-sensitive toggle. Case-insensitive matching folds **ASCII only**; full
  Unicode case folding is explicitly out of scope, and the toggle gives exact
  behaviour either way.
- Matches are validated to begin and end on UTF-8 character boundaries, so a query
  can never match inside a character.
- Next/Previous wrap around; the count is shown as "current of total".
- All matches on visible lines are highlighted; the current match is drawn in a
  distinct colour.
- Replace replaces the current match; Replace All replaces every match as a single
  undoable action.
- **Empty query behaviour is defined and safe**: it produces no matches, no scan,
  and no error. Next/Previous are no-ops, and Replace All *refuses* with an
  explanation rather than inserting the replacement between every character —
  which is what a naive implementation would do.
- Match results are bounded at 1,000,000 entries; hitting the bound is reported.

---

## Interface

- Line-number gutter, right-aligned and vertically aligned with the text rows.
- Status bar: cursor line/column, line count, encoding (`UTF-8`, plus `BOM` when
  present), line-ending style, selection size, unsaved indicator, document size,
  live piece count, and background-operation progress.
- Vertical and horizontal scrolling, with draggable scrollbars and wheel support.
- Monospace font throughout, with exact caret and selection positioning.
- No soft wrapping: long lines scroll horizontally.
- Multiple tabs, each retaining its own buffer, cursor, selection, scroll offset,
  undo/redo history, search state, and unsaved flag. The tab label carries a `*`
  when dirty and a tooltip with the full path.
- Visible menu bar (File / Edit / Search / View / Help) **and** a toolbar of
  buttons, so no main action is menu-only. Help lists every shortcut in-app.

### Shortcuts

| Key | Action |
|---|---|
| `Ctrl+N` / `Ctrl+O` | New / open |
| `Ctrl+S` / `Ctrl+Shift+S` | Save / save as |
| `Ctrl+W` | Close tab (prompts when dirty) |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab |
| `Ctrl+F` / `Ctrl+H` | Find / find and replace |
| `F3` / `Shift+F3` | Next / previous match |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl+A` | Select all |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copy / cut / paste |
| Arrows, `Home`/`End`, `Ctrl+Home`/`Ctrl+End` | Navigation |
| `Ctrl+←` / `Ctrl+→` | Word navigation |
| `PgUp` / `PgDn` | Page up / down |
| `Shift`+navigation, `Shift+click` | Extend selection |
| `Ctrl+Shift+←` / `Ctrl+Shift+→` | Select by word |
| Double-click, then drag | Select word, then extend by words |
| `Backspace` / `Delete` / `Enter` / `Tab` | Editing |

Closing an unsaved tab, or exiting with any unsaved document, prompts with
**Save / Discard / Cancel**. Dismissing the prompt any other way (Escape, clicking
the backdrop) is treated as *Cancel*, never as Discard.

---

## Source layout

```text
src/
  main.rs            binary entry point; CLI file argument
  lib.rs             crate root
  app.rs             frame loop, tabs, menus, dialogs, job results
  buffer/
    mod.rs           piece table: Delta, snapshots, piece stream, line index
    chunk.rs         append-only chunk store
    treap.rs         implicit treap with subtree aggregates
  undo.rs            piece-level edits, grouping, saved-state serials
  document.rs        per-tab state and editing operations
  motion.rs          cursor motions, word boundaries, columns
  search.rs          literal search, replace-all planning, search state
  fileio.rs          validated load, atomic save, BOM handling
  progress.rs        cross-thread progress and cancellation
  ui/
    mod.rs           monospace metrics and palette
    textview.rs      the custom text widget: layout, painting, input
    tabs.rs          tab strip
    searchbar.rs     find / replace bar
    dialogs.rs       confirmation and error modals
    jobs.rs          worker threads and result messages
tests/
  buffer.rs          buffer, undo, search, and file-format integration tests
  render.rs          headless tests for the custom text view: painting, hit
                     testing, and the full keyboard surface
examples/
  largefile.rs       measures load/edit/search/save against the fixtures
tools/
  make_fixtures.py   deterministic fixture generator
```

`buffer`, `undo`, `document`, `motion`, `search`, `fileio`, and `progress` have no
GUI dependency and are directly unit-testable.

### One deliberate exception

The **find and replace text fields** are egui `TextEdit` widgets. The brief
prohibits a ready-made editing widget "for the document area"; a single-line search
box is a UI control, not a document. The document itself is drawn and driven
entirely by `ui::textview`, which implements its own layout, hit-testing,
selection painting, gutter, scrollbars, and input handling.

---

## Measured performance

Produced by `cargo run --release --example largefile`, which drives the library
(no GUI) against the generated fixtures on the development machine
(AMD FX-8300, Windows 10, `--release`).

### `fixtures/large_100mb.txt` — 100.2 MiB, 1,008,493 lines

| Operation | Time | Rate |
|---|---|---|
| Load + UTF-8 validate + build index | 614 ms | 163 MiB/s |
| 2000 random `line_start()` queries | 23 ms | 11.7 µs/query |
| 10,000 `prev_char` steps from the end | 2.0 ms | 0.2 µs/step |
| 1000 keystrokes at the end | 1.7 ms | 1.7 µs/keystroke |
| 1000 `Ctrl+Right` word steps | 2.5 ms | 2.5 µs/step |
| Snapshot (for save or search) | 0.05 ms | 1,604 pieces by pointer |
| Search, 5,000 matches | 75 ms | 1.31 GiB/s |
| Search, case-insensitive, 100 matches | 103 ms | 973 MiB/s |
| Atomic save | 656 ms | 153 MiB/s |

### `fixtures/million_lines.txt` — 37.2 MiB, 1,000,001 lines

| Operation | Time | Rate |
|---|---|---|
| Load + UTF-8 validate + build index | 276 ms | 135 MiB/s |
| 2000 random `line_start()` queries | 46 ms | 23.1 µs/query |
| 1000 keystrokes at the end | 1.7 ms | 1.7 µs/keystroke |
| Snapshot | 0.02 ms | 596 pieces |
| Search | 44 ms | 841 MiB/s |
| Atomic save | 240 ms | 155 MiB/s |

What these numbers establish:

- **A keystroke is ~1.7 µs regardless of file size.** 1.7 µs on the 100 MiB file
  and 1.7 µs on the 1M-line file: the piece table makes edit cost independent of
  document size, and the 1000-keystroke run added exactly 1000 pieces.
- **A snapshot is ~0.05 ms on 100 MiB.** That is the property the whole background
  design rests on — it is cheap enough to take one per Ctrl+S or per keystroke in
  the search box.
- **Search runs at ~1 GiB/s**, so a full scan of the largest fixture is ~75 ms and
  is off the UI thread anyway.
- **Line-index queries are ~12–23 µs**, which is dominated by the bounded scan
  inside the piece that holds the target line, not by the tree descent.
- Resident memory is ~100 MiB for a 100 MiB file: the file bytes are held once, in
  one buffer, and shared with every snapshot rather than copied.

Interactive responsiveness was checked by launching the built binary against
`fixtures/large_100mb.txt` and observing it load, stay responsive
(`Responding = True`), and hold a stable working set across repeated samples.

---

## Verification

```sh
cargo test --release          # 95 tests: 67 buffer/undo/search/file-format
                              # integration tests + 28 headless view tests
cargo run --release --example largefile
cargo run --release -- fixtures/large_100mb.txt
```

The test suite is not scaffolding around the implementation; it targets the
failure modes that a piece table actually has:

- `random_edits_match_a_naive_string_model` drives 4,000 pseudo-random
  insert/delete/replace operations with a mixed alphabet (multi-byte characters,
  CRLF, tabs) and, **after every single operation**, compares the buffer's bytes,
  line count, every line span, and the reverse byte→line mapping against a plain
  `String` model, then runs a full structural check of the tree.
- `Buffer::check_invariants` walks the tree and asserts it is acyclic, that every
  node is reached exactly once, that heap order holds, and that every cached
  subtree aggregate matches a recomputation — then confirms that `flatten()` (used
  by snapshots, and therefore by saving and searching) agrees with the piece
  stream (used by rendering and editing).
- The headless view tests in `tests/render.rs` run the real
  `textview::show` through egui's headless runner, covering tricky content
  (tabs, CRLF, a lone trailing CR, combining marks, emoji, ZWJ sequences, a
  5,000-character line), click-to-caret mapping at every column of several lines,
  the full keyboard surface, and the guarantee that a 200,000-line document
  produces no more paint work than a 50-line one in the same viewport.

To exercise the large-file requirements by hand:

1. Open `fixtures/large_100mb.txt`; the tab appears immediately with a progress bar
   and Cancel button, and the window stays responsive.
2. Scroll to the bottom — the status bar line count reads 1,008,493.
3. Type into the document; the keystroke is instant, and the piece count in the
   status bar grows by a couple of pieces.
4. `Ctrl+S` while continuing to type: the save runs in the background, the tab
   stays dirty, and the file on disk is a consistent snapshot.
5. `Ctrl+F`, search `SEARCHABLE_TOKEN`: the count reaches 5,000, matches on visible
   lines are highlighted, and the current match is distinct. Typing during the scan
   cancels it and clears the stale results.
6. `Ctrl+H`, replace all `SEARCHABLE_TOKEN` with something short, then `Ctrl+Z` —
   the whole replacement is undone in one step.
7. Open `fixtures/invalid_utf8.bin`: it is rejected with the byte offset and nature
   of the first invalid sequence, the tab stays empty, and the application keeps
   running (verified: the process stays alive and responsive on this fixture).

> Visual verification note: the editor was launched and confirmed to run and load
> the 100 MiB fixture, but the environment's screenshot capture returned unrelated
> desktop windows rather than the editor's own surface, so pixel-level visual
> confirmation could not be performed. Rendering correctness is instead covered by
> the headless tests above, which execute the real paint and input paths.

---

## Limitations

- No syntax highlighting, language server, plugins, terminal, or rich-text
  formatting (out of scope per the brief).
- No soft wrapping; long lines scroll horizontally (as specified).
- No bidirectional text layout (as specified).
- Case-insensitive search folds ASCII only.
- Combining marks occupy their own cells rather than composing onto the base
  character.
- Documents are capped at 4 GiB − 1 bytes.
- Undo history is capped at 256 MiB of retained text; older steps are evicted.
