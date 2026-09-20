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

The compiler must successfully compile programs implementing the following utilities. The resulting executables must run and produce correct output.

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

The API must successfully process the existing event history and return correct data.

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

### 5. Authenticated Web Scraper in Python

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
