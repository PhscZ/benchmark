"""Independent Glicko-2 oracle written from the specification.

This deliberately shares no code with the Rust implementation: it re-derives
the rating fold from the prose spec so that the two can be compared on the
same event history. Used by `smoke.py`; not part of the deliverable.
"""

import json
import math
from datetime import datetime, timezone

SCALE = 173.7178
EPS = 1e-6
TAU = 0.5
DEFAULT_RATING = 1500.0
DEFAULT_RD = 350.0
DEFAULT_SIGMA = 0.06
NOBODY = ".nobody"
WIN_KILLS = 20
DAY = 86400


def g(phi):
    return 1.0 / math.sqrt(1.0 + 3.0 * phi * phi / (math.pi ** 2))


def expected(gj, mu, muj):
    return 1.0 / (1.0 + math.exp(-gj * (mu - muj)))


def solve_sigma(sigma, phi, v, delta):
    a = math.log(sigma * sigma)

    def f(x):
        ex = math.exp(x)
        return (
            ex * (delta * delta - phi * phi - v - ex)
            / (2.0 * (phi * phi + v + ex) ** 2)
            - (x - a) / (TAU * TAU)
        )

    A = a
    if delta * delta > phi * phi + v:
        B = math.log(delta * delta - phi * phi - v)
    else:
        k = 1.0
        while k < 100 and f(a - k * TAU) < 0:
            k += 1
        B = a - k * TAU
    fA, fB = f(A), f(B)
    for _ in range(200):
        if abs(B - A) <= EPS:
            break
        C = A + (A - B) * fA / (fB - fA)
        fC = f(C)
        if fC * fB <= 0:
            A, fA = B, fB
        else:
            fA /= 2.0
        B, fB = C, fC
    return math.exp(A / 2.0)


def update(rating, rd, sigma, opp_rating, opp_rd, score):
    mu = (rating - 1500.0) / SCALE
    phi = rd / SCALE
    mu_j = (opp_rating - 1500.0) / SCALE
    phi_j = opp_rd / SCALE
    gj = g(phi_j)
    e = expected(gj, mu, mu_j)
    v = 1.0 / (gj * gj * e * (1.0 - e))
    delta = v * gj * (score - e)
    sigma_new = solve_sigma(sigma, phi, v, delta)
    phi_star = math.sqrt(phi * phi + sigma_new * sigma_new)
    phi_new = 1.0 / math.sqrt(1.0 / (phi_star * phi_star) + 1.0 / v)
    mu_new = mu + phi_new * phi_new * gj * (score - e)
    return mu_new * SCALE + 1500.0, phi_new * SCALE, sigma_new


def fmt_time(unix):
    """RFC 3339 in UTC, matching the API's `last_active` format."""
    return datetime.fromtimestamp(unix, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def mov_scores(winner_kills, loser_kills):
    win = 0.5 + 0.5 * (winner_kills - loser_kills) / winner_kills
    return win, 1.0 - win


def new_state():
    return {
        "rating": DEFAULT_RATING,
        "rd": DEFAULT_RD,
        "sigma": DEFAULT_SIGMA,
        "wins": 0,
        "losses": 0,
        "kills": 0,
        "deaths": 0,
        "last": None,
    }


class Oracle:
    """Categories: 'total', '1v1', 'pub'."""

    def __init__(self):
        self.players = {}
        self.pairs = {}  # (lo, hi) -> {"kills": [lo_k, hi_k] per cat, "match": [lo, hi] per cat}
        self.cats = ("total", "1v1", "pub")

    def p(self, login):
        if login not in self.players:
            self.players[login] = {c: new_state() for c in self.cats}
        return self.players[login]

    def apply(self, ev):
        killer, victim = ev["killer"], ev["victim"]
        if killer == NOBODY or victim == NOBODY:
            return
        n = ev["player_count"]
        lobby = "1v1" if n == 2 else ("pub" if n >= 3 else None)
        cats = ["total"] + ([lobby] if lobby else [])
        t = ev["time"]
        k, v = self.p(ev["killer"]), self.p(ev["victim"])

        if killer == victim:
            for c in cats:
                s = k[c]
                if s["last"] is not None:
                    periods = max(0, t - s["last"]) // DAY
                    if periods:
                        phi = s["rd"] / SCALE
                        s["rd"] = math.sqrt(phi * phi + periods * s["sigma"] ** 2) * SCALE
                s["last"] = t
                s["kills"] += 1
                s["deaths"] += 1
            return

        for c in cats:
            for s in (k[c], v[c]):
                if s["last"] is not None:
                    periods = max(0, t - s["last"]) // DAY
                    if periods:
                        phi = s["rd"] / SCALE
                        s["rd"] = math.sqrt(phi * phi + periods * s["sigma"] ** 2) * SCALE
                s["last"] = t

            lo, hi = (killer, victim) if killer < victim else (victim, killer)
            pair = self.pairs.setdefault((lo, hi), {"kills": {}, "match": {}})
            pair["kills"].setdefault(c, [0, 0])
            pair["match"].setdefault(c, [0, 0])
            idx = 0 if killer == lo else 1
            pair["kills"][c][idx] += 1
            pair["match"][c][idx] += 1
            k[c]["kills"] += 1
            v[c]["deaths"] += 1

            mk = pair["match"][c]
            if mk[0] >= WIN_KILLS or mk[1] >= WIN_KILLS:
                w_idx = 0 if mk[0] >= WIN_KILLS else 1
                winner, loser = (lo, hi) if w_idx == 0 else (hi, lo)
                ws, ls = self.p(winner)[c], self.p(loser)[c]
                win_score, loss_score = mov_scores(mk[w_idx], mk[1 - w_idx])
                ws["wins"] += 1
                ls["losses"] += 1
                # Both use the opponent's pre-update state.
                wr = (ws["rating"], ws["rd"], ws["sigma"])
                lr = (ls["rating"], ls["rd"], ls["sigma"])
                nw = update(*wr, lr[0], lr[1], win_score)
                nl = update(*lr, wr[0], wr[1], loss_score)
                ws["rating"], ws["rd"], ws["sigma"] = nw
                ls["rating"], ls["rd"], ls["sigma"] = nl
                pair["match"][c] = [0, 0]

    def digest(self):
        out = []
        for login in sorted(self.players):
            for c in self.cats:
                s = self.players[login][c]
                out.append(
                    "{}|{}|{:.10f}|{:.10f}|{:.10f}|{}|{}|{}|{}|{}".format(
                        login, c, s["rating"], s["rd"], s["sigma"],
                        s["wins"], s["losses"], s["kills"], s["deaths"], s["last"],
                    )
                )
        for (lo, hi) in sorted(self.pairs):
            p = self.pairs[(lo, hi)]
            for c in self.cats:
                kk = p["kills"].get(c, [0, 0])
                mm = p["match"].get(c, [0, 0])
                if kk[0] or kk[1]:
                    out.append("pair|{}|{}|{}|{}|{}|{}|{}".format(lo, hi, c, kk[0], kk[1], mm[0], mm[1]))
        return "\n".join(out)


def fold(events):
    o = Oracle()
    for ev in sorted(events, key=lambda e: (e["time"], e.get("seq", 0))):
        o.apply(ev)
    return o
