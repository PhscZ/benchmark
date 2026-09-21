//! The rating fold: turns a chronological stream of eligible events into
//! ratings, statistics and pairwise match counters.

use std::collections::HashMap;

use crate::glicko;
use crate::model::*;

/// Margin-of-victory scores for a finished race.
///
/// `winScore = 0.5 + 0.5 * (winnerScore - loserScore) / winnerScore`, so a
/// 20-19 win scores 0.525 and a 20-0 win scores 1.0. The loser gets the
/// complement. Returns `(winner, loser)`.
pub fn mov_scores(winner_kills: u64, loser_kills: u64) -> (f64, f64) {
    if winner_kills == 0 {
        return (0.5, 0.5);
    }
    let win = 0.5 + 0.5 * (winner_kills as f64 - loser_kills as f64) / winner_kills as f64;
    (win, 1.0 - win)
}

/// All derived state: per-player ratings/statistics plus per-pair counters.
#[derive(Default)]
pub struct Engine {
    pub players: Vec<PlayerState>,
    /// login -> index into `players`.
    pub ids: HashMap<String, u32>,
    /// canonical pair key -> counters.
    pub pairs: HashMap<u64, PairState>,
    /// player index -> keys of every pair that player belongs to.
    pub adjacency: Vec<Vec<u64>>,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.players.clear();
        self.ids.clear();
        self.pairs.clear();
        self.adjacency.clear();
    }

    pub fn player_id(&mut self, login: &str) -> u32 {
        if let Some(&id) = self.ids.get(login) {
            return id;
        }
        let id = self.players.len() as u32;
        self.players.push(PlayerState::new(login));
        self.adjacency.push(Vec::new());
        self.ids.insert(login.to_string(), id);
        id
    }

    /// Fold one eligible event into the state.
    ///
    /// The event feeds `total` plus its lobby category (1v1 or pub). In each of
    /// those rating systems it: grows the RD of both players by their
    /// inactivity, registers the kill in the pair's match counter and lifetime
    /// head-to-head counter, and - when the kill completes a race to 20 - runs
    /// the Glicko-2 update and resets that pair's match counter for that
    /// category only.
    pub fn apply(&mut self, ev: &Event) {
        if !ev.eligible() {
            return;
        }
        let killer = self.player_id(&ev.killer);
        let victim = self.player_id(&ev.victim);

        // A player cannot race themselves: the kill still counts, nothing else.
        if killer == victim {
            for cat in ev.categories() {
                let s = &mut self.players[killer as usize].cats[cat.index()];
                s.touch(ev.time);
                s.kills += 1;
                s.deaths += 1;
            }
            return;
        }

        for cat in ev.categories() {
            self.apply_category(cat, killer, victim, ev.time);
        }
    }

    fn apply_category(&mut self, cat: Category, killer: u32, victim: u32, time: i64) {
        let ci = cat.index();

        // 1. Reactive inactivity growth, before any rating update this event causes.
        for id in [killer, victim] {
            self.players[id as usize].cats[ci].touch(time);
        }

        // 2. Pair counters (canonical order, so both directions share one entry).
        let (lo, hi) = if killer < victim {
            (killer, victim)
        } else {
            (victim, killer)
        };
        let key = pair_key(lo, hi);
        let mut pair = match self.pairs.get(&key) {
            Some(p) => *p,
            None => {
                self.adjacency[lo as usize].push(key);
                self.adjacency[hi as usize].push(key);
                PairState::default()
            }
        };
        let killer_is_lo = killer == lo;
        let (killer_kills, victim_kills) = if killer_is_lo {
            (pair.match_lo[ci], pair.match_hi[ci])
        } else {
            (pair.match_hi[ci], pair.match_lo[ci])
        };
        if killer_is_lo {
            pair.kills_lo[ci] += 1;
            pair.match_lo[ci] += 1;
        } else {
            pair.kills_hi[ci] += 1;
            pair.match_hi[ci] += 1;
        }

        // 3. Lifetime statistics never reset.
        self.players[killer as usize].cats[ci].kills += 1;
        self.players[victim as usize].cats[ci].deaths += 1;

        // 4. A completed race scores both players and resets only this pair.
        if pair.match_lo[ci] >= MATCH_WIN_KILLS || pair.match_hi[ci] >= MATCH_WIN_KILLS {
            let winner_is_lo = pair.match_lo[ci] >= MATCH_WIN_KILLS;
            let (winner, loser) = if winner_is_lo { (lo, hi) } else { (hi, lo) };
            let (winner_kills, loser_kills) = if winner_is_lo {
                (pair.match_lo[ci] as u64, pair.match_hi[ci] as u64)
            } else {
                (pair.match_hi[ci] as u64, pair.match_lo[ci] as u64)
            };
            let (win_score, loss_score) = mov_scores(winner_kills, loser_kills);

            self.players[winner as usize].cats[ci].wins += 1;
            self.players[loser as usize].cats[ci].losses += 1;

            // Both players are scored against the opponent's pre-update state.
            let winner_rating = self.players[winner as usize].cats[ci].rating();
            let loser_rating = self.players[loser as usize].cats[ci].rating();
            let new_winner = glicko::update_period(winner_rating, &[(loser_rating, win_score)], TAU, EPSILON);
            let new_loser = glicko::update_period(loser_rating, &[(winner_rating, loss_score)], TAU, EPSILON);
            self.players[winner as usize].cats[ci].set(new_winner);
            self.players[loser as usize].cats[ci].set(new_loser);

            pair.match_lo[ci] = 0;
            pair.match_hi[ci] = 0;
            let _ = (killer_kills, victim_kills);
        }

        self.pairs.insert(key, pair);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(time: i64, killer: &str, victim: &str, players: u32) -> Event {
        Event {
            id: format!("{time}:{killer}:{victim}"),
            time,
            killer: killer.into(),
            victim: victim.into(),
            player_count: players,
            seq: 0,
        }
    }

    fn kills(e: &Engine, login: &str, cat: Category) -> u64 {
        e.players[e.ids[login] as usize].cats[cat.index()].kills
    }

    fn stats(e: &Engine, login: &str, cat: Category) -> CatStats {
        e.players[e.ids[login] as usize].cats[cat.index()].clone()
    }

    #[test]
    fn margin_of_victory_scores() {
        let (win, loss) = mov_scores(20, 19);
        assert!((win - 0.525).abs() < 1e-12, "{win}");
        assert!((loss - 0.475).abs() < 1e-12);
        let (win, loss) = mov_scores(20, 0);
        assert!((win - 1.0).abs() < 1e-12);
        assert!(loss.abs() < 1e-12);
        let (win, _) = mov_scores(20, 10);
        assert!((win - 0.75).abs() < 1e-12);
    }

    #[test]
    fn single_kill_only_moves_counters() {
        let mut e = Engine::new();
        e.apply(&ev(1_000, "a", "b", 2));

        let a = stats(&e, "a", Category::Total);
        assert_eq!(a.kills, 1);
        assert_eq!(a.deaths, 0);
        assert_eq!(a.rating, DEFAULT_RATING);
        assert_eq!(a.rd, DEFAULT_RD, "no rating update before a match ends");
        assert_eq!(a.last_active, Some(1_000));
        assert_eq!(stats(&e, "b", Category::Total).deaths, 1);
        assert_eq!(kills(&e, "a", Category::Duel), 1);
        assert_eq!(kills(&e, "a", Category::Pub), 0);
    }

    #[test]
    fn twenty_kills_complete_a_match_and_reset_only_that_pair() {
        let mut e = Engine::new();
        // a races b to 20 in a duel lobby while c is a separate opponent.
        for i in 0..20 {
            e.apply(&ev(1_000 + i, "a", "b", 2));
        }
        e.apply(&ev(2_000, "a", "c", 2));

        let a = stats(&e, "a", Category::Duel);
        assert_eq!(a.wins, 1, "race to 20 is one win");
        assert_eq!(a.losses, 0);
        assert_eq!(a.kills, 21, "lifetime kills survive the match reset");
        assert!(a.rating > 1600.0, "rating {}", a.rating);
        assert!(a.rd < DEFAULT_RD);

        let b = stats(&e, "b", Category::Duel);
        assert_eq!(b.losses, 1);
        assert_eq!(b.deaths, 20);
        assert!(b.rating < 1400.0, "rating {}", b.rating);

        // a vs b match counter is back to zero, a vs c is at one.
        let ab = e.pairs[&pair_key(e.ids["a"], e.ids["b"])];
        assert_eq!(ab.match_lo[Category::Duel.index()], 0);
        assert_eq!(ab.match_hi[Category::Duel.index()], 0);
        assert_eq!(ab.kills_lo[Category::Duel.index()], 20, "head-to-head is lifetime");
        let ac = e.pairs[&pair_key(e.ids["a"], e.ids["c"])];
        assert_eq!(ac.match_lo[Category::Duel.index()], 1);

        // The win was scored 20-0, the maximum margin.
        assert_eq!(a.wins, 1);
        assert_eq!(e.players[e.ids["c"] as usize].cats[Category::Duel.index()].kills, 0);
    }

    /// The margin of victory must move ratings monotonically: a 20-0 win is
    /// worth more than a 20-1 win, which is worth more than a 20-19 win.
    #[test]
    fn closer_matches_are_worth_less() {
        let duel = |loser_kills: usize| {
            let mut e = Engine::new();
            for i in 0..20 - loser_kills {
                e.apply(&ev(1_000 + i as i64, "a", "b", 2));
            }
            for i in 0..loser_kills {
                e.apply(&ev(2_000 + i as i64, "b", "a", 2));
            }
            for i in 0..loser_kills {
                e.apply(&ev(3_000 + i as i64, "a", "b", 2));
            }
            // Exactly 20 kills for a, `loser_kills` for b.
            let a = stats(&e, "a", Category::Duel);
            let b = stats(&e, "b", Category::Duel);
            assert_eq!(a.wins, 1, "a won the race");
            assert_eq!(b.losses, 1, "b lost the race");
            assert_eq!(a.kills, 20);
            assert_eq!(b.kills, loser_kills as u64);
            (a.rating, b.rating)
        };

        let (blowout_win, blowout_loss) = duel(0);
        let (close_win, close_loss) = duel(19);
        assert!(
            blowout_win > close_win,
            "20-0 ({blowout_win}) must be worth more than 20-19 ({close_win})"
        );
        assert!(close_win > DEFAULT_RATING, "a win always gains: {close_win}");
        assert!(close_loss < DEFAULT_RATING, "a loss always costs: {close_loss}");
        assert!(
            blowout_loss < close_loss,
            "losing 0-20 ({blowout_loss}) must cost more than losing 19-20 ({close_loss})"
        );

        // The near-even race sits just above a coin flip in score terms.
        let (win_score, loss_score) = mov_scores(20, 19);
        assert!((win_score - 0.525).abs() < 1e-12);
        assert!((win_score + loss_score - 1.0).abs() < 1e-12);
        assert!(close_win - DEFAULT_RATING < blowout_win - DEFAULT_RATING);
    }

    #[test]
    fn inactivity_grows_rd_by_whole_days_only() {
        let mut e = Engine::new();
        e.apply(&ev(0, "a", "b", 2));
        assert_eq!(stats(&e, "a", Category::Total).rd, DEFAULT_RD);

        // 23h59m later: still zero whole days.
        let t = 23 * 3600 + 59 * 60;
        e.apply(&ev(t, "a", "b", 2));
        assert_eq!(stats(&e, "a", Category::Total).rd, DEFAULT_RD);
        assert_eq!(stats(&e, "a", Category::Total).last_active, Some(t));

        // 49h after that: two whole days of volatility.
        let t2 = t + 49 * 3600;
        e.apply(&ev(t2, "a", "b", 2));
        let rd = stats(&e, "a", Category::Total).rd;
        let expected = glicko::inflate_rd(DEFAULT_RD, DEFAULT_VOLATILITY, 2);
        assert!((rd - expected).abs() < 1e-9, "{rd} != {expected}");
        assert!(rd > DEFAULT_RD);
        assert_eq!(stats(&e, "a", Category::Total).rating, DEFAULT_RATING, "inactivity must not move rating");
        assert_eq!(stats(&e, "a", Category::Total).sigma, DEFAULT_VOLATILITY, "inactivity must not move volatility");

        // Fractional days are discarded: the next event is 12h later.
        let t3 = t2 + 12 * 3600;
        e.apply(&ev(t3, "a", "b", 2));
        assert_eq!(stats(&e, "a", Category::Total).rd, rd, "23h59m+12h is still zero whole days");
    }

    #[test]
    fn first_event_never_grows_rd() {
        let mut e = Engine::new();
        // Absurdly late first event: no prior activity to measure against.
        e.apply(&ev(1_700_000_000, "a", "b", 3));
        assert_eq!(stats(&e, "a", Category::Pub).rd, DEFAULT_RD);
    }

    #[test]
    fn lobby_growth_switches_category_without_losing_counters() {
        let mut e = Engine::new();
        for i in 0..5 {
            e.apply(&ev(1_000 + i, "a", "b", 2));
        }
        assert_eq!(stats(&e, "a", Category::Duel).kills, 5);
        assert_eq!(stats(&e, "a", Category::Pub).kills, 0);

        // A third player joins: later events are pub, the duel counters stay put.
        for i in 0..3 {
            e.apply(&ev(2_000 + i, "a", "b", 3));
        }
        assert_eq!(stats(&e, "a", Category::Duel).kills, 5);
        assert_eq!(stats(&e, "a", Category::Pub).kills, 3);
        assert_eq!(stats(&e, "a", Category::Total).kills, 8);

        // The lobby shrinks again: the duel race resumes from its stored counter.
        e.apply(&ev(3_000, "a", "b", 2));
        let ab = e.pairs[&pair_key(e.ids["a"], e.ids["b"])];
        assert_eq!(ab.match_lo[Category::Duel.index()], 6);
        assert_eq!(ab.match_lo[Category::Pub.index()], 3);
    }

    #[test]
    fn self_kill_counts_but_never_rates() {
        let mut e = Engine::new();
        e.apply(&ev(10, "a", "a", 2));
        let a = stats(&e, "a", Category::Total);
        assert_eq!(a.kills, 1);
        assert_eq!(a.deaths, 1);
        assert_eq!(a.rating, DEFAULT_RATING);
        assert!(e.pairs.is_empty());
    }

    #[test]
    fn ineligible_events_are_ignored() {
        let mut e = Engine::new();
        e.apply(&ev(10, NOBODY, "b", 2));
        e.apply(&ev(11, "a", NOBODY, 2));
        assert!(e.players.is_empty());
        assert!(e.pairs.is_empty());
    }
}
