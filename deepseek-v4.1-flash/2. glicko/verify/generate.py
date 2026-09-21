"""Generate a deterministic, chronologically ordered event history.

Usage: python generate.py <out.json> [target_events] [players]

Mirrors the shape of a real kill log: duels that grow into pub lobbies, world
deaths attributed to `.nobody`, sessions separated by day-scale gaps, and a few
same-day gaps that must not inflate RD.
"""

import json
import random
import sys

NOBODY = ".nobody"


def generate(target_events=4000, players=30, seed=7):
    rng = random.Random(seed)
    logins = ["player_%02d" % i for i in range(players)]
    # Ids embed the generator parameters so that dumps produced with different
    # targets/player counts never collide when loaded into the same server.
    run = "n%d-p%d-s%d" % (target_events, players, seed)
    # A few players never play together, so matchups are not fully connected.
    events = []
    t = 1_600_000_000

    while len(events) < target_events:
        # How long since the previous session: 0 days, 23h59m, exactly 1-3 days.
        gap = rng.choice([3600, 23 * 3600 + 59 * 60, 24 * 3600, 49 * 3600, 72 * 3600, 5 * 3600])
        t += gap
        lobby = rng.sample(logins, rng.choice([2, 2, 3, 4, 5, 8]))
        session_kills = rng.randint(10, 120)
        # Session-tagged ids so a later session cannot collide with an earlier one.
        tag = len(events)

        for _ in range(session_kills):
            if len(events) >= target_events:
                break
            t += rng.randint(3, 40)
            killer = rng.choice(lobby)
            victim = rng.choice(lobby)
            # World/environment death: recorded, never rated.
            if rng.random() < 0.05:
                victim = NOBODY
            events.append(
                {
                    "id": "%s-%d-%d" % (run, tag, len(events)),
                    "time": t,
                    "killer": killer,
                    "victim": victim,
                    "player_count": len(lobby),
                }
            )
            # A duel that gets crashed by a third player.
            if len(lobby) == 2 and rng.random() < 0.08:
                extra = rng.choice([p for p in logins if p not in lobby])
                lobby.append(extra)
            # Or a player leaves a pub, possibly shrinking it back to a duel.
            elif len(lobby) > 2 and rng.random() < 0.08:
                lobby.pop(rng.randrange(len(lobby)))
    return events


if __name__ == "__main__":
    out = sys.argv[1]
    target = int(sys.argv[2]) if len(sys.argv) > 2 else 4000
    nplayers = int(sys.argv[3]) if len(sys.argv) > 3 else 30
    events = generate(target, nplayers)
    with open(out, "w", encoding="utf-8") as fh:
        json.dump(events, fh)
    print("wrote %d events to %s" % (len(events), out))
