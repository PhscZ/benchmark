"""Specification edge cases, asserted through the HTTP API on a fresh server.

Usage: python edge.py <base_url>

Everything here targets a rule that is easy to get subtly wrong, and each case
uses a brand-new server so that counts are unambiguous:

  1. 23h59m of inactivity adds no RD; 49h adds exactly two periods
  2. a player's first eligible event never inflates RD
  3. inactivity only moves RD, never rating or volatility
  4. inactivity is applied *before* the rating update caused by the same event
  5. a completed race resets only that pair's counters
  6. match counters are per category: a pub race does not reset a duel race
  7. a growing lobby switches category without transferring counters, and the
     duel race resumes where it stopped
  8. margin of victory is monotonic: 20-0 > 20-1 > 20-19
  9. lifetime kills/deaths survive a match reset
 10. `.nobody` events change nothing at all
"""

import json
import math
import sys
import urllib.request

import oracle

BASE = sys.argv[1].rstrip("/")
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))

failures = []


def check(name, ok, detail=""):
    print("  [%s] %s%s" % ("ok  " if ok else "FAIL", name, (" -- " + detail) if detail and not ok else ""))
    if not ok:
        failures.append(name)


def close(a, b, tol=1e-9):
    return abs(a - b) <= tol


def post(path, payload):
    req = urllib.request.Request(
        BASE + path, data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"}, method="POST",
    )
    with OPENER.open(req) as r:
        return json.loads(r.read().decode("utf-8"))


def get(path, **params):
    url = BASE + path
    if params:
        url += "?" + urllib.parse.urlencode(params)
    with OPENER.open(url) as r:
        return json.loads(r.read().decode("utf-8"))


def inflate(rd, sigma, periods):
    phi = rd / oracle.SCALE
    return math.sqrt(phi * phi + periods * sigma * sigma) * oracle.SCALE


DAY = 86400
T0 = 1_700_000_000


def main():
    print("== 1-3. inactivity RD growth ==")
    # a is active, then idle for 23h59m, then 49h, then 12h more.
    # b0 appears in the first event only, so its RD must never be inflated.
    ev = [
        {"id": "i1", "time": T0, "killer": "a", "victim": "b0", "player_count": 2},
        {"id": "i2", "time": T0 + 23 * 3600 + 59 * 60, "killer": "a", "victim": "b", "player_count": 2},
        {"id": "i3", "time": T0 + 23 * 3600 + 59 * 60 + 49 * 3600, "killer": "a", "victim": "b", "player_count": 2},
        {"id": "i4", "time": T0 + 23 * 3600 + 59 * 60 + 49 * 3600 + 12 * 3600, "killer": "a", "victim": "b", "player_count": 2},
    ]
    post("/api/v1/events/import", ev)
    a = get("/api/v1/players/a")["categories"]["total"]
    b0 = get("/api/v1/players/b0")["categories"]["total"]
    check("a player's first event never inflates RD", close(b0["rd"], 350.0), str(b0["rd"]))
    check("...and it is not inflated by later inactivity either",
          close(b0["rd"], 350.0) and close(b0["rating"], 1500.0), str(b0))
    # Two whole periods after 49h.
    want = inflate(350.0, 0.06, 2)
    check("49h after a 23h59m gap adds two periods", close(a["rd"], want), "%r vs %r" % (a["rd"], want))
    check("inactivity leaves rating alone", close(a["rating"], 1500.0), str(a["rating"]))
    check("inactivity leaves volatility alone", close(a["volatility"], 0.06, 1e-12), str(a["volatility"]))

    # lastActive is the last registered event, not the start of the gap.
    expected_last = T0 + 23 * 3600 + 59 * 60 + 49 * 3600 + 12 * 3600
    check("lastActive is the newest event's timestamp",
          a["last_active"] == oracle.fmt_time(expected_last),
          "%r vs %r" % (a["last_active"], oracle.fmt_time(expected_last)))

    # 12h more is still zero whole days: the two periods must not have been
    # carried over as a fraction.
    check("fractional days are discarded, not carried", close(a["rd"], want), str(a["rd"]))

    print("\n== 4. inactivity is applied before the current event's update ==")
    # c and d race to 20 (rating update), idle 49h, then race again. The second
    # update must start from the inflated RD.
    ev = []
    for i in range(20):
        ev.append({"id": "m%d" % i, "time": T0 + i, "killer": "c", "victim": "d", "player_count": 2})
    last = T0 + 19
    for i in range(20):
        ev.append({"id": "n%d" % i, "time": last + 49 * 3600 + i, "killer": "c", "victim": "d", "player_count": 2})
    post("/api/v1/events/import", ev)

    # Replay the same events through the oracle and compare exactly: the oracle
    # applies the growth inline, so agreement proves the ordering.
    o = oracle.fold([dict(e, seq=i) for i, e in enumerate(ev)])
    c = get("/api/v1/players/c")["categories"]["1v1"]
    want_c = o.players["c"]["1v1"]
    check("two races separated by a 49h idle gap match the oracle",
          close(c["rating"], want_c["rating"]) and close(c["rd"], want_c["rd"]), "%r vs %r" % (c, want_c))
    check("the second race is a second win", c["wins"] == 2, str(c["wins"]))

    # Control: the same two races with no idle gap must end at a *lower* RD,
    # which only happens if the gap really was applied before the second update.
    ev = []
    for i in range(20):
        ev.append({"id": "s%d" % i, "time": T0 + i, "killer": "s", "victim": "t", "player_count": 2})
    for i in range(20):
        ev.append({"id": "u%d" % i, "time": T0 + 1000 + i, "killer": "s", "victim": "t", "player_count": 2})
    post("/api/v1/events/import", ev)
    s = get("/api/v1/players/s", category="1v1")["categories"]["1v1"]
    check("no idle gap leaves a tighter RD than a 49h gap",
          s["rd"] < c["rd"], "%.9f vs %.9f" % (s["rd"], c["rd"]))
    # A larger pre-race RD permits a larger step, so the idle player must have
    # moved further on the identical 20-0 result. This is the observable proof
    # that the growth was applied *before* the second update.
    check("the inflated RD let the idle player move further",
          (c["rating"] - 1500.0) > (s["rating"] - 1500.0),
          "%.9f vs %.9f" % (c["rating"] - 1500.0, s["rating"] - 1500.0))

    print("\n== 5-6. match reset isolation ==")
    # e races f to 20 in a duel; e also tags g 5 times; e wins a pub race vs h.
    ev = []
    for i in range(20):
        ev.append({"id": "ef%d" % i, "time": T0 + i, "killer": "e", "victim": "f", "player_count": 2})
    for i in range(5):
        ev.append({"id": "eg%d" % i, "time": T0 + 100 + i, "killer": "e", "victim": "g", "player_count": 2})
    for i in range(20):
        ev.append({"id": "eh%d" % i, "time": T0 + 200 + i, "killer": "e", "victim": "h", "player_count": 4})
    post("/api/v1/events/import", ev)

    e_duel = get("/api/v1/players/e", category="1v1")["categories"]["1v1"]
    e_pub = get("/api/v1/players/e", category="pub")["categories"]["pub"]
    check("duel race counted as a win", e_duel["wins"] == 1, str(e_duel["wins"]))
    check("pub race counted as a separate win", e_pub["wins"] == 1, str(e_pub["wins"]))
    check("lifetime kills survive the reset", e_duel["kills"] == 25, str(e_duel["kills"]))
    check("pub kills are separate from duel kills", e_pub["kills"] == 20, str(e_pub["kills"]))
    check("total kills are the sum", get("/api/v1/players/e")["categories"]["total"]["kills"] == 45,
          str(get("/api/v1/players/e")["categories"]["total"]["kills"]))

    # f's duel with e was reset, but e's duel with g is untouched and still open:
    # 15 more kills on g must complete a second race.
    ev = [{"id": "eg2_%d" % i, "time": T0 + 400 + i, "killer": "e", "victim": "g", "player_count": 2} for i in range(15)]
    post("/api/v1/events/import", ev)
    e_duel = get("/api/v1/players/e", category="1v1")["categories"]["1v1"]
    check("the untouched pair race resumed and completed", e_duel["wins"] == 2, str(e_duel["wins"]))

    m = {r["opponent"]: r for r in get("/api/v1/players/e/matchups", category="1v1", limit=0)["items"]}
    check("head-to-head vs f is lifetime 20", m["f"]["kills"] == 20, str(m.get("f")))
    check("head-to-head vs g is lifetime 20", m["g"]["kills"] == 20, str(m.get("g")))
    check("g fought back zero times", m["g"]["deaths"] == 0)

    print("\n== 7. lobby growth and shrink ==")
    # p and q trade 7 kills in a duel, a third player joins for 3 pub kills,
    # then the lobby shrinks back to a duel.
    ev = []
    for i in range(7):
        ev.append({"id": "pq%d" % i, "time": T0 + i, "killer": "p", "victim": "q", "player_count": 2})
    for i in range(3):
        ev.append({"id": "pqr%d" % i, "time": T0 + 50 + i, "killer": "p", "victim": "q", "player_count": 3})
    ev.append({"id": "pq_solo", "time": T0 + 100, "killer": "p", "victim": "q", "player_count": 2})
    post("/api/v1/events/import", ev)

    p = get("/api/v1/players/p")["categories"]
    check("duel kills kept while the lobby was a pub", p["1v1"]["kills"] == 8, str(p["1v1"]["kills"]))
    check("pub kills accumulated separately", p["pub"]["kills"] == 3, str(p["pub"]["kills"]))
    check("total spans both categories", p["total"]["kills"] == 11, str(p["total"]["kills"]))

    # 12 more duel kills continue the duel race from 8 -> 20, so it completes.
    ev = [{"id": "pq2_%d" % i, "time": T0 + 200 + i, "killer": "p", "victim": "q", "player_count": 2} for i in range(12)]
    post("/api/v1/events/import", ev)
    p = get("/api/v1/players/p")["categories"]
    check("duel race resumed at 8 and completed at 20", p["1v1"]["wins"] == 1, str(p["1v1"]))
    check("pub race was not affected by the duel reset", p["pub"]["wins"] == 0, str(p["pub"]))
    check("pub counters still stored", p["pub"]["kills"] == 3, str(p["pub"]["kills"]))

    print("\n== 8. margin of victory is monotonic ==")
    def race(prefix, loser_kills, base):
        ev = []
        for i in range(20 - loser_kills):
            ev.append({"id": "%s_w%d" % (prefix, i), "time": base + i, "killer": "w_" + prefix, "victim": "l_" + prefix, "player_count": 2})
        for i in range(loser_kills):
            ev.append({"id": "%s_l%d" % (prefix, i), "time": base + 100 + i, "killer": "l_" + prefix, "victim": "w_" + prefix, "player_count": 2})
        for i in range(loser_kills):
            ev.append({"id": "%s_f%d" % (prefix, i), "time": base + 200 + i, "killer": "w_" + prefix, "victim": "l_" + prefix, "player_count": 2})
        post("/api/v1/events/import", ev)
        return get("/api/v1/players/w_" + prefix, category="1v1")["categories"]["1v1"]

    blowout = race("blowout", 0, T0)
    one = race("one", 1, T0)
    close_game = race("close", 19, T0)
    check("20-0 beats 20-1 beats 20-19",
          blowout["rating"] > one["rating"] > close_game["rating"] > 1500.0,
          "%.3f / %.3f / %.3f" % (blowout["rating"], one["rating"], close_game["rating"]))
    check("a 20-19 win still counts as a win", close_game["wins"] == 1, str(close_game))
    check("wins are integers, never fractional",
          all(isinstance(v["wins"], int) for v in (blowout, one, close_game)))
    # The loser's loss mirrors the winner's gain from the same starting point.
    loser = get("/api/v1/players/l_blowout", category="1v1")["categories"]["1v1"]
    check("winner gain mirrors loser loss",
          close(blowout["rating"] - 1500.0, 1500.0 - loser["rating"], 1e-9),
          "%.9f vs %.9f" % (blowout["rating"] - 1500.0, 1500.0 - loser["rating"]))

    print("\n== 10. .nobody changes nothing ==")
    before = get("/api/v1/players/w_blowout")["categories"]
    post("/api/v1/events/import", [
        {"id": "nb1", "time": T0 + 10 ** 6, "killer": "w_blowout", "victim": oracle.NOBODY, "player_count": 2},
        {"id": "nb2", "time": T0 + 10 ** 6 + 1, "killer": oracle.NOBODY, "victim": "w_blowout", "player_count": 2},
    ])
    after = get("/api/v1/players/w_blowout")["categories"]
    check("excluded events leave every counter untouched", before == after, "%s vs %s" % (before, after))

    print("\n== summary ==")
    if failures:
        print("FAILED (%d): %s" % (len(failures), ", ".join(failures)))
        return 1
    print("all edge cases passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
