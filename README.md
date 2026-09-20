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
