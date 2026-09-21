"""Scale check: large history, out-of-order chunked import, query latency.

Usage: python perf.py <base_url> <events.json>

Proves three things the spec cares about:
  * "processing the existing event history" is fast
  * chunks that arrive out of chronological order still fold to the same state
  * serving queries stays sub-millisecond-ish at scale
"""

import json
import random
import sys
import time
import urllib.request

import oracle

BASE = sys.argv[1].rstrip("/")
EVENTS = sys.argv[2]
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def get(path, **params):
    url = BASE + path
    if params:
        url += "?" + urllib.parse.urlencode(params)
    t0 = time.perf_counter()
    with OPENER.open(url) as r:
        payload = json.loads(r.read().decode("utf-8"))
    return (time.perf_counter() - t0) * 1000.0, payload


def post(path, payload):
    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        BASE + path, data=body, headers={"Content-Type": "application/json"}, method="POST"
    )
    t0 = time.perf_counter()
    with OPENER.open(req) as r:
        out = json.loads(r.read().decode("utf-8"))
    return (time.perf_counter() - t0) * 1000.0, out, len(body)


def main():
    events = json.load(open(EVENTS, encoding="utf-8"))
    print("history: %d events, %.2f MiB JSON" % (len(events), len(json.dumps(events)) / 1048576))

    # The oracle comparison below replays exactly this file, so the server must
    # hold exactly this history and nothing else.
    existing = get("/health")[1]["events"]
    if existing:
        print("perf.py needs an empty server, but it already holds %d events."
              % existing)
        print("Restart it (or start a second instance on another port) and retry.")
        return 2

    # Shuffle the chunks so the server receives history out of chronological
    # order, exactly like a backfill from an existing system.
    chunks = [events[i:i + 20000] for i in range(0, len(events), 20000)]
    random.Random(11).shuffle(chunks)

    total_ms = 0.0
    total_bytes = 0
    for i, chunk in enumerate(chunks):
        ms, report, nbytes = post("/api/v1/events/import", chunk)
        total_ms += ms
        total_bytes += nbytes
        if i == 0 or i == len(chunks) - 1:
            print("  chunk %d: %d events in %.0f ms (%s)" % (i, len(chunk), ms, report))
    print("import: %d events in %.0f ms total (%.0f MiB sent)" % (len(events), total_ms, total_bytes / 1048576))

    ms, health = get("/health")
    print("health: %.1f ms %s" % (ms, health))
    assert health["events"] == len(events), health

    print("\nqueries (cold cache, then warm):")
    for cat in ("total", "1v1", "pub"):
        ms, board = get("/api/v1/leaderboard", category=cat, limit=0)
        ms2, _ = get("/api/v1/leaderboard", category=cat, limit=0)
        print("  leaderboard[%s] %d players: cold %.1f ms, warm %.1f ms" % (cat, board["total"], ms, ms2))

    ms, board = get("/api/v1/leaderboard", limit=100)
    ms2, _ = get("/api/v1/leaderboard", limit=100, offset=200)
    print("  leaderboard page of 100: %.1f ms / offset page %.1f ms" % (ms, ms2))

    login = board["items"][0]["login"]
    ms, _ = get("/api/v1/players/" + login)
    print("  player info: %.1f ms" % ms)
    ms, m = get("/api/v1/players/%s/matchups" % login, limit=0)
    print("  matchups (%d rows): %.1f ms" % (m["total"], ms))
    ms, _ = get("/api/v1/players/%s/matchups" % login, limit=10, offset=10)
    print("  matchups page: %.1f ms" % ms)
    ms, _ = get("/api/v1/events", limit=100)
    print("  latest 100 events: %.1f ms" % ms)
    ms, _ = get("/api/v1/events", player=login, limit=50)
    print("  latest 50 events for one player: %.1f ms" % ms)

    print("\nverify shuffled import matches a chronological fold:")
    t0 = time.perf_counter()
    o = oracle.fold(events)
    print("  oracle fold: %.1f s" % (time.perf_counter() - t0))

    ms, board = get("/api/v1/leaderboard", category="total", limit=0)
    bad = []
    for row in board["items"]:
        want = o.players[row["login"]]["total"]
        if abs(row["rating"] - want["rating"]) > 1e-9 or row["kills"] != want["kills"]:
            bad.append("%s %.9f vs %.9f" % (row["login"], row["rating"], want["rating"]))
    print("  leaderboard matches oracle: %s" % ("yes" if not bad else "NO -- " + "; ".join(bad[:3])))
    if bad:
        return 1

    # Spot-check matchups at scale too.
    mismatched = 0
    for row in board["items"][:25]:
        login = row["login"]
        _, m = get("/api/v1/players/%s/matchups" % login, limit=0, category="1v1")
        for r in m["items"]:
            lo, hi = sorted((login, r["opponent"]))
            idx = 0 if login == lo else 1
            want = o.pairs[(lo, hi)]["kills"].get("1v1", [0, 0])
            if r["kills"] != want[idx] or r["deaths"] != want[1 - idx]:
                mismatched += 1
    print("  1v1 matchup spot-check (25 players): %s" % ("yes" if not mismatched else "NO (%d bad)" % mismatched))
    return 1 if mismatched else 0


if __name__ == "__main__":
    sys.exit(main())
