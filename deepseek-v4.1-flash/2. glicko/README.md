# Glicko-2 rating REST API

A Rust service that folds a chronological stream of kill events (`A kills B`) into
Glicko-2 ratings, match statistics and pairwise match counters, and serves them
over HTTP.

Players are identified by **login** everywhere: in the event log, in the URL
paths, and in every response. There are no numeric player ids on the wire.

---

## Build & run

Requires a stable Rust toolchain (built and verified with `rustc 1.97.0`).

```bash
cargo build --release          # binary at target/release/glicko-api
cargo test                     # 30 unit tests, including the published Glicko-2 reference vector

# start empty
./target/release/glicko-api --bind 127.0.0.1:8080

# or preload an event dump before serving
./target/release/glicko-api --bind 127.0.0.1:8080 --events history.json
```

| Flag | Env | Default | Meaning |
| --- | --- | --- | --- |
| `--bind <ADDR>` | `GLICKO_BIND` | `127.0.0.1:8080` | Listen address |
| `--events <FILE>` | `GLICKO_EVENTS` | – | JSON dump to import at startup |
| `-h`, `--help` | – | – | Usage |

State is in-memory only: the event log is the source of truth, so a restart plus a
re-import reproduces every rating exactly.

---

## Tuning constants

All defaults from the specification, in `src/model.rs`:

| Constant | Value |
| --- | --- |
| Rating | `1500.0` |
| Rating deviation (RD) | `350.0` |
| Volatility (sigma) | `0.06` |
| Scale | `173.7178` |
| Convergence tolerance (epsilon) | `1e-6` |
| tau | `0.5` |
| Match length | race to 20 kills (best of 39) |
| Excluded login | `.nobody` |

---

## How the specification maps onto the code

| Requirement | Where |
| --- | --- |
| Glicko-2 update, volatility solver | `src/glicko.rs` |
| Constants, categories, `CatStats`, `PairState` | `src/model.rs` |
| Event fold: counters, match resets, inactivity, MoV | `src/engine.rs` |
| Event log, idempotent import, replay, leaderboard cache | `src/store.rs` |
| Routes, query parsing, DTOs | `src/api.rs` |

### Eligibility

An event is **eligible** when neither the killer nor the victim is `.nobody`
(suicides, world kills). Ineligible events are stored and served by
`GET /api/v1/events` with `"eligible": false`, but they never touch ratings,
match counters, pair counters, `lastActive`, or any statistic. `.nobody` never
appears on a leaderboard and is not a player.

### Three independent rating systems

| Category | Fed by |
| --- | --- |
| `total` | every eligible event |
| `1v1` | eligible events whose recorded `player_count` is exactly 2 |
| `pub` | eligible events whose recorded `player_count` is 3 or more |

Each eligible event contributes to `total` **and** its applicable category
(`1v1` or `pub`). Classification uses the `player_count` recorded on the event
itself, never the lobby size at the start or end of a race. An event with a
recorded `player_count` below 2 only contributes to `total`.

The lobby is dynamic. If a third player joins a duel, later events feed `pub`
while the `1v1` counters sit untouched; if the lobby shrinks back to two, the
`1v1` race resumes from its stored counter. Nothing is transferred or reset
because the lobby size changed. Ratings, statistics and pair counters are stored
per category, so all three systems evolve independently.

### Matches

A match is a race to 20 kills between two specific players. When a player
registers their 20th kill against that opponent in a given category:

* `wins` / `losses` are incremented (as whole match outcomes, never fractional);
* a Glicko-2 update runs for both players in that category;
* **only that pair's** match counters in **that category** are reset.

Counters against other opponents, and counters for the same opponent in the other
categories, are untouched. Cumulative `kills` and `deaths` are lifetime counters
and never reset when a match ends.

### Margin-of-victory scoring

Standard win/draw/loss values are replaced by the custom score:

```
winScore  = 0.5 + 0.5 * (winnerScore - loserScore) / winnerScore
lossScore = 1.0 - winScore
```

`winnerScore` / `loserScore` are the final kill counts of the race, so a 20-0 win
scores `1.0` / `0.0`, a 20-10 win scores `0.75` / `0.25`, and a 20-19 win scores
`0.525` / `0.475` — close to a coin flip. Both players are scored against the
opponent's pre-match rating, and the winner's gain mirrors the loser's loss.

### Inactivity RD growth

Applied **reactively, before processing an eligible event**, for both the killer
and the victim, using event timestamps — never the wall clock, and never
calendar-day boundaries. In `CatStats::touch`:

```
periods = floor((eventTime - lastActive) / 24h)
phi     = RD / 173.7178
phiNew  = sqrt(phi * phi + periods * sigma * sigma)
RD      = phiNew * 173.7178
```

* Uses the player's **current** volatility, not the default.
* `lastActive` is initialised on the player's first eligible event in that
  category, so no prior inactivity is applied.
* After every eligible event `lastActive` is set to that event's timestamp, even
  when `periods` is 0. Fractional days are discarded, never carried over: a 23h59m
  gap adds nothing, a 49h gap adds exactly two periods.
* Only RD changes. Rating and volatility are untouched.
* Never applied in background jobs or when serving reads — only inline, before a
  rating update caused by the current event.

Inactivity growth can push RD above the 350 default; that is the intended effect
of the formula.

### Imports are idempotent

`POST /api/v1/events/import` keys on the event `id`, so re-sending a batch is
free (`skipped`) and retrying a failed upload is safe. An `id` that arrives again
with *different* content is treated as a correction: the stored event is replaced
and the affected suffix of the log is replayed. Because events are kept sorted by
`(time, seq)`, a batch that only appends folds just the new tail; a batch that
inserts or corrects earlier history replays from the first changed event. Either
way the result is identical to importing everything in chronological order.

---

## API

Base path `/api/v1`. All list endpoints share a pagination envelope:

```json
{ "items": [...], "total": 42, "limit": 50, "offset": 0 }
```

`limit` defaults to 50, caps at 1000, and `limit=0` means **all rows** (this is
how "full leaderboard" is requested). `offset` past the end returns an empty
`items` array with the true `total`. `category` accepts `total`, `1v1` or `pub`
(default `total`; `duel` is accepted as an alias for `1v1`). Invalid values
return `400` with `{"error": "..."}`.

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/health` | Liveness plus log/player counts |
| `POST` | `/api/v1/events/import` | Import events |
| `GET` | `/api/v1/events` | Latest events, `player` filter, paginated |
| `GET` | `/api/v1/leaderboard` | Leaderboard, `category`, paginated |
| `GET` | `/api/v1/players/{login}` | One player, `category` optional |
| `GET` | `/api/v1/players/{login}/matchups` | Matchups vs other players, `category`, paginated |
| `GET` | `/api/v1/players/{login}/head-to-head/{opponent}` | Head-to-head vs one player, `category` |

### Import

Accepts either a bare array or `{"events": [...]}`.

```bash
curl -X POST http://127.0.0.1:8080/api/v1/events/import \
  -H 'Content-Type: application/json' \
  -d '[
    {"id":"e1","time":1700000000,"killer":"alice","victim":"bob","player_count":2},
    {"id":"e2","time":"2023-11-14T22:13:21Z","killer":"bob","victim":"alice","players":4},
    {"id":"e3","time":1700000002000,"killer":"alice","victim":".nobody","player_count":4}
  ]'
```

```json
{ "received": 3, "imported": 3, "skipped": 0, "updated": 0,
  "events_total": 3, "players": 2, "rebuilt": false }
```

* `id` — stable identifier from your system; required, unique, drives idempotency.
* `time` — unix seconds, unix milliseconds (auto-detected at ≥1e11), or an
  RFC 3339 string.
* `killer`, `victim` — logins. `player_count` — players in the lobby, accepted
  under the aliases `players`, `playerCount`, `lobby_size`.
* `imported` = new events, `skipped` = already present and identical,
  `updated` = already present with different content, `rebuilt` = the applied
  prefix had to be replayed.
* The whole batch is validated before anything is written, so a malformed
  payload cannot leave the log half-imported — a batch containing one bad event
  imports nothing.
* Every failure returns `{"error": "..."}` with a `4xx` status, including body
  parse failures (which axum would otherwise report as bare text):

  | Request | Response |
  | --- | --- |
  | missing field | `400 {"error":"Failed to deserialize the JSON body into the target type: [1]: missing field \`victim\` at line 1 column 98"}` |
  | empty id | `400 {"error":"event[0]: \`id\` must not be empty"}` |
  | malformed JSON | `400 {"error":"Failed to parse the request body as JSON: key must be a string at line 1 column 2"}` |
  | bad category | `400 {"error":"unknown category \"ranked\"; expected total, 1v1 or pub"}` |
  | unknown login | `404 {"error":"unknown player \"nope\""}` |

Import of a large history can be sent in chunks in any order:

```bash
python -c "
import json,urllib.request
ev=json.load(open('history.json'))
op=urllib.request.build_opener(urllib.request.ProxyHandler({}))
for i in range(0,len(ev),20000):
    req=urllib.request.Request('http://127.0.0.1:8080/api/v1/events/import',
        data=json.dumps(ev[i:i+20000]).encode(),
        headers={'Content-Type':'application/json'},method='POST')
    print(op.open(req).read().decode())
"
```

### Leaderboard

```bash
curl 'http://127.0.0.1:8080/api/v1/leaderboard?category=1v1&limit=2'
```

```json
{
  "items": [
    {
      "rank": 1,
      "login": "alice",
      "rating": 1662.3108939062977,
      "rd": 290.31896371798047,
      "volatility": 0.05999967537233814,
      "wins": 1,
      "losses": 0,
      "winrate": 1.0,
      "kills": 20,
      "deaths": 3,
      "kdr": 6.666666666666667,
      "last_active": "2023-11-14T22:15:02Z"
    },
    {
      "rank": 2,
      "login": "bob",
      "rating": 1337.6891060937023,
      "rd": 290.31896371798047,
      "volatility": 0.05999967537233814,
      "wins": 0,
      "losses": 1,
      "winrate": 0.0,
      "kills": 3,
      "deaths": 20,
      "kdr": 0.15,
      "last_active": "2023-11-14T22:15:02Z"
    }
  ],
  "total": 2,
  "limit": 2,
  "offset": 0
}
```

This is the result of a 20-3 duel win: the winner gains exactly what the loser
drops (±162.31), both RDs shrink, and wins/losses are whole numbers.

Sorted by `rating` descending, ties broken by login, so paging is stable.
`rank` is absolute across the whole category. Only players with at least one
eligible event in that category are listed — a player with no `pub` events does
not appear on the `pub` leaderboard. `kdr` is `null` until the player has died
(division by zero is undefined rather than reported as `0`). Numbers are returned
at full `f64` precision; formatting is the client's concern.

```bash
curl 'http://127.0.0.1:8080/api/v1/leaderboard?limit=0'      # full leaderboard
curl 'http://127.0.0.1:8080/api/v1/leaderboard?limit=50&offset=100'
curl 'http://127.0.0.1:8080/api/v1/leaderboard?category=pub'
```

### Player

```bash
curl http://127.0.0.1:8080/api/v1/players/alice
curl 'http://127.0.0.1:8080/api/v1/players/alice?category=pub'
```

```json
{
  "login": "alice",
  "categories": {
    "total": {
      "rating": 1662.3108939062977,
      "rd": 290.31896371798047,
      "volatility": 0.05999967537233814,
      "wins": 1,
      "losses": 0,
      "winrate": 1.0,
      "kills": 20,
      "deaths": 3,
      "kdr": 6.666666666666667,
      "last_active": "2023-11-14T22:15:02Z"
    },
    "1v1": {
      "rating": 1662.3108939062977,
      "rd": 290.31896371798047,
      "volatility": 0.05999967537233814,
      "wins": 1,
      "losses": 0,
      "winrate": 1.0,
      "kills": 20,
      "deaths": 3,
      "kdr": 6.666666666666667,
      "last_active": "2023-11-14T22:15:02Z"
    },
    "pub": {
      "rating": 1500.0,
      "rd": 350.0,
      "volatility": 0.06,
      "wins": 0,
      "losses": 0,
      "winrate": 0.0,
      "kills": 0,
      "deaths": 0,
      "kdr": null,
      "last_active": null
    }
  }
}
```

All three categories by default; `?category=` narrows the payload to one.
Unknown login → `404`. `last_active` is `null` for a category the player has no
eligible events in — here alice's only `pub` event was a world kill credited to
`.nobody`, so it left no trace in `pub` at all.

### Matchups

```bash
curl 'http://127.0.0.1:8080/api/v1/players/alice/matchups?category=1v1&limit=10'
```

```json
{ "items": [
    { "opponent": "bob", "kills": 20, "deaths": 3, "kdr": 6.666666666666667 }
  ],
  "total": 1, "limit": 10, "offset": 0 }
```

Lifetime head-to-head counters (not the in-progress match counters), sorted by
total engagements descending then opponent login. Opponents with no eligible
events in the selected category are omitted.

### Head-to-head

```bash
curl http://127.0.0.1:8080/api/v1/players/alice/head-to-head/bob
curl 'http://127.0.0.1:8080/api/v1/players/alice/head-to-head/bob?category=pub'
```

```json
{
  "category": "total",
  "player":   { "login": "alice", "kills": 20, "deaths": 3, "kdr": 6.666666666666667 },
  "opponent": { "login": "bob",   "kills": 3,  "deaths": 20, "kdr": 0.15 }
}
```

Symmetric by construction: `player.kills == opponent.deaths`. A pair that has
never met in the selected category returns zeros rather than `404`; an unknown
login on either side is `404`.

### Events feed

```bash
curl 'http://127.0.0.1:8080/api/v1/events?limit=2'
curl 'http://127.0.0.1:8080/api/v1/events?player=alice&limit=50&offset=50'
```

```json
{ "items": [
    { "id": "w0", "time": "2023-11-14T22:18:20Z", "killer": "alice", "victim": ".nobody",
      "player_count": 4, "lobby": "pub", "eligible": false },
    { "id": "p11", "time": "2023-11-14T22:16:51Z", "killer": "carol", "victim": "dave",
      "player_count": 4, "lobby": "pub", "eligible": true }
  ],
  "total": 36, "limit": 2, "offset": 0 }
```

Newest first. `player` matches events where that login was the killer **or** the
victim; `player=.nobody` lists the excluded events. `lobby` is the event's
classification (`1v1` / `pub`, `null` below two players) and `eligible` is false
exactly when a participant is `.nobody`.

---

## Performance

Measured on the development machine (`cargo build --release`, 12th Gen i7),
200 000 events across 200 players, uploaded in 10 shuffled 20 000-event chunks
(i.e. out of chronological order, forcing mid-log replays):

| Operation | Result |
| --- | --- |
| Import 200 000 events (24 MiB JSON) | **490 ms** total, ~2.5 µs/event |
| Re-import the same batch | all `skipped`, no recomputation |
| Leaderboard, 200 players, cold / warm | 1.4 ms / 0.9 ms |
| Leaderboard page of 100, first / offset page | 0.7 ms / 0.6 ms |
| Player info | 0.5 ms |
| Matchups, 139 rows | 0.7 ms |
| Latest 100 events | 1.0 ms |
| Latest 50 events for one player (200k log) | 3.3 ms |

(Timings are wall-clock over loopback and vary by a few ms run to run; the
leaderboard figures are per-category cold-cache misses and cache hits.)

How it stays fast and memory-lean:

* The fold is a single pass over the log with no per-event allocation; state is
  `Vec<PlayerState>` indexed by a `HashMap<login, u32>`.
* Pair state lives in one `HashMap<u64, PairState>` keyed by a packed unordered
  pair id, with a per-player adjacency list for matchups — so matchups are
  `O(engagements)`, not `O(players × players)`.
* Sorted leaderboards are cached behind an `Arc` and invalidated by a version
  counter on write, so repeated reads clone nothing and re-sort nothing.
* Incremental imports fold only the new tail; only genuinely out-of-order or
  corrective batches replay, and a replay is a single cheap pass.

The event log is held in memory (roughly 100 bytes/event), which is what makes
the single-pass fold and zero-copy reads possible. For a history that does not
fit in memory, the fold is the only part that needs changing: `Store::fold_tail`
reads a sorted log, so swapping `Vec<Event>` for a disk-backed or SQL-backed
source is contained.

---

## Verification

```bash
cargo test                                  # 30 unit tests

# End-to-end: every endpoint, cross-validated against an independent
# Python implementation of the same specification.
cd verify
python generate.py events.json 4000 30
../target/release/glicko-api --bind 127.0.0.1:8642 &
python smoke.py http://127.0.0.1:8642 events.json     # 52 checks

# Specification edge cases on a clean server (restart it first).
python edge.py  http://127.0.0.1:8642                 # 31 checks

# Scale, out-of-order import, latency. Needs an empty server: it replays
# exactly this file and compares the result against the oracle.
python generate.py big.json 200000 200
python perf.py  http://127.0.0.1:8642 big.json
```

`verify/oracle.py` re-derives the whole rating fold from the prose specification
and shares no code with the Rust implementation. `smoke.py` asserts that every
rating, RD, volatility, win/loss and kill/death counter from the API matches the
oracle to within `1e-9` across all three categories, alongside checks for
idempotent re-import, pagination, ranking, `kdr`/`winrate` consistency,
head-to-head symmetry, the `.nobody` exclusion, and every error path.

`verify/edge.py` asserts the rules that are easiest to get subtly wrong, through
the API rather than the internals: that 23h59m of inactivity adds no RD while 49h
adds exactly two periods, that fractional days are discarded, that a first event
never inflates RD, that inactivity moves only RD, that the growth is applied
*before* the current event's rating update (proved by an idle pair moving further
on the identical result than a pair that never idled), that a completed race
resets only that pair and only that category, that a growing lobby switches
category without transferring counters and the duel race resumes where it
stopped, that the margin-of-victory score is monotonic in the final tally, and
that `.nobody` events leave every counter byte-identical.

Unit tests cover the published Glicko-2 reference vector (1500/200/0.06 against
1400/30, 1550/100 and 1700/300 → 1464.06, 151.52, 0.05999), the volatility
solver, monotonicity of the margin-of-victory score, whole-day-only inactivity
growth (23h59m adds nothing, 49h adds two periods), first-event initialisation,
match resets that spare other pairs and other categories, lobby growth/shrink
category switching, self-kills, and that out-of-order chunked imports produce
byte-identical state to a chronological import.

---

## Layout

```
Cargo.toml
src/
  main.rs     CLI, startup import, server bootstrap
  glicko.rs   Glicko-2 math: g/expected, volatility solver, RD inflation, period update
  model.rs    constants, Category, Event, CatStats, PairState, DTOs, time parsing
  engine.rs   the event fold
  store.rs    event log, idempotent import/replay, cached leaderboards, queries
  api.rs      routes, query params, pagination, response shapes
verify/
  oracle.py   independent implementation of the specification (test oracle)
  generate.py deterministic event-history generator
  smoke.py    endpoint + oracle cross-validation suite
  edge.py     specification edge cases asserted through the API
  perf.py     scale, out-of-order import, latency
```
