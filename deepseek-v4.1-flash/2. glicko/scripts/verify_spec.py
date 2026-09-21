#!/usr/bin/env python3
"""End-to-end verification of the specified semantics against a running server.

Requires a server over an EMPTY data directory, since the checks assert absolute
counts and it imports a fixed history:

    ./target/release/glicko-api --data-dir /tmp/glicko-verify --port 8124
    python scripts/verify_spec.py --base 127.0.0.1:8124

Every check asserts one requirement from the specification and prints PASS/FAIL.
"""

from __future__ import annotations

import argparse
import http.client
import json
import sys
from datetime import datetime, timedelta, timezone

BASE = "127.0.0.1:8124"
FAILURES: list[str] = []
CHECKS = 0


def call(method: str, path: str, body=None):
    """One HTTP request. Uses http.client so no ambient proxy can interfere."""
    host, port = BASE.split(":")
    connection = http.client.HTTPConnection(host, int(port), timeout=120)
    try:
        headers = {"content-type": "application/json"} if body is not None else {}
        payload = json.dumps(body).encode() if body is not None else None
        connection.request(method, path, body=payload, headers=headers)
        response = connection.getresponse()
        raw = response.read()
        return response.status, (json.loads(raw) if raw else None)
    finally:
        connection.close()


def check(label: str, actual, expected) -> None:
    global CHECKS
    CHECKS += 1
    if actual != expected:
        FAILURES.append(f"{label}: expected {expected!r}, got {actual!r}")
        print(f"FAIL {label}: expected {expected!r}, got {actual!r}")
    else:
        print(f"pass {label}")


def check_close(label: str, actual: float, expected: float, tolerance: float) -> None:
    global CHECKS
    CHECKS += 1
    if abs(actual - expected) > tolerance:
        FAILURES.append(f"{label}: expected {expected} +/- {tolerance}, got {actual}")
        print(f"FAIL {label}: expected {expected} +/- {tolerance}, got {actual}")
    else:
        print(f"pass {label}")


def event(event_id: str, when: datetime, killer: str, victim: str, player_count: int) -> dict:
    return {
        "id": event_id,
        "time": when.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "killer": killer,
        "victim": victim,
        "player_count": player_count,
    }


def import_events(events: list[dict]) -> dict:
    status, report = call("POST", "/events/import", events)
    if status != 200:
        raise SystemExit(f"import failed: {status} {report}")
    return report


def race(prefix: str, start: datetime, winner: str, loser: str, player_count: int, loser_kills: int, step=1):
    """A 20-x race, loser kills interleaved so the score is realistic."""
    events = []
    won = lost = 0
    total = 20 + loser_kills
    for i in range(total):
        if lost < loser_kills and (i % 2 == 1 or won == 20):
            lost += 1
            events.append(event(f"{prefix}-{i}", start + timedelta(seconds=i * step), loser, winner, player_count))
        else:
            won += 1
            events.append(event(f"{prefix}-{i}", start + timedelta(seconds=i * step), winner, loser, player_count))
    assert (won, lost) == (20, loser_kills), (won, lost)
    return events


def main() -> None:
    global BASE
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default=BASE, help="host:port of the server under test")
    args = parser.parse_args()
    BASE = args.base

    print(f"== verifying against http://{BASE} ==\n")

    # ---------------------------------------------------------------- defaults
    print("-- defaults --")
    _, config = call("GET", "/config")
    check("default rating", config["initial_rating"], 1500.0)
    check("default rd", config["initial_rd"], 350.0)
    check("default volatility", config["initial_volatility"], 0.06)
    check("default scale", config["scale"], 173.7178)
    check("default epsilon", config["epsilon"], 1e-6)
    check("default tau", config["tau"], 0.5)
    check("match target", config["match_kill_target"], 20)
    check("excluded logins", config["excluded_logins"], [".nobody", ".self"])

    # ------------------------------------------- the attachment's sample payload
    print("\n-- sample payload from the specification --")
    report = import_events(
        [
            {"id": "e5", "time": "2026-05-16T18:36:04Z", "killer": "nicolas404", "victim": "orinslc", "player_count": 3},
            {"id": "e10", "time": "2026-05-16T18:36:25Z", "killer": "nicolas404", "victim": "orinslc", "player_count": 3},
            {"id": "e11", "time": "2026-05-16T18:36:25Z", "killer": "orinslc", "victim": "nicolas404", "player_count": 2},
        ]
    )
    check("imported", report["imported"], 3)
    check("players", report["players"], 2)
    check("no race completed", report["matches"], 0)

    _, total = call("GET", "/leaderboard?category=total")
    nicolas = next(p for p in total if p["login"] == "nicolas404")
    orinslc = next(p for p in total if p["login"] == "orinslc")
    check("nicolas kills/deaths", (nicolas["kills"], nicolas["deaths"]), (2, 1))
    check("nicolas kd", nicolas["kd_ratio"], 2.0)
    check("nicolas winrate (no matches yet)", nicolas["winrate"], 0.0)
    check("orinslc kills/deaths", (orinslc["kills"], orinslc["deaths"]), (1, 2))
    check("orinslc kd", orinslc["kd_ratio"], 0.5)

    # ------------------------------------- classification by recorded player_count
    print("\n-- categories follow the event's recorded player count --")
    _, pub = call("GET", "/leaderboard?category=pub")
    pub_logins = sorted(p["login"] for p in pub)
    check("pub ladder", pub_logins, ["nicolas404", "orinslc"])
    check("pub kills for nicolas", next(p for p in pub if p["login"] == "nicolas404")["kills"], 2)
    _, duel = call("GET", "/leaderboard?category=1v1")
    check("1v1 kills for orinslc", next(p for p in duel if p["login"] == "orinslc")["kills"], 1)
    _, player = call("GET", "/players/nicolas404?category=1v1")
    check("nicolas 1v1 kills/deaths", (player["kills"], player["deaths"]), (0, 1))

    # ----------------------------------------------- re-import is idempotent
    print("\n-- idempotent re-import --")
    report = import_events(
        [{"id": "e5", "time": "2026-05-16T18:36:04Z", "killer": "nicolas404", "victim": "orinslc", "player_count": 3}]
    )
    check("duplicates detected", report["duplicates"], 1)
    check("nothing new imported", report["imported"], 0)
    _, player = call("GET", "/players/nicolas404")
    check("kills unchanged", player["kills"], 2)

    # -------------------------------------------------------- exclusions
    print("\n-- .nobody / .self excluded from ratings and statistics --")
    import_events(
        [
            event("x1", datetime(2026, 5, 16, 19, 0, tzinfo=timezone.utc), ".nobody", "alice", 2),
            event("x2", datetime(2026, 5, 16, 19, 0, 1, tzinfo=timezone.utc), "alice", ".self", 2),
            event("x3", datetime(2026, 5, 16, 19, 0, 2, tzinfo=timezone.utc), "alice", "bob", 2),
        ]
    )
    status, _ = call("GET", "/players/.nobody")
    check(".nobody is not a player", status, 404)
    _, alice = call("GET", "/players/alice")
    check("alice kills only counts the eligible event", alice["kills"], 1)
    check("alice deaths", alice["deaths"], 0)
    _, events = call("GET", "/events?limit=100")
    check("excluded events are still in the log", any(e["killer"] == ".nobody" for e in events["items"]), True)

    # ------------------------------------ race to 20, margin of victory, resets
    print("\n-- race to 20 kills, per-pair counter reset, margin of victory --")
    base = datetime(2026, 6, 1, 0, 0, tzinfo=timezone.utc)
    import_events(race("dom", base, "dominant", "victim", 2, 0))
    _, winner = call("GET", "/players/dominant")
    check("winner wins", winner["wins"], 1)
    check("winner kills (cumulative, not reset)", winner["kills"], 20)
    check("winner winrate", winner["winrate"], 1.0)
    _, loser = call("GET", "/players/victim")
    check("loser losses", loser["losses"], 1)
    check("loser deaths (cumulative)", loser["deaths"], 20)
    dominant_gain = winner["rating"] - 1500.0
    check("a 20-0 win gains rating", dominant_gain > 0, True)

    # a close race must move the rating much less than a dominant one
    base = datetime(2026, 6, 2, 0, 0, tzinfo=timezone.utc)
    import_events(race("close", base, "closer", "runner", 2, 19))
    _, closer = call("GET", "/players/closer")
    close_gain = closer["rating"] - 1500.0
    check("a 20-19 win still gains rating", close_gain > 0, True)
    check("a 20-19 win gains far less than a 20-0 win", close_gain < dominant_gain / 4, True)
    check("a 20-19 win is still a win, not a fraction", closer["wins"], 1)

    # the reset is per pair: a's race against b resets, against c does not
    base = datetime(2026, 6, 3, 0, 0, tzinfo=timezone.utc)
    events = []
    for i in range(19):
        events.append(event(f"pair-ab-{i}", base + timedelta(seconds=i * 2), "paira", "pairb", 2))
        events.append(event(f"pair-ac-{i}", base + timedelta(seconds=i * 2 + 1), "paira", "pairc", 2))
    events.append(event("pair-ab-win", base + timedelta(seconds=100), "paira", "pairb", 2))
    import_events(events)
    _, paira = call("GET", "/players/paira")
    check("paira wins the completed race", paira["wins"], 1)
    check("paira cumulative kills include both races", paira["kills"], 39)
    _, h2h_b = call("GET", "/players/paira/matchups/pairb")
    check("counter against the beaten opponent is reset", h2h_b["kills"], 20)
    _, h2h_c = call("GET", "/players/paira/matchups/pairc")
    check("counter against the other opponent is untouched", h2h_c["kills"], 19)
    check("h2h kills/deaths/kd", (h2h_c["kills"], h2h_c["deaths"], h2h_c["kd_ratio"]), (19, 0, 19.0))

    # ------------------------------------------- dynamic lobby / category switch
    print("\n-- dynamic lobby: category follows the recorded player count --")
    base = datetime(2026, 7, 1, 0, 0, tzinfo=timezone.utc)
    events = [event(f"lob-1v1-{i}", base + timedelta(seconds=i), "mover", "target", 2) for i in range(15)]
    events += [event(f"lob-pub-{i}", base + timedelta(seconds=1000 + i), "mover", "target", 4) for i in range(5)]
    import_events(events)
    _, mover_1v1 = call("GET", "/players/mover?category=1v1")
    _, mover_pub = call("GET", "/players/mover?category=pub")
    _, mover_total = call("GET", "/players/mover?category=total")
    check("1v1 kills stop at the lobby change", mover_1v1["kills"], 15)
    check("pub kills start fresh", mover_pub["kills"], 5)
    check("total sees every eligible event", mover_total["kills"], 20)
    check("the 1v1 race did not complete", mover_1v1["wins"], 0)
    check("the pub race did not complete either", mover_pub["wins"], 0)
    check("the total race did complete", mover_total["wins"], 1)

    # ------------------------------------------------------ inactivity handling
    print("\n-- inactivity RD growth: whole days, reactive, per player --")
    t0 = datetime(2026, 8, 1, 0, 0, tzinfo=timezone.utc)
    import_events([event("inact-0", t0, "idle", "sparring", 2)])
    _, idle = call("GET", "/players/idle")
    check("starts at the default RD", idle["rd"], 350.0)

    # 23h59m: less than one whole period, so no growth
    import_events([event("inact-1", t0 + timedelta(hours=23, minutes=59), "idle", "sparring", 2)])
    _, idle = call("GET", "/players/idle")
    check("23h59m adds no inactivity RD", idle["rd"], 350.0)

    # 49h after that: two whole periods
    import_events([event("inact-2", t0 + timedelta(hours=23, minutes=59) + timedelta(hours=49), "idle", "sparring", 2)])
    _, idle = call("GET", "/players/idle")
    phi = 350.0 / 173.7178
    expected_rd = (phi * phi + 2 * 0.06 * 0.06) ** 0.5 * 173.7178
    check_close("49h adds exactly two periods of RD growth", idle["rd"], expected_rd, 1e-6)
    check("rating is untouched by inactivity", idle["rating"], 1500.0)
    check("volatility is untouched by inactivity", idle["volatility"], 0.06)

    # ------------------------------------------------------------ alt accounts
    print("\n-- alt accounts --")
    # the alt racks up kills; the main is registered later and must be recalculated
    base = datetime(2026, 9, 1, 0, 0, tzinfo=timezone.utc)
    import_events([event(f"alt-{i}", base + timedelta(seconds=i), "fck", "bob", 2) for i in range(20)])
    _, fck = call("GET", "/noalt/players/fck")
    check("noalt view: the alt is its own player", (fck["login"], fck["wins"]), ("fck", 1))
    _, before = call("GET", "/leaderboard")
    check("alt-aware view before the mapping has no main", any(p["login"] == "deadlyenergy" for p in before), False)

    _, before_mapping = call("GET", "/health")
    status, report = call(
        "POST",
        "/alts",
        {"items": [{"alt": "fck", "id": 57, "main": "deadlyenergy"}, {"alt": "orinslc", "id": 56, "main": "orinslc56"}]},
    )
    check("alt mapping accepted", status, 200)
    # merging an alt into a new main leaves the player count unchanged
    check("recalculation reports the merged player count", report["players"], before_mapping["players"])
    _, main = call("GET", "/players/deadlyenergy")
    check("the main inherits the history", (main["login"], main["kills"], main["wins"]), ("deadlyenergy", 20, 1))
    check("the main inherits the rating movement", main["rating"] > 1500.0, True)
    _, aliased = call("GET", "/players/fck")
    check("the alt resolves to the main", aliased["login"], "deadlyenergy")
    _, noalt_after = call("GET", "/noalt/players/fck")
    check("the noalt view is unaffected", (noalt_after["login"], noalt_after["wins"]), ("fck", 1))
    _, alts = call("GET", "/alts")
    check("both mappings stored", alts["total"], 2)

    # a mapping for an account that never played is harmless
    _, events = call("GET", "/events?player=fck&limit=1")
    check("event feed resolves the login", events["items"][0]["killer_resolved"], "deadlyenergy")
    check("event feed keeps the recorded login", events["items"][0]["killer"], "fck")

    status, _ = call("POST", "/alts", {"items": [{"alt": "loop", "main": "loop"}]})
    check("self-mapping rejected", status, 400)
    status, _ = call("DELETE", "/alts/fck")
    check("mapping can be removed", status, 200)
    _, fck_again = call("GET", "/players/fck")
    check("the alt is a standalone player again", fck_again["login"], "fck")
    status, _ = call("GET", "/players/deadlyenergy")
    check("the main is gone once unmapped", status, 404)
    # restore for the remaining checks
    call("POST", "/alts", {"items": [{"alt": "fck", "id": 57, "main": "deadlyenergy"}]})

    # --------------------------------------------------------- pagination + feed
    print("\n-- pagination and the event feed --")
    _, full = call("GET", "/leaderboard")
    _, page1 = call("GET", "/leaderboard/paginated?page=1&limit=5")
    check("paginated total matches the full ladder", page1["total"], len(full))
    check("page size honoured", len(page1["items"]), 5)
    check("first rank is 1", page1["items"][0]["rank"], 1)
    _, page2 = call("GET", "/leaderboard/paginated?page=2&limit=5")
    check("second page continues the ranking", page2["items"][0]["rank"], 6)
    check("pages do not overlap", page1["items"][0]["login"] != page2["items"][0]["login"], True)

    _, feed = call("GET", "/events?limit=10")
    check("feed is newest first", feed["items"][0]["id"], "alt-19")
    _, filtered = call("GET", "/events?player=bob&limit=5")
    check("feed filters by victim too", all(e["killer"] == "fck" or e["victim"] == "bob" for e in filtered["items"]), True)
    status, _ = call("GET", "/events?player=ghost")
    check("unknown filter is a 404", status, 404)
    status, _ = call("GET", "/leaderboard/paginated?limit=99999")
    check("oversized page is rejected", status, 400)

    # -------------------------------------------------------------- matchups
    print("\n-- matchups --")
    _, matchups = call("GET", "/players/deadlyenergy/matchups?limit=100")
    opponents = [m["opponent"] for m in matchups["items"]]
    check("matchups list every opponent", "bob" in opponents, True)
    bob = next(m for m in matchups["items"] if m["opponent"] == "bob")
    check("matchup kills/deaths", (bob["kills"], bob["deaths"]), (20, 0))
    check("matchup kd ratio", bob["kd_ratio"], 20.0)
    # pagination over a player with several opponents (paira faced pairb and pairc)
    _, multi = call("GET", "/players/paira/matchups?limit=1&page=1")
    _, multi2 = call("GET", "/players/paira/matchups?limit=1&page=2")
    check("matchup pagination reports totals", multi["total"], 2)
    check("matchup pagination walks the list", multi["items"][0]["opponent"] != multi2["items"][0]["opponent"], True)
    _, multi3 = call("GET", "/players/paira/matchups?limit=1&page=3")
    check("matchup pagination ends cleanly", multi3["items"], [])

    # ------------------------------------------------------------- swagger ui
    print("\n-- swagger ui --")
    status, doc = call("GET", "/api-docs/openapi.json")
    check("openapi document served", status, 200)
    for path in [
        "/events/import",
        "/leaderboard",
        "/leaderboard/paginated",
        "/players/{login}",
        "/players/{login}/matchups",
        "/players/{login}/matchups/{opponent}",
        "/events",
        "/alts",
        "/noalt/leaderboard",
        "/noalt/leaderboard/paginated",
        "/noalt/players/{login}",
        "/noalt/players/{login}/matchups",
        "/noalt/players/{login}/matchups/{opponent}",
        "/noalt/events",
    ]:
        check(f"openapi documents {path}", path in doc["paths"], True)

    print()
    if FAILURES:
        print(f"{len(FAILURES)} of {CHECKS} checks FAILED")
        for failure in FAILURES:
            print(f"  - {failure}")
        sys.exit(1)
    print(f"all {CHECKS} checks passed")


if __name__ == "__main__":
    main()
