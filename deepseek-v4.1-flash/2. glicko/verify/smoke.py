"""End-to-end smoke test: every endpoint, plus cross-validation against oracle.py.

Usage: python smoke.py <base_url> <events.json>

Checks performed:
  1. import (bare array), then re-import must be idempotent
  2. every GET endpoint returns the documented shape
  3. leaderboard stats are internally consistent (winrate, kdr, ranking)
  4. the Rust fold matches the independent Python oracle exactly
  5. pagination and filters behave
  6. .nobody events never appear in ratings or statistics
"""

import json
import math
import sys
import urllib.error
import urllib.parse
import urllib.request

import oracle

BASE = sys.argv[1].rstrip("/")
EVENTS = sys.argv[2]

# This machine has a system-wide HTTP proxy configured; never route the
# localhost API through it.
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))

failures = []


def check(name, ok, detail=""):
    status = "ok  " if ok else "FAIL"
    print("  [%s] %s%s" % (status, name, (" -- " + detail) if detail and not ok else ""))
    if not ok:
        failures.append(name)


def get(path, **params):
    url = BASE + path
    if params:
        url += "?" + urllib.parse.urlencode({k: v for k, v in params.items() if v is not None})
    with OPENER.open(url) as r:
        return r.status, json.loads(r.read().decode("utf-8"))


def post(path, payload):
    req = urllib.request.Request(
        BASE + path,
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with OPENER.open(req) as r:
        return r.status, json.loads(r.read().decode("utf-8"))


def expect_error(path, status, **params):
    try:
        get(path, **params)
    except urllib.error.HTTPError as e:
        return e.code == status
    return False


def main():
    events = json.load(open(EVENTS, encoding="utf-8"))
    print("history: %d events, %d distinct killers" % (len(events), len({e["killer"] for e in events})))

    print("\n== import ==")
    status, report = post("/api/v1/events/import", events)
    check("import returns 200", status == 200, str(status))
    check("import reports every event", report["imported"] == len(events), str(report))
    check("import did not rebuild", report["rebuilt"] is False, str(report))

    status, again = post("/api/v1/events/import", events)
    check("re-import is idempotent", again["imported"] == 0 and again["skipped"] == len(events), str(again))
    check("re-import kept the event count", again["events_total"] == len(events), str(again))

    status, wrapped = post("/api/v1/events/import", {"events": events[:5]})
    check("wrapped payload accepted", status == 200 and wrapped["skipped"] == 5, str(wrapped))

    status, health = get("/health")
    check("health reports the log size", health["events"] == len(events), str(health))

    print("\n== oracle cross-validation ==")
    o = oracle.fold(events)
    for cat in ("total", "1v1", "pub"):
        status, board = get("/api/v1/leaderboard", category=cat, limit=0)
        check("leaderboard[%s] size matches oracle" % cat, board["total"] == len(o.players), "%d vs %d" % (board["total"], len(o.players)))
        mismatches = []
        for row in board["items"]:
            s = o.players.get(row["login"])
            if s is None:
                mismatches.append("%s: missing in oracle" % row["login"])
                continue
            want = s[cat]
            if abs(row["rating"] - want["rating"]) > 1e-9:
                mismatches.append("%s rating %.12f vs %.12f" % (row["login"], row["rating"], want["rating"]))
            if abs(row["rd"] - want["rd"]) > 1e-9:
                mismatches.append("%s rd %.12f vs %.12f" % (row["login"], row["rd"], want["rd"]))
            if abs(row["volatility"] - want["sigma"]) > 1e-12:
                mismatches.append("%s sigma %.12f vs %.12f" % (row["login"], row["volatility"], want["sigma"]))
            for key in ("wins", "losses", "kills", "deaths"):
                if row[key] != want[key]:
                    mismatches.append("%s %s %s vs %s" % (row["login"], key, row[key], want[key]))
        check("ratings+stats[%s] match oracle" % cat, not mismatches, "; ".join(mismatches[:4]))

    print("\n== leaderboard integrity ==")
    status, board = get("/api/v1/leaderboard", limit=0)
    rows = board["items"]
    check("ranked descending", all(rows[i]["rating"] >= rows[i + 1]["rating"] for i in range(len(rows) - 1)))
    check("ranks are 1..n", [r["rank"] for r in rows] == list(range(1, len(rows) + 1)))
    bad = []
    for r in rows:
        matches = r["wins"] + r["losses"]
        if matches and abs(r["winrate"] - r["wins"] / matches) > 1e-12:
            bad.append("%s winrate" % r["login"])
        if r["deaths"] == 0 and r["kdr"] is not None:
            bad.append("%s kdr should be null" % r["login"])
        if r["deaths"] and abs(r["kdr"] - r["kills"] / r["deaths"]) > 1e-12:
            bad.append("%s kdr" % r["login"])
        if not (0.0 < r["rd"] and math.isfinite(r["rd"])):
            bad.append("%s rd %s invalid" % (r["login"], r["rd"]))
    check("winrate/kdr/rd self-consistent", not bad, "; ".join(bad[:4]))

    print("\n== pagination ==")
    status, p1 = get("/api/v1/leaderboard", limit=5, offset=0)
    status, p2 = get("/api/v1/leaderboard", limit=5, offset=5)
    check("page size honoured", len(p1["items"]) == 5, str(len(p1["items"])))
    check("total is the full count", p1["total"] == board["total"])
    check("pages do not overlap", not ({r["login"] for r in p1["items"]} & {r["login"] for r in p2["items"]}))
    check("offset is echoed", p2["offset"] == 5)
    status, tail = get("/api/v1/leaderboard", limit=5, offset=10 ** 6)
    check("offset past the end is empty", tail["items"] == [] and tail["total"] == board["total"])
    check("bad category rejected", expect_error("/api/v1/leaderboard", 400, category="ranked"))
    check("oversized limit rejected", expect_error("/api/v1/leaderboard", 400, limit=99999))

    print("\n== player info ==")
    login = rows[0]["login"]
    status, info = get("/api/v1/players/" + urllib.parse.quote(login))
    check("player has all three categories", set(info["categories"]) == {"total", "1v1", "pub"}, str(list(info["categories"])))
    for cat in ("total", "1v1", "pub"):
        got, want = info["categories"][cat], o.players[login][cat]
        check("player[%s] matches oracle" % cat,
              abs(got["rating"] - want["rating"]) < 1e-9
              and abs(got["rd"] - want["rd"]) < 1e-9
              and abs(got["volatility"] - want["sigma"]) < 1e-12
              and got["kills"] == want["kills"] and got["deaths"] == want["deaths"]
              and got["wins"] == want["wins"] and got["losses"] == want["losses"],
              "%s vs %s" % (got, want))
    status, filtered = get("/api/v1/players/" + urllib.parse.quote(login), category="pub")
    check("category filter narrows the payload", set(filtered["categories"]) == {"pub"})
    check("unknown player is 404", expect_error("/api/v1/players/nope_not_here", 404))

    print("\n== matchups ==")
    status, m = get("/api/v1/players/" + urllib.parse.quote(login) + "/matchups", limit=0)
    check("matchups listed", m["total"] > 0, str(m["total"]))
    check("matchup rows are complete", all(set(r) == {"opponent", "kills", "deaths", "kdr"} for r in m["items"]))
    bad = []
    for r in m["items"]:
        want = o.pairs.get(tuple(sorted((login, r["opponent"]))))
        if want is None:
            bad.append("%s missing in oracle" % r["opponent"])
            continue
        lo, hi = sorted((login, r["opponent"]))
        idx = 0 if login == lo else 1
        if r["kills"] != want["kills"]["total"][idx] or r["deaths"] != want["kills"]["total"][1 - idx]:
            bad.append("%s %s/%s vs %s/%s" % (r["opponent"], r["kills"], r["deaths"],
                                              want["kills"]["total"][idx], want["kills"]["total"][1 - idx]))
    check("matchup counters match oracle", not bad, "; ".join(bad[:4]))
    status, mp = get("/api/v1/players/" + urllib.parse.quote(login) + "/matchups", limit=2, offset=1)
    check("matchup pagination", len(mp["items"]) <= 2 and mp["total"] == m["total"])

    print("\n== head-to-head ==")
    opp = m["items"][0]["opponent"]
    path = "/api/v1/players/%s/head-to-head/%s" % (urllib.parse.quote(login), urllib.parse.quote(opp))
    status, h2h = get(path)
    check("h2h category defaults to total", h2h["category"] == "total")
    check("h2h mirrors the matchup row",
          h2h["player"]["kills"] == m["items"][0]["kills"] and h2h["player"]["deaths"] == m["items"][0]["deaths"])
    check("h2h is symmetric", h2h["player"]["kills"] == h2h["opponent"]["deaths"])
    check("h2h kdr is present or null",
          (h2h["player"]["kdr"] is None) == (h2h["player"]["deaths"] == 0))
    status, h2h_pub = get(path, category="pub")
    check("h2h category selection", h2h_pub["category"] == "pub")
    status, h2h_rev = get("/api/v1/players/%s/head-to-head/%s" % (urllib.parse.quote(opp), urllib.parse.quote(login)))
    check("h2h reverses cleanly", h2h_rev["player"]["kills"] == h2h["player"]["deaths"])
    check("h2h unknown opponent is 404", expect_error("/api/v1/players/%s/head-to-head/nope" % urllib.parse.quote(login), 404))

    print("\n== events feed ==")
    status, ev = get("/api/v1/events", limit=10)
    check("newest first", all(ev["items"][i]["time"] >= ev["items"][i + 1]["time"] for i in range(len(ev["items"]) - 1)))
    check("event total equals the log", ev["total"] == len(events), "%s vs %s" % (ev["total"], len(events)))
    check("event fields complete",
          all(set(e) == {"id", "time", "killer", "victim", "player_count", "lobby", "eligible"} for e in ev["items"]))
    check("lobby label matches player_count",
          all((e["lobby"] == "1v1") == (e["player_count"] == 2) and (e["lobby"] == "pub") == (e["player_count"] >= 3)
              for e in ev["items"]))

    status, mine = get("/api/v1/events", player=login, limit=0)
    expected = sum(1 for e in events if e["killer"] == login or e["victim"] == login)
    check("player filter counts both roles", mine["total"] == expected, "%s vs %s" % (mine["total"], expected))
    check("player filter only returns that player",
          all(e["killer"] == login or e["victim"] == login for e in mine["items"]))
    check("filtered feed is newest first",
          all(mine["items"][i]["time"] >= mine["items"][i + 1]["time"] for i in range(len(mine["items"]) - 1)))

    status, page = get("/api/v1/events", limit=3, offset=2)
    check("event pagination", len(page["items"]) == 3 and page["items"][0]["id"] == ev["items"][2]["id"])

    print("\n== .nobody exclusion ==")
    status, nobody = get("/api/v1/events", player=oracle.NOBODY, limit=0)
    check("nobody events are recorded", nobody["total"] > 0, str(nobody["total"]))
    check("nobody events are marked ineligible", all(not e["eligible"] for e in nobody["items"]))
    check("nobody is not a player", expect_error("/api/v1/players/%s" % urllib.parse.quote(oracle.NOBODY), 404))
    rated_logins = {r["login"] for r in rows}
    check("nobody is not on the leaderboard", oracle.NOBODY not in rated_logins)

    print("\n== summary ==")
    if failures:
        print("FAILED (%d): %s" % (len(failures), ", ".join(failures)))
        return 1
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
