#!/usr/bin/env python3
"""Generates a realistic event history for smoke testing and benchmarking.

Produces a JSON array accepted verbatim by POST /events/import, plus an alt map.
Kills are drawn from a skewed distribution so a few players dominate, and each
event carries the lobby size recorded at that moment, which is what the rating
engine classifies on.
"""

from __future__ import annotations

import argparse
import json
import random
from datetime import datetime, timedelta, timezone

EXCLUDED = (".nobody", ".self")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--events", type=int, default=1_000_000)
    parser.add_argument("--players", type=int, default=500)
    parser.add_argument("--alts", type=int, default=40)
    parser.add_argument("--seed", type=int, default=20260516)
    parser.add_argument("--start", default="2026-01-01T00:00:00Z")
    parser.add_argument("--out", default="bulk-events.json")
    parser.add_argument("--alts-out", default="bulk-alts.json")
    args = parser.parse_args()

    rng = random.Random(args.seed)
    start = datetime.fromisoformat(args.start.replace("Z", "+00:00"))

    players = [f"player{i:04d}" for i in range(args.players)]
    # a power-law-ish skill weight: player0000 kills far more often than player0499
    weights = [1.0 / (i + 1) ** 0.8 for i in range(args.players)]

    # alts are extra logins played by the first `args.alts` mains
    alts = []
    alt_logins = []
    for i in range(args.alts):
        alt = f"smurf{i:03d}"
        alts.append({"alt": alt, "id": 1000 + i, "main": players[i]})
        alt_logins.append(alt)
    logins = players + alt_logins + list(EXCLUDED)
    login_weights = weights + [weights[i % args.players] for i in range(args.alts)] + [0.02, 0.02]

    # lobby size changes slowly: a lobby sits at 2, then grows, then shrinks
    lobby_size = 2
    now = start
    with open(args.out, "w", encoding="utf-8") as handle:
        handle.write("[")
        for i in range(args.events):
            # time advances unevenly, including multi-day gaps per player
            now += timedelta(seconds=rng.choice([1, 2, 3, 5, 8, 13, 30, 90, 3600, 90000]))

            if rng.random() < 0.01:
                lobby_size = rng.choice([2, 2, 2, 3, 4, 6, 9, 16])

            killer = rng.choices(logins, weights=login_weights, k=1)[0]
            victim = rng.choices(logins, weights=login_weights, k=1)[0]
            if victim == killer:
                victim = logins[(logins.index(killer) + 1) % len(logins)]

            if i:
                handle.write(",")
            handle.write(
                json.dumps(
                    {
                        "id": f"e{i}",
                        "time": now.strftime("%Y-%m-%dT%H:%M:%SZ"),
                        "killer": killer,
                        "victim": victim,
                        "player_count": lobby_size,
                    },
                    separators=(",", ":"),
                )
            )
        handle.write("]")

    with open(args.alts_out, "w", encoding="utf-8") as handle:
        json.dump(alts, handle, indent=2)

    print(f"wrote {args.events} events to {args.out} and {len(alts)} alts to {args.alts_out}")


if __name__ == "__main__":
    main()
