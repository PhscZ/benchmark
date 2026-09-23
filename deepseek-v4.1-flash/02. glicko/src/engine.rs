//! The rating engine.
//!
//! One chronological pass over the event log produces a complete derived state:
//! three ladders per player, per-opponent statistics, current match counters and
//! sorted leaderboards. Everything is recomputed from the log, which is what
//! makes late alt-account mappings and out-of-order imports correct rather than
//! approximate.

use crate::glicko::{self, Game, GlickoParams, Rating, SECONDS_PER_DAY};
use crate::model::{Category, StoredEvent};
use std::cmp::Ordering;
use std::collections::HashMap;

/// A race is won by the first player to reach this many kills on the opponent.
pub const MATCH_KILL_TARGET: u32 = 20;

/// Per-category rating, inactivity anchor and cumulative statistics.
#[derive(Debug, Clone, Copy, Default)]
pub struct CatStats {
    pub rating: f64,
    pub rd: f64,
    pub volatility: f64,
    /// Timestamp of the last *eligible* event that touched this category.
    pub last_active: Option<i64>,
    pub wins: u64,
    pub losses: u64,
    /// Cumulative kills; never reset by a completed match.
    pub kills: u64,
    /// Cumulative deaths; never reset by a completed match.
    pub deaths: u64,
}

/// Lifetime head-to-head counters against a single opponent in one category.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OppStats {
    pub kills: u64,
    pub deaths: u64,
}

#[derive(Debug)]
pub struct Player {
    pub cats: [CatStats; 3],
    /// opponent id -> lifetime kills/deaths against them, per category.
    pub opponents: [HashMap<u32, OppStats>; 3],
    /// opponent id -> kills in the *current* race, per category. Reset (for that
    /// pair only) when the race is won.
    pub match_kills: [HashMap<u32, u32>; 3],
}

impl Player {
    fn new(params: &GlickoParams) -> Self {
        let fresh = CatStats {
            rating: params.initial_rating,
            rd: params.initial_rd,
            volatility: params.initial_volatility,
            ..CatStats::default()
        };
        Self {
            cats: [fresh; 3],
            opponents: Default::default(),
            match_kills: Default::default(),
        }
    }
}

/// Everything derived from the event log for one alt-resolution mode.
pub struct Derived {
    /// Indexed by canonical player id; `None` until the player's first eligible event.
    pub players: Vec<Option<Box<Player>>>,
    /// raw login id -> canonical login id. Identity for the no-alt view,
    /// alt-chain collapse for the alt-aware view.
    pub resolve: Vec<u32>,
    /// Registered player ids per category, ordered by rating desc, RD asc, login asc.
    pub leaderboards: [Vec<u32>; 3],
    pub registered: u32,
    /// Compressed sparse row index of the events each player took part in, in
    /// log order, so the filtered event feed is a slice rather than a scan.
    /// `event_indices[event_offsets[p]..event_offsets[p + 1]]`.
    event_offsets: Vec<u32>,
    event_indices: Vec<u32>,
}

/// An empty derived state, used before the first recomputation.
impl Default for Derived {
    fn default() -> Self {
        Self {
            players: Vec::new(),
            resolve: Vec::new(),
            leaderboards: [Vec::new(), Vec::new(), Vec::new()],
            registered: 0,
            // one row offset per (absent) player keeps `events_of` well-formed
            event_offsets: vec![0],
            event_indices: Vec::new(),
        }
    }
}

impl Derived {
    pub fn player(&self, id: u32) -> Option<&Player> {
        self.players.get(id as usize).and_then(|p| p.as_deref())
    }

    pub fn stats(&self, id: u32, cat: Category) -> Option<&CatStats> {
        self.player(id).map(|p| &p.cats[cat.index()])
    }

    /// Collapses a raw login id to its canonical id in this view.
    pub fn canonical(&self, raw: u32) -> u32 {
        self.resolve[raw as usize]
    }

    /// Events this player appeared in, oldest first, as log positions.
    pub fn events_of(&self, id: u32) -> &[u32] {
        let start = self.event_offsets[id as usize] as usize;
        let end = self.event_offsets[id as usize + 1] as usize;
        &self.event_indices[start..end]
    }
}

/// Counters describing what the pass actually did; surfaced in import reports.
#[derive(Debug, Clone, Copy, Default)]
pub struct ComputeStats {
    /// Eligible events applied (after exclusions and self-kill drops).
    pub processed: u64,
    /// Events dropped because killer or victim is an excluded login.
    pub skipped_excluded: u64,
    /// Events dropped because both sides resolve to the same player.
    pub skipped_self: u64,
    /// Completed races (a race can complete in two categories at once).
    pub matches: u64,
}

pub struct ComputeInput<'a> {
    pub events: &'a [StoredEvent],
    /// Event indices in chronological order.
    pub order: &'a [u32],
    /// raw login id -> canonical login id.
    pub resolve: &'a [u32],
    pub names: &'a [String],
    /// raw login id -> excluded from all rating maths and statistics.
    pub excluded: &'a [bool],
    pub params: &'a GlickoParams,
}

fn player_mut<'a>(
    players: &'a mut [Option<Box<Player>>],
    id: u32,
    params: &GlickoParams,
) -> &'a mut Player {
    let slot = &mut players[id as usize];
    if slot.is_none() {
        *slot = Some(Box::new(Player::new(params)));
    }
    slot.as_mut().expect("just initialized")
}

/// Replays the whole log and returns the derived state.
pub fn compute(input: &ComputeInput<'_>) -> (Derived, ComputeStats) {
    let params = input.params;
    let mut players: Vec<Option<Box<Player>>> = (0..input.names.len()).map(|_| None).collect();
    let mut stats = ComputeStats::default();

    for &event_idx in input.order {
        let event = &input.events[event_idx as usize];
        let killer_raw = event.killer;
        let victim_raw = event.victim;

        if input.excluded[killer_raw as usize] || input.excluded[victim_raw as usize] {
            stats.skipped_excluded += 1;
            continue;
        }
        let killer = input.resolve[killer_raw as usize];
        let victim = input.resolve[victim_raw as usize];
        if killer == victim {
            // Killing your own alt is not a race against yourself.
            stats.skipped_self += 1;
            continue;
        }
        stats.processed += 1;

        let (first, second) = Category::for_player_count(event.player_count);
        for cat in std::iter::once(first).chain(second) {
            let ci = cat.index();

            // --- reactive inactivity inflation, before any rating change ---
            for player_id in [killer, victim] {
                let player = player_mut(&mut players, player_id, params);
                let cat_stats = &mut player.cats[ci];
                if let Some(last_active) = cat_stats.last_active {
                    let periods = (event.time_secs - last_active).div_euclid(SECONDS_PER_DAY);
                    if periods > 0 {
                        cat_stats.rd = glicko::inflate_rd(
                            cat_stats.rd,
                            cat_stats.volatility,
                            periods,
                            params.scale,
                        );
                    }
                }
                // Whole days are discarded, never carried over.
                cat_stats.last_active = Some(event.time_secs);
            }

            // --- cumulative kills/deaths + lifetime head-to-head ---
            let match_kills = {
                let player = player_mut(&mut players, killer, params);
                player.cats[ci].kills += 1;
                player.opponents[ci].entry(victim).or_default().kills += 1;
                let count = player.match_kills[ci].entry(victim).or_insert(0);
                *count += 1;
                *count
            };
            {
                let player = player_mut(&mut players, victim, params);
                player.cats[ci].deaths += 1;
                player.opponents[ci].entry(killer).or_default().deaths += 1;
            }

            // --- race complete? ---
            if match_kills >= MATCH_KILL_TARGET {
                let loser_score = players[victim as usize]
                    .as_ref()
                    .expect("victim is registered")
                    .match_kills[ci]
                    .get(&killer)
                    .copied()
                    .unwrap_or(0);
                let (win_score, loss_score) = glicko::margin_of_victory(match_kills, loser_score);

                // Both updates read pre-match values.
                let winner_before = players[killer as usize].as_ref().expect("registered").cats[ci];
                let loser_before = players[victim as usize].as_ref().expect("registered").cats[ci];
                let winner_after = glicko::update(
                    Rating {
                        rating: winner_before.rating,
                        rd: winner_before.rd,
                        volatility: winner_before.volatility,
                    },
                    &[Game {
                        opponent_rating: loser_before.rating,
                        opponent_rd: loser_before.rd,
                        score: win_score,
                    }],
                    params,
                );
                let loser_after = glicko::update(
                    Rating {
                        rating: loser_before.rating,
                        rd: loser_before.rd,
                        volatility: loser_before.volatility,
                    },
                    &[Game {
                        opponent_rating: winner_before.rating,
                        opponent_rd: winner_before.rd,
                        score: loss_score,
                    }],
                    params,
                );

                {
                    let player = player_mut(&mut players, killer, params);
                    let cat_stats = &mut player.cats[ci];
                    cat_stats.rating = winner_after.rating;
                    cat_stats.rd = winner_after.rd;
                    cat_stats.volatility = winner_after.volatility;
                    cat_stats.wins += 1;
                    // Only this pair's race counters reset; other opponents keep theirs.
                    player.match_kills[ci].remove(&victim);
                }
                {
                    let player = player_mut(&mut players, victim, params);
                    let cat_stats = &mut player.cats[ci];
                    cat_stats.rating = loser_after.rating;
                    cat_stats.rd = loser_after.rd;
                    cat_stats.volatility = loser_after.volatility;
                    cat_stats.losses += 1;
                    player.match_kills[ci].remove(&killer);
                }
                stats.matches += 1;
            }
        }
    }

    // --- leaderboards ---
    let mut leaderboards: [Vec<u32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (id, slot) in players.iter().enumerate() {
        if let Some(player) = slot {
            for (ci, board) in leaderboards.iter_mut().enumerate() {
                if player.cats[ci].last_active.is_some() {
                    board.push(id as u32);
                }
            }
        }
    }
    for (ci, board) in leaderboards.iter_mut().enumerate() {
        board.sort_unstable_by(|&a, &b| {
            let left = players[a as usize].as_ref().expect("board member").cats[ci];
            let right = players[b as usize].as_ref().expect("board member").cats[ci];
            right
                .rating
                .partial_cmp(&left.rating)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.rd.partial_cmp(&right.rd).unwrap_or(Ordering::Equal))
                .then_with(|| input.names[a as usize].cmp(&input.names[b as usize]))
        });
    }

    let registered = players.iter().filter(|p| p.is_some()).count() as u32;
    let (event_offsets, event_indices) = build_event_index(input);

    (
        Derived {
            players,
            resolve: input.resolve.to_vec(),
            leaderboards,
            registered,
            event_offsets,
            event_indices,
        },
        stats,
    )
}

/// Builds the compressed sparse row index of `player -> event positions`.
///
/// Covers every event, eligible or not, because the feed shows the raw log.
/// A player who appears on both sides of one event (a self-kill that survived
/// alt resolution) is indexed once.
fn build_event_index(input: &ComputeInput<'_>) -> (Vec<u32>, Vec<u32>) {
    let player_count = input.names.len();
    let mut offsets = vec![0u32; player_count + 1];
    for event in input.events {
        let killer = input.resolve[event.killer as usize];
        let victim = input.resolve[event.victim as usize];
        offsets[killer as usize + 1] += 1;
        if victim != killer {
            offsets[victim as usize + 1] += 1;
        }
    }
    for index in 0..player_count {
        offsets[index + 1] += offsets[index];
    }

    let mut indices = vec![0u32; offsets[player_count] as usize];
    let mut cursor = offsets.clone();
    for (position, event) in input.events.iter().enumerate() {
        let position = position as u32;
        let killer = input.resolve[event.killer as usize];
        indices[cursor[killer as usize] as usize] = position;
        cursor[killer as usize] += 1;
        let victim = input.resolve[event.victim as usize];
        if victim != killer {
            indices[cursor[victim as usize] as usize] = position;
            cursor[victim as usize] += 1;
        }
    }
    (offsets, indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Interner;
    use chrono::DateTime;

    /// Builds an event log and runs the engine over it, with or without alts.
    struct Harness {
        interner: Interner,
        events: Vec<StoredEvent>,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                interner: Interner::default(),
                events: Vec::new(),
            }
        }

        fn intern(&mut self, login: &str) -> u32 {
            self.interner.intern(login)
        }

        fn push(&mut self, killer: &str, victim: &str, player_count: u32, secs: i64) {
            let killer = self.interner.intern(killer);
            let victim = self.interner.intern(victim);
            self.events.push(StoredEvent {
                id: format!("e{}", self.events.len()),
                time: DateTime::from_timestamp(secs, 0).unwrap(),
                time_secs: secs,
                killer,
                victim,
                player_count,
            });
        }

        fn run(&self, params: &GlickoParams) -> (Derived, ComputeStats) {
            self.run_with(&[], params)
        }

        /// `alts` are `(alt, main)` pairs, applied as a raw -> canonical mapping.
        fn run_with(
            &self,
            alts: &[(&str, &str)],
            params: &GlickoParams,
        ) -> (Derived, ComputeStats) {
            let order: Vec<u32> = (0..self.events.len() as u32).collect();
            let mut resolve: Vec<u32> = (0..self.interner.len() as u32).collect();
            for (alt, main) in alts {
                let alt = self.interner.get(alt).expect("alt is interned");
                let main = self.interner.get(main).expect("main is interned");
                resolve[alt as usize] = main;
            }
            let excluded: Vec<bool> = self
                .interner
                .names()
                .iter()
                .map(|name| matches!(name.as_str(), ".nobody" | ".self"))
                .collect();
            compute(&ComputeInput {
                events: &self.events,
                order: &order,
                resolve: &resolve,
                names: self.interner.names(),
                excluded: &excluded,
                params,
            })
        }

        fn id(&self, login: &str) -> u32 {
            self.interner.get(login).expect("interned")
        }
    }

    fn params() -> GlickoParams {
        GlickoParams::default()
    }

    #[test]
    fn race_to_twenty_completes_a_match() {
        let mut h = Harness::new();
        for i in 0..19 {
            h.push("a", "b", 2, 1_000 + i);
        }
        let (derived, stats) = h.run(&params());
        assert_eq!(stats.matches, 0, "19 kills is not a win");
        let a = *derived.stats(h.id("a"), Category::OneVsOne).unwrap();
        assert_eq!((a.wins, a.kills), (0, 19));
        assert_eq!(a.rating, 1500.0);

        h.push("a", "b", 2, 1_020);
        let (derived, stats) = h.run(&params());
        assert_eq!(stats.matches, 2, "one race, completed in total and 1v1");
        let a = *derived.stats(h.id("a"), Category::OneVsOne).unwrap();
        let b = *derived.stats(h.id("b"), Category::OneVsOne).unwrap();
        assert_eq!((a.wins, a.losses), (1, 0));
        assert_eq!((b.wins, b.losses), (0, 1));
        assert!(a.rating > 1500.0 && b.rating < 1500.0);
        // cumulative counters survive the reset
        assert_eq!((a.kills, b.deaths), (20, 20));
        // but the race counter for that pair is cleared
        assert!(derived.player(h.id("a")).unwrap().match_kills[1].is_empty());
    }

    #[test]
    fn reset_is_per_pair_only() {
        let mut h = Harness::new();
        for i in 0..19 {
            h.push("a", "b", 2, 1_000 + i);
            h.push("a", "c", 2, 1_000 + i);
        }
        h.push("a", "b", 2, 1_100); // a wins the race against b
        let (derived, _) = h.run(&params());
        let a = derived.player(h.id("a")).unwrap();
        assert!(
            !a.match_kills[1].contains_key(&h.id("b")),
            "pair a->b reset"
        );
        assert_eq!(
            a.match_kills[1].get(&h.id("c")).copied(),
            Some(19),
            "pair a->c untouched"
        );
    }

    #[test]
    fn categories_are_independent_ladders() {
        let mut h = Harness::new();
        for i in 0..20 {
            h.push("a", "b", 3, 1_000 + i); // pub only
        }
        let (derived, stats) = h.run(&params());
        assert_eq!(stats.matches, 2, "the race completes in total and in pub");
        let a = derived.stats(h.id("a"), Category::Total).unwrap();
        assert_eq!(a.wins, 1);
        assert_eq!(derived.stats(h.id("a"), Category::Pub).unwrap().wins, 1);
        let duel = derived.stats(h.id("a"), Category::OneVsOne).unwrap();
        assert_eq!(duel.wins, 0);
        assert!(
            duel.last_active.is_none(),
            "1v1 ladder untouched by pub events"
        );
        assert_eq!(duel.rating, 1500.0);
    }

    #[test]
    fn lobby_size_changes_switch_category_without_resetting_counters() {
        let mut h = Harness::new();
        for i in 0..15 {
            h.push("a", "b", 2, 1_000 + i);
        }
        for i in 0..5 {
            h.push("a", "b", 4, 2_000 + i); // a third player joined the lobby
        }
        let (derived, _) = h.run(&params());
        let a = derived.player(h.id("a")).unwrap();
        // the 1v1 race stalls at 15 kills and is kept
        assert_eq!(a.match_kills[1].get(&h.id("b")).copied(), Some(15));
        // pub keeps its own counter
        assert_eq!(a.match_kills[2].get(&h.id("b")).copied(), Some(5));
        // total sees all 20 and completes
        assert!(a.match_kills[0].is_empty());
        assert_eq!((a.cats[0].wins, a.cats[1].wins, a.cats[2].wins), (1, 0, 0));
    }

    #[test]
    fn excluded_logins_are_ignored_entirely() {
        let mut h = Harness::new();
        for i in 0..25 {
            h.push(".nobody", "a", 2, 1_000 + i);
        }
        h.push("a", ".self", 2, 2_000);
        let (derived, stats) = h.run(&params());
        assert_eq!(stats.processed, 0);
        assert_eq!(stats.skipped_excluded, 26);
        assert_eq!(derived.registered, 0);
        assert!(derived.stats(h.id("a"), Category::Total).is_none());
    }

    #[test]
    fn inactivity_grows_rd_reactively_before_the_event() {
        let mut h = Harness::new();
        h.push("a", "b", 2, 0);
        // 23h59m later: less than a whole period, no growth
        h.push("a", "b", 2, 23 * 3600 + 59 * 60);
        // 49h after that: two whole periods
        h.push("a", "b", 2, 23 * 3600 + 59 * 60 + 49 * 3600);
        let (derived, _) = h.run(&params());
        let p = params();
        let expected = glicko::inflate_rd(p.initial_rd, p.initial_volatility, 2, p.scale);
        let a = derived.stats(h.id("a"), Category::Total).unwrap();
        assert!(
            (a.rd - expected).abs() < 1e-9,
            "rd {} expected {}",
            a.rd,
            expected
        );
        assert!(a.rd > p.initial_rd);
        // rating and volatility are untouched by inactivity
        assert_eq!(a.rating, p.initial_rating);
        assert_eq!(a.volatility, p.initial_volatility);
    }

    #[test]
    fn inactivity_uses_the_players_own_volatility() {
        let mut h = Harness::new();
        h.push("a", "b", 2, 0);
        h.push("a", "b", 2, 3 * SECONDS_PER_DAY);
        let (derived, _) = h.run(&params());
        let p = params();
        let a = derived.stats(h.id("a"), Category::Total).unwrap();
        let expected = glicko::inflate_rd(p.initial_rd, p.initial_volatility, 3, p.scale);
        assert!((a.rd - expected).abs() < 1e-9);
        // the volatility itself is never modified by inactivity
        assert_eq!(a.volatility, p.initial_volatility);
    }

    #[test]
    fn close_win_moves_less_than_a_dominant_win() {
        let mut close = Harness::new();
        for i in 0..19 {
            close.push("a", "b", 2, 1_000 + 2 * i);
            close.push("b", "a", 2, 1_001 + 2 * i);
        }
        close.push("a", "b", 2, 2_000);
        let (close_derived, _) = close.run(&params());
        let close_gain = close_derived
            .stats(close.id("a"), Category::Total)
            .unwrap()
            .rating
            - 1500.0;
        let close_drop = 1500.0
            - close_derived
                .stats(close.id("b"), Category::Total)
                .unwrap()
                .rating;

        let mut dominant = Harness::new();
        for i in 0..20 {
            dominant.push("a", "b", 2, 1_000 + i);
        }
        let (dominant_derived, _) = dominant.run(&params());
        let dominant_gain = dominant_derived
            .stats(dominant.id("a"), Category::Total)
            .unwrap()
            .rating
            - 1500.0;

        assert!(close_gain > 0.0 && dominant_gain > 0.0);
        // a 20-19 result sits close to a draw, so it must move far less than a 20-0
        assert!(
            close_gain < dominant_gain / 4.0,
            "20-19 gain {close_gain} should be a small fraction of the 20-0 gain {dominant_gain}"
        );
        assert!(
            close_drop < close_gain * 1.5 && close_drop > close_gain * 0.5,
            "both sides of a 20-19 should move by a similar, small amount"
        );
    }

    #[test]
    fn alt_mapping_merges_kills_and_recomputes_the_race() {
        let mut h = Harness::new();
        h.intern("deadlyenergy"); // the main is known before any event mentions it
        for i in 0..10 {
            h.push("fck", "bob", 2, 1_000 + i);
        }
        for i in 0..10 {
            h.push("deadlyenergy", "bob", 2, 2_000 + i);
        }

        let (split, stats) = h.run(&params());
        assert_eq!(stats.matches, 0, "neither identity reaches 20 kills alone");
        assert_eq!(split.stats(h.id("fck"), Category::Total).unwrap().kills, 10);
        assert_eq!(
            split
                .stats(h.id("deadlyenergy"), Category::Total)
                .unwrap()
                .kills,
            10
        );

        let (merged, stats) = h.run_with(&[("fck", "deadlyenergy")], &params());
        assert_eq!(stats.matches, 2, "merged identity completes the race");
        let main = merged.stats(h.id("deadlyenergy"), Category::Total).unwrap();
        assert_eq!((main.kills, main.wins), (20, 1));
        assert!(main.rating > 1500.0);
        assert!(
            merged.stats(h.id("fck"), Category::Total).is_none(),
            "alt is not a player of its own"
        );
        assert_eq!(merged.registered, 2, "deadlyenergy + bob");
    }

    #[test]
    fn self_kills_after_alt_resolution_are_dropped() {
        let mut h = Harness::new();
        h.intern("deadlyenergy");
        for i in 0..20 {
            h.push("fck", "deadlyenergy", 2, 1_000 + i);
        }
        let (derived, stats) = h.run_with(&[("fck", "deadlyenergy")], &params());
        assert_eq!(stats.skipped_self, 20);
        assert_eq!(stats.processed, 0);
        assert_eq!(derived.registered, 0);
    }

    #[test]
    fn the_event_index_lists_each_players_events_once_in_log_order() {
        let mut h = Harness::new();
        h.intern("deadlyenergy");
        h.intern("benchwarmer"); // known to the interner, never in an event
        h.push("a", "b", 2, 1_000);
        h.push("b", "a", 2, 1_100);
        h.push("a", "c", 2, 1_200);
        // a self-kill that only becomes one after alt resolution
        h.push("fck", "deadlyenergy", 2, 1_300);
        h.push("c", "a", 2, 1_400);

        let (derived, _) = h.run_with(&[("fck", "deadlyenergy")], &params());
        let positions = |login: &str| -> Vec<u32> {
            let id = derived.canonical(h.id(login));
            derived.events_of(id).to_vec()
        };

        assert_eq!(
            positions("a"),
            vec![0, 1, 2, 4],
            "oldest first, one entry per event"
        );
        assert_eq!(positions("b"), vec![0, 1]);
        assert_eq!(positions("c"), vec![2, 4]);
        // the merged identity appears once for its self-kill, not twice
        assert_eq!(positions("deadlyenergy"), vec![3]);
        assert_eq!(positions("fck"), positions("deadlyenergy"));
        // a known login with no events has an empty row
        assert!(positions("benchwarmer").is_empty());
    }

    #[test]
    fn leaderboard_is_ordered_by_rating() {
        let mut h = Harness::new();
        // a beats b three times in a row; c never wins
        for round in 0..3 {
            for i in 0..20 {
                h.push("a", "b", 2, 1_000 + round * 100 + i);
            }
        }
        for i in 0..20 {
            h.push("b", "c", 2, 10_000 + i);
        }
        let (derived, _) = h.run(&params());
        let board = &derived.leaderboards[Category::Total.index()];
        let names: Vec<&str> = board.iter().map(|&id| h.interner.name(id)).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        let ratings: Vec<f64> = board
            .iter()
            .map(|&id| derived.stats(id, Category::Total).unwrap().rating)
            .collect();
        assert!(ratings[0] > ratings[1] && ratings[1] > ratings[2]);
    }
}
