# Glicko-2 kill-rating REST API

A Rust (axum) service that computes [Glicko-2](http://www.glicko.net/glicko/glicko2.pdf)
ratings for a game where events are kills, e.g. *player A killed player B*. It
replays a chronological event log into three independent rating ladders, tracks
per-opponent statistics, resolves alt accounts, and serves everything over HTTP
with an OpenAPI document and Swagger UI.

```
cargo run --release                                   # http://0.0.0.0:8080
cargo run --release -- --import-file history.json     # seed a history at startup
```

- Swagger UI: <http://localhost:8080/swagger-ui>
- OpenAPI JSON: <http://localhost:8080/api-docs/openapi.json>

---

## 1. Model

### Defaults

| Parameter | Value | Flag / env |
|---|---|---|
| rating | `1500` | `--initial-rating` / `GLICKO_INITIAL_RATING` |
| rating deviation (RD) | `350` | `--initial-rd` / `GLICKO_INITIAL_RD` |
| volatility (sigma) | `0.06` | `--initial-volatility` / `GLICKO_INITIAL_VOLATILITY` |
| scale | `173.7178` | `--scale` / `GLICKO_SCALE` |
| convergence tolerance (epsilon) | `1e-6` | `--epsilon` / `GLICKO_EPSILON` |
| tau | `0.5` | `--tau` / `GLICKO_TAU` |

`GET /config` reports the effective values.

### Players and logins

Logins are the identifier everywhere: paths, query filters, responses. Internally
they are interned to dense `u32` handles so the rating loop never hashes a string.

### Events

An event is one kill:

```json
{"id": "e5", "time": "2026-05-16T18:36:04Z", "killer": "nicolas404", "victim": "orinslc", "player_count": 3}
```

- `id` is stable and de-duplicating: re-posting an event is a no-op.
- `time` is RFC 3339 and is the only clock the engine uses.
- `player_count` is the lobby size **at the moment of the kill**, which is what
  decides the category. It is never the lobby size at the start or end of a race.

### Exclusions

Events where the killer or victim is `.nobody` or `.self` are dropped from all
rating calculations and statistics. They remain in the event log and are still
returned by `/events`. Configurable via `--exclude` (comma-separated, empty
string disables it).

### Categories

Three independent ladders per player, each with its own rating, RD, volatility,
inactivity anchor, match counters and statistics:

| Category | Eligible events |
|---|---|
| `total` | every eligible event |
| `1v1` | eligible events with exactly two players in the lobby |
| `pub` | eligible events with more than two players in the lobby |

Every eligible event feeds `total` **and** its applicable category. An event with
fewer than two players is malformed input: it feeds `total` only.

The lobby is dynamic. If a third player joins a 1v1 lobby, subsequent events feed
`pub`; the half-finished `1v1` race keeps its counters and resumes if the lobby
shrinks again. Category counters are never transferred or reset because the lobby
size changed.

### Matches

A match is a race to **20 kills** between two specific players (best of 39). The
player who reaches 20 kills on the opponent wins.

- On a win, **only that pair's** race counters are reset, **in that category
  only**. Counters against other opponents are untouched.
- Cumulative `kills` and `deaths` are tracked separately and never reset.
- A race is evaluated per category, so one event can complete a race in `total`
  while the `1v1`/`pub` race between the same pair is still open.

### Margin-of-victory scoring

Instead of win/draw/loss values, the score fed to Glicko-2 depends on the final
kill counts:

```
winScore  = 0.5 + 0.5 * (winnerScore - loserScore) / winnerScore
lossScore = 1.0 - winScore
```

`winnerScore`/`loserScore` are the players' kill counts for that match. A 20-19
win scores 0.525 (close to a draw); a 20-0 win scores 1.0. Both players' ratings
are updated from their **pre-match** states.

Win/loss **counters** stay integral: a 20-19 win is recorded as one win and one
loss, never as a fraction.

### Inactivity

Applied reactively, before processing an eligible event, for both the killer and
the victim, from event timestamps — never from the wall clock, and never from
calendar-day boundaries. `lastActive` is the timestamp of the player's last
registered eligible event *in that category*.

```
periods  = floor((eventTime - lastActive) / 24h)
phi      = RD / 173.7178
phiNew   = sqrt(phi^2 + periods * sigma^2)
RD       = phiNew * 173.7178
```

- Uses the player's **current** volatility, not the default.
- The first eligible event for a player initialises `lastActive` and applies no
  prior inactivity.
- `lastActive` is set after every eligible event, even when `periods` is zero.
- Fractional days are discarded, not carried over: 23h59m adds nothing, 49h adds
  two periods.
- Only RD changes. Rating and volatility are untouched.
- Never applied in background jobs or while serving reads — only as part of
  processing an event.

### Alt accounts

An alt map (`{"alt": "...", "main": "...", "id": ...}`) collapses accounts onto a
main login. Default endpoints resolve alts; `/noalt/*` endpoints treat every login
as its own player. Chains collapse transitively (`a -> b -> c` puts `a` and `b`
under `c`), self-mappings and cycles are rejected with `400`.

Because the log is the source of truth and the derived state is recomputed from
it, mappings are correct even when applied **after** the events:

- Mapping an account that already has kills merges that history into the main and
  recalculates everything, including re-evaluating whether a race was completed.
- Mapping an account whose main **never appeared in any event** registers that
  main with the alt's full history.
- Removing a mapping restores the split.

Kills between two accounts that resolve to the same player (an alt killing its own
main) are dropped as self-kills.

---

## 2. Build and run

Requires a Rust toolchain (edition 2024; built and tested on 1.97).

```bash
cargo build --release          # binary at target/release/glicko-api
cargo test                     # 48 tests: unit + HTTP integration
cargo run --release            # start on 0.0.0.0:8080, data in ./data
```

Useful flags (all also available as env vars, see `--help`):

| Flag | Default | Meaning |
|---|---|---|
| `--host`, `--port` | `0.0.0.0`, `8080` | bind address |
| `--data-dir` | `data` | holds `events.jsonl` and `alts.json` |
| `--import-file` | – | JSON array of events to seed at startup |
| `--alts-file` | – | JSON array of alt mappings to seed at startup |
| `--exclude` | `.nobody,.self` | logins excluded from everything |
| `--default-page-size` | `50` | page size when `limit` is omitted |
| `--max-page-size` | `1000` | largest accepted `limit` |

State is persisted as an append-only `events.jsonl` (one event per line, same
shape as the import payload) plus `alts.json`. A restart replays the log; there is
no database to run.

---

## 3. API

Base URL `http://localhost:8080`. Every endpoint under `/noalt/` is the
alt-free variant of the same endpoint; everything else collapses alts.

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/health` | liveness, log size, player and match counts |
| `GET` | `/config` | effective rating parameters, exclusions, page limits |
| `POST` | `/events/import` | import an array of events |
| `GET` | `/events` | latest events, newest first; `page`, `limit`, `player` |
| `GET` | `/leaderboard` | full ladder; `category` |
| `GET` | `/leaderboard/paginated` | one page; `page`, `limit`, `category` |
| `GET` | `/players/{login}` | player stats; `category` |
| `GET` | `/players/{login}/matchups` | per-opponent stats; `page`, `limit`, `category` |
| `GET` | `/players/{login}/matchups/{opponent}` | head-to-head; `category` |
| `GET` | `/alts` | alt mappings |
| `POST` | `/alts` | add/replace mappings, then recalculate |
| `DELETE` | `/alts/{alt}` | remove a mapping, then recalculate |

`category` accepts `total` (default), `1v1` or `pub`.

Leaderboards, player info, matchups and head-to-head all report `kills`, `deaths`
and `kd_ratio` (kills per death; a player with no deaths is credited with their
kill count). Leaderboards and player info additionally report `rating`, `rd`,
`wins`, `losses` and `winrate`.

Errors are always `{"error": "..."}` with `400` (bad input), `404` (unknown
player or mapping) or `500`.

### Examples

Import (the sample payload from the specification):

```bash
curl -X POST http://localhost:8080/events/import \
  -H 'content-type: application/json' \
  -d '[{"id": "e5", "time": "2026-05-16T18:36:04Z", "killer": "nicolas404", "victim": "orinslc", "player_count": 3},
       {"id": "e10", "time": "2026-05-16T18:36:25Z", "killer": "nicolas404", "victim": "orinslc", "player_count": 3},
       {"id": "e11", "time": "2026-05-16T18:36:25Z", "killer": "orinslc", "victim": "nicolas404", "player_count": 2}]'
```

```json
{"received":3,"imported":3,"duplicates":0,"total_events":3,"players":2,
 "eligible_events":3,"skipped_excluded":0,"skipped_self":0,"matches":0,"recompute_ms":0}
```

Import from a file (streamed, so a large history never has to fit in memory):

```bash
curl -X POST http://localhost:8080/events/import \
  -H 'content-type: application/json' \
  --data-binary @history.json
```

Or point the server at it directly and skip the HTTP hop entirely:

```bash
./target/release/glicko-api --import-file history.json --alts-file alts.json
```

Leaderboards:

```bash
curl 'http://localhost:8080/leaderboard'                     # full, total ladder
curl 'http://localhost:8080/leaderboard?category=1v1'        # 1v1 ladder
curl 'http://localhost:8080/leaderboard/paginated?page=2&limit=25&category=pub'
curl 'http://localhost:8080/noalt/leaderboard'               # alts not collapsed
```

```json
[{"rank":1,"login":"nicolas404","rating":1512.34,"rd":290.11,"wins":1,"losses":0,
  "winrate":1.0,"kills":20,"deaths":19,"kd_ratio":1.0526}]
```

Players, matchups, head-to-head:

```bash
curl 'http://localhost:8080/players/nicolas404'
curl 'http://localhost:8080/players/nicolas404?category=pub'
curl 'http://localhost:8080/players/nicolas404/matchups?page=1&limit=20'
curl 'http://localhost:8080/players/nicolas404/matchups/orinslc?category=1v1'
```

```json
{"login":"nicolas404","opponent":"orinslc","category":"1v1",
 "kills":20,"deaths":19,"kd_ratio":1.0526}
```

Events, newest first, filtered to one login on either side:

```bash
curl 'http://localhost:8080/events?limit=100'
curl 'http://localhost:8080/events?player=nicolas404&page=2&limit=50'
```

Alt accounts:

```bash
curl -X POST http://localhost:8080/alts \
  -H 'content-type: application/json' \
  -d '{"items": [{"alt": "fck", "id": 57, "main": "deadlyenergy"},
                 {"alt": "orinslc", "id": 56, "main": "orinslc56"}]}'
curl 'http://localhost:8080/alts'
curl -X DELETE http://localhost:8080/alts/fck
```

Adding a mapping returns the same recalculation report as an import
(`players`, `eligible_events`, `matches`, …) so the effect on the ratings is
visible immediately.

---

## 4. Performance

Measured on the development machine (Windows 11, i7-12700), release build, over a
synthetic history of **1,000,000 events / 500 players / 40 alts** (107 MB):

| Operation | Time |
|---|---|
| Cold start: parse + persist 1M events, recompute both views | **1.13 s** |
| Recompute only (in-memory, both alt-aware and no-alt views) | ~0.55 s |
| `GET /leaderboard` (full ladder, 91 KB) | 0.32 ms |
| `GET /leaderboard/paginated` (50 rows) | 0.09 ms |
| `GET /players/{login}` | 0.07 ms |
| `GET /players/{login}/matchups` (50 rows) | 0.11 ms |
| `GET /players/{login}/matchups/{opponent}` | 0.08 ms |
| `GET /events?limit=100` | 0.15 ms |
| `GET /events?player={login}&limit=50` | 0.11 ms |

Median of 30 requests over a keep-alive connection.

How it stays fast:

- **One pass per view.** Ratings, statistics, match counters and leaderboards are
  produced by a single chronological walk over the log. Nothing is recomputed per
  request; reads are lookups into prebuilt structures.
- **Two views in parallel.** The alt-aware and no-alt states are computed on
  separate threads, so `/noalt/*` costs nothing at query time.
- **Interned logins.** `u32` handles throughout the hot loop; no string hashing or
  allocation per event.
- **Streaming import.** The JSON array is deserialized incrementally and appended
  to the log as it is read, so memory stays proportional to the log, not to the
  request body. Events are written in the same pass that interns them.
- **Sorted chronological index.** Appending in time order is detected and avoids a
  re-sort; only out-of-order arrivals trigger one.
- **CSR event index.** Per-player event positions are stored compressed, so the
  filtered event feed is a slice plus a reverse walk rather than a scan of the
  whole log.

---

## 5. Layout

```
src/
  glicko.rs   pure Glicko-2 maths: rating update, RD inflation, margin of victory
  model.rs    events, categories, login interning
  engine.rs   the chronological pass: matches, categories, stats, leaderboards
  store.rs    event log, alt map, persistence, all queries
  api.rs      HTTP handlers, DTOs, pagination
  error.rs    error type and its HTTP mapping
  openapi.rs  router and OpenAPI document
  config.rs   CLI/env configuration
scripts/
  generate_events.py  synthetic history generator
  verify_spec.py      end-to-end verification of the specification
tests/
  http.rs     integration tests driving the real router
```

`verify_spec.py` is an executable checklist of the behaviour described above. Run
it against a server on an **empty** data directory:

```bash
./target/release/glicko-api --data-dir /tmp/glicko-verify --port 8124 &
python scripts/verify_spec.py --base 127.0.0.1:8124
```

Generate a large history to play with:

```bash
python scripts/generate_events.py --events 1000000 --players 500 --out events.json
```
