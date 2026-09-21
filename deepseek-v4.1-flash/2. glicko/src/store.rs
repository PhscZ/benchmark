//! Event log plus derived state.
//!
//! Events are kept sorted by `(time, seq)`. A prefix of that vector is already
//! folded into the engine (`applied`); an import that only appends to the end
//! folds the new tail, while anything that rewrites or inserts into the applied
//! prefix replays the log from scratch. That keeps imports O(new events) in the
//! normal case and always correct for late-arriving history.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::engine::Engine;
use crate::model::*;

/// Cache of the sorted leaderboard for each category, keyed by store version.
struct BoardCache {
    version: u64,
    rows: Arc<Vec<LeaderRow>>,
}

#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    pub received: usize,
    /// Events newly added to the log.
    pub imported: usize,
    /// Events whose id was already present with identical content.
    pub skipped: usize,
    /// Events whose id was present with different content (replaced).
    pub updated: usize,
    pub events_total: usize,
    pub players: usize,
    /// True when the applied prefix had to be replayed.
    pub rebuilt: bool,
}

pub struct Store {
    engine: Engine,
    events: Vec<Event>,
    by_id: HashMap<String, usize>,
    /// Number of leading `events` already folded into the engine.
    applied: usize,
    next_seq: u64,
    version: u64,
    boards: Mutex<[Option<BoardCache>; CATEGORY_COUNT]>,
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl Store {
    pub fn new() -> Self {
        Self {
            engine: Engine::new(),
            events: Vec::new(),
            by_id: HashMap::new(),
            applied: 0,
            next_seq: 0,
            version: 0,
            boards: Mutex::new(Default::default()),
        }
    }

    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    pub fn player_count(&self) -> usize {
        self.engine.players.len()
    }

    pub fn player_id(&self, login: &str) -> Option<u32> {
        self.engine.ids.get(login).copied()
    }

    // -- ingestion ---------------------------------------------------------

    /// Register a batch of events. Ids make the operation idempotent.
    pub fn import(&mut self, inputs: Vec<EventInput>) -> Result<ImportReport, String> {
        let mut report = ImportReport {
            received: inputs.len(),
            ..Default::default()
        };
        let applied_before = self.applied;

        // Validate the whole batch before touching any state, so a bad payload
        // cannot leave the log half-imported.
        for (i, input) in inputs.iter().enumerate() {
            if input.id.is_empty() {
                return Err(format!("event[{i}]: `id` must not be empty"));
            }
            if input.killer.is_empty() || input.victim.is_empty() {
                return Err(format!("event[{i}] ({}): `killer` and `victim` are required", input.id));
            }
        }

        let mut pending: Vec<Event> = Vec::new();
        let mut pending_ids: HashMap<String, usize> = HashMap::new();
        // Lowest index of an event replaced in place, and of an event inserted
        // into the middle of the log. Anything below `applied` forces a replay.
        let mut first_replaced = usize::MAX;
        let mut first_inserted = usize::MAX;

        for input in inputs {
            if let Some(&idx) = self.by_id.get(&input.id) {
                if input.same_as(&self.events[idx]) {
                    report.skipped += 1;
                } else {
                    let seq = self.events[idx].seq;
                    self.events[idx] = input.into_event(seq);
                    first_replaced = first_replaced.min(idx);
                    report.updated += 1;
                }
                continue;
            }
            if let Some(&p) = pending_ids.get(&input.id) {
                let same = {
                    let e = &pending[p];
                    input.time == e.time
                        && input.killer == e.killer
                        && input.victim == e.victim
                        && input.player_count == e.player_count
                };
                if same {
                    report.skipped += 1;
                } else {
                    let seq = pending[p].seq;
                    pending[p] = input.into_event(seq);
                    report.updated += 1;
                }
                continue;
            }

            let seq = self.next_seq;
            self.next_seq += 1;
            pending_ids.insert(input.id.clone(), pending.len());
            pending.push(input.into_event(seq));
            report.imported += 1;
        }

        // A replacement may have moved an event out of timestamp order.
        if first_replaced != usize::MAX && !is_sorted(&self.events) {
            self.events.sort_by_key(|e| (e.time, e.seq));
            first_replaced = 0;
        }

        let mut appended = true;
        if !pending.is_empty() {
            let (index, was_append) = self.merge_pending(pending);
            first_inserted = index;
            appended = was_append;
        }

        if appended {
            // Existing indices are untouched, so only the new tail needs indexing.
            if first_inserted != usize::MAX {
                for (offset, event) in self.events[first_inserted..].iter().enumerate() {
                    self.by_id.insert(event.id.clone(), first_inserted + offset);
                }
            }
        } else if self.by_id.len() != self.events.len() {
            self.reindex();
        }

        if first_replaced.min(first_inserted) < applied_before {
            self.engine.clear();
            self.applied = 0;
            report.rebuilt = true;
        }
        self.fold_tail();

        report.events_total = self.events.len();
        report.players = self.engine.players.len();
        if report.imported + report.updated > 0 {
            self.version += 1;
        }
        Ok(report)
    }

    /// Fold every event that is not applied yet.
    fn fold_tail(&mut self) {
        if self.applied >= self.events.len() {
            return;
        }
        let events = std::mem::take(&mut self.events);
        for event in &events[self.applied..] {
            self.engine.apply(event);
        }
        self.applied = events.len();
        self.events = events;
    }

    /// Insert `pending` (unsorted) into the sorted log.
    /// Returns the index of the first inserted event and whether it was a pure append.
    fn merge_pending(&mut self, mut pending: Vec<Event>) -> (usize, bool) {
        pending.sort_by_key(|e| (e.time, e.seq));
        let at_end = self
            .events
            .last()
            .map_or(true, |last| (last.time, last.seq) <= (pending[0].time, pending[0].seq));
        if at_end {
            let at = self.events.len();
            self.events.extend(pending);
            return (at, true);
        }

        let capacity = self.events.len() + pending.len();
        let mut old = std::mem::take(&mut self.events).into_iter().peekable();
        let mut new = pending.into_iter().peekable();
        let mut merged = Vec::with_capacity(capacity);
        let mut first_new = usize::MAX;
        loop {
            let take_old = match (old.peek(), new.peek()) {
                (Some(o), Some(n)) => (o.time, o.seq) <= (n.time, n.seq),
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => break,
            };
            if take_old {
                merged.push(old.next().expect("peeked"));
            } else {
                first_new = first_new.min(merged.len());
                merged.push(new.next().expect("peeked"));
            }
        }
        self.events = merged;
        (first_new, false)
    }

    fn reindex(&mut self) {
        self.by_id.clear();
        self.by_id.reserve(self.events.len());
        for (i, e) in self.events.iter().enumerate() {
            self.by_id.insert(e.id.clone(), i);
        }
    }

    // -- queries -----------------------------------------------------------

    /// Sorted leaderboard for a category, cached until the next mutation.
    /// Only players with at least one eligible event in that category appear.
    pub fn leaderboard(&self, cat: Category) -> (Arc<Vec<LeaderRow>>, usize) {
        let mut boards = self.boards.lock().unwrap_or_else(|e| e.into_inner());
        let slot = &mut boards[cat.index()];
        let rows = match slot {
            Some(cache) if cache.version == self.version => cache.rows.clone(),
            _ => {
                let ci = cat.index();
                let mut rows: Vec<LeaderRow> = self
                    .engine
                    .players
                    .iter()
                    .filter(|p| p.cats[ci].last_active.is_some())
                    .map(|p| LeaderRow {
                        rank: 0,
                        login: p.login.clone(),
                        stats: p.cats[ci].view(),
                    })
                    .collect();
                rows.sort_by(|a, b| {
                    b.stats
                        .rating
                        .partial_cmp(&a.stats.rating)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.login.cmp(&b.login))
                });
                for (i, row) in rows.iter_mut().enumerate() {
                    row.rank = i + 1;
                }
                let rows = Arc::new(rows);
                *slot = Some(BoardCache {
                    version: self.version,
                    rows: rows.clone(),
                });
                rows
            }
        };
        let total = rows.len();
        (rows, total)
    }

    pub fn player_stats(&self, id: u32) -> [StatsView; CATEGORY_COUNT] {
        let p = &self.engine.players[id as usize];
        [
            p.cats[Category::Total.index()].view(),
            p.cats[Category::Duel.index()].view(),
            p.cats[Category::Pub.index()].view(),
        ]
    }

    /// Kills and deaths of `id` against every opponent it has faced, in one category.
    pub fn matchups(&self, id: u32, cat: Category) -> Vec<MatchupRow> {
        let ci = cat.index();
        let mut rows: Vec<MatchupRow> = self.engine.adjacency[id as usize]
            .iter()
            .filter_map(|key| {
                let pair = self.engine.pairs.get(key)?;
                let (lo, hi) = pair_key_parts(*key);
                let (kills, deaths) = if id == lo {
                    (pair.kills_lo[ci], pair.kills_hi[ci])
                } else {
                    (pair.kills_hi[ci], pair.kills_lo[ci])
                };
                if kills == 0 && deaths == 0 {
                    return None;
                }
                let opponent = if id == lo { hi } else { lo };
                Some(MatchupRow {
                    opponent: self.engine.players[opponent as usize].login.clone(),
                    kills,
                    deaths,
                    kdr: if deaths == 0 {
                        None
                    } else {
                        Some(kills as f64 / deaths as f64)
                    },
                })
            })
            .collect();
        rows.sort_by(|a, b| {
            (b.kills + b.deaths)
                .cmp(&(a.kills + a.deaths))
                .then_with(|| a.opponent.cmp(&b.opponent))
        });
        rows
    }

    /// `(kills_by_a, kills_by_b)` for a pair in one category.
    pub fn head_to_head(&self, a: u32, b: u32, cat: Category) -> Option<(u64, u64)> {
        let ci = cat.index();
        let key = pair_key(a, b);
        let pair = self.engine.pairs.get(&key)?;
        let (lo, _) = pair_key_parts(key);
        Some(if a == lo {
            (pair.kills_lo[ci], pair.kills_hi[ci])
        } else {
            (pair.kills_hi[ci], pair.kills_lo[ci])
        })
    }

    /// Latest events first, optionally restricted to one player (killer or victim).
    pub fn recent_events(&self, player: Option<&str>, limit: usize, offset: usize) -> (Vec<EventView>, usize) {
        let take = if limit == 0 { usize::MAX } else { limit };
        let mut items = Vec::new();
        let mut matched = 0usize;
        for event in self.events.iter().rev() {
            if let Some(login) = player {
                if event.killer != login && event.victim != login {
                    continue;
                }
            }
            matched += 1;
            if matched > offset && items.len() < take {
                items.push(EventView::from(event));
            }
        }
        (items, matched)
    }
}

fn is_sorted(events: &[Event]) -> bool {
    events.windows(2).all(|w| (w[0].time, w[0].seq) <= (w[1].time, w[1].seq))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: &str, time: i64, killer: &str, victim: &str, players: u32) -> EventInput {
        EventInput {
            id: id.to_string(),
            time,
            killer: killer.to_string(),
            victim: victim.to_string(),
            player_count: players,
        }
    }

    /// Full state digest, used to prove that replay order does not matter.
    fn fingerprint(store: &Store) -> String {
        let mut out = String::new();
        for p in &store.engine.players {
            out.push_str(&format!("{}\n", p.login));
            for (i, c) in p.cats.iter().enumerate() {
                out.push_str(&format!(
                    "  {} r={:.10} rd={:.10} s={:.10} w={} l={} k={} d={} la={:?}\n",
                    Category::ALL[i].as_str(),
                    c.rating,
                    c.rd,
                    c.sigma,
                    c.wins,
                    c.losses,
                    c.kills,
                    c.deaths,
                    c.last_active
                ));
            }
        }
        let mut keys: Vec<&u64> = store.engine.pairs.keys().collect();
        keys.sort();
        for k in keys {
            let (lo, hi) = pair_key_parts(*k);
            let p = &store.engine.pairs[k];
            out.push_str(&format!(
                "pair {}|{} kills={:?}/{:?} match={:?}/{:?}\n",
                store.engine.players[lo as usize].login,
                store.engine.players[hi as usize].login,
                p.kills_lo,
                p.kills_hi,
                p.match_lo,
                p.match_hi
            ));
        }
        out
    }

    fn history() -> Vec<EventInput> {
        let mut v = Vec::new();
        // a and b race in a duel lobby across a day boundary.
        for i in 0..25 {
            v.push(input(&format!("d{i}"), 1_000 + i * 60, if i % 3 == 0 { "b" } else { "a" }, if i % 3 == 0 { "a" } else { "b" }, 2));
        }
        // a third player joins: pub events, plus an excluded .nobody death.
        for i in 0..10 {
            v.push(input(&format!("p{i}"), 200_000 + i * 90_000, "c", "a", 4));
        }
        v.push(input("x0", 400_000, "a", NOBODY, 4));
        v.push(input("x1", 400_001, NOBODY, "b", 4));
        v
    }

    #[test]
    fn in_order_and_out_of_order_imports_agree() {
        let mut ordered = Store::new();
        ordered.import(history()).unwrap();

        let mut shuffled = Store::new();
        let mut h = history();
        h.reverse();
        // Feed it in awkward chunks too.
        for chunk in h.chunks(7) {
            shuffled.import(chunk.to_vec()).unwrap();
        }

        assert_eq!(fingerprint(&ordered), fingerprint(&shuffled));
        assert_eq!(ordered.event_count(), shuffled.event_count());
        assert_eq!(ordered.applied, ordered.event_count());
    }

    #[test]
    fn reimporting_the_same_batch_is_a_no_op() {
        let mut store = Store::new();
        let first = store.import(history()).unwrap();
        assert_eq!(first.imported, history().len());
        assert!(!first.rebuilt, "nothing was applied yet, so nothing to replay");

        let before = fingerprint(&store);
        let second = store.import(history()).unwrap();
        assert_eq!(second.imported, 0);
        assert_eq!(second.skipped, history().len());
        assert!(!second.rebuilt);
        assert_eq!(before, fingerprint(&store));
    }

    #[test]
    fn corrections_replay_the_log() {
        let mut store = Store::new();
        store.import(history()).unwrap();
        let before = fingerprint(&store);

        // Same id, different victim: the event is replaced and the log replayed.
        let mut fixed = history();
        fixed[0] = input("d0", 1_000, "a", "c", 2);
        let report = store.import(fixed).unwrap();
        assert_eq!(report.updated, 1);
        assert_eq!(report.skipped, history().len() - 1);
        assert!(report.rebuilt);
        assert_ne!(before, fingerprint(&store));
    }

    #[test]
    fn excluded_events_are_stored_but_never_rated() {
        let mut store = Store::new();
        store.import(vec![
            input("1", 0, NOBODY, "b", 2),
            input("2", 1, "a", NOBODY, 2),
            input("3", 2, NOBODY, NOBODY, 3),
        ])
        .unwrap();
        assert_eq!(store.player_count(), 0, ".nobody events register nobody");
        assert_eq!(store.event_count(), 3);
        let (items, total) = store.recent_events(None, 10, 0);
        assert_eq!(total, 3);
        assert!(items.iter().all(|e| !e.eligible));
    }

    #[test]
    fn late_history_extends_an_already_applied_prefix() {
        let mut store = Store::new();
        store.import(vec![input("a", 0, "a", "b", 2)]).unwrap();
        assert!(!store.import(vec![input("b", 60, "a", "b", 2)]).unwrap().rebuilt);
        // An event that lands before the applied prefix forces a replay.
        let report = store.import(vec![input("c", -60, "a", "b", 2)]).unwrap();
        assert!(report.rebuilt);
        assert_eq!(store.recent_events(None, 10, 0).0[2].id, "c");
    }

    #[test]
    fn leaderboard_is_ranked_and_cached_until_the_next_write() {
        let mut store = Store::new();
        store.import(history()).unwrap();

        let (rows, total) = store.leaderboard(Category::Total);
        assert_eq!(total, 3);
        assert_eq!(rows[0].rank, 1);
        assert!(rows[0].stats.rating >= rows[1].stats.rating);
        // Cached: same Arc while nothing changed.
        assert!(Arc::ptr_eq(&rows, &store.leaderboard(Category::Total).0));
        // Invalidated by the next write.
        store.import(vec![input("z", 500_000, "a", "c", 2)]).unwrap();
        assert!(!Arc::ptr_eq(&rows, &store.leaderboard(Category::Total).0));
    }

    #[test]
    fn leaderboards_only_list_players_active_in_that_category() {
        let mut store = Store::new();
        store.import(vec![input("1", 0, "a", "b", 3)]).unwrap();
        assert_eq!(store.leaderboard(Category::Pub).1, 2);
        assert_eq!(store.leaderboard(Category::Duel).1, 0);
        assert_eq!(store.leaderboard(Category::Total).1, 2);
    }

    #[test]
    fn matchups_and_head_to_head_are_symmetric() {
        let mut store = Store::new();
        store.import(history()).unwrap();
        let a = store.player_id("a").unwrap();
        let b = store.player_id("b").unwrap();
        let c = store.player_id("c").unwrap();

        let (a_kills, b_kills) = store.head_to_head(a, b, Category::Duel).unwrap();
        assert_eq!((b_kills, a_kills), store.head_to_head(b, a, Category::Duel).unwrap());
        assert!(a_kills + b_kills > 0);

        let rows = store.matchups(c, Category::Total);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].opponent, "a");
        assert_eq!(rows[0].kills, 10);
        assert_eq!(rows[0].deaths, 0);
        assert_eq!(rows[0].kdr, None, "no deaths means an undefined ratio");

        // b only ever met a in 1v1 events.
        assert_eq!(store.matchups(b, Category::Pub).len(), 0);
        assert_eq!(store.matchups(b, Category::Duel).len(), 1);
    }

    #[test]
    fn recent_events_filter_by_player_and_paginate() {
        let mut store = Store::new();
        store.import(history()).unwrap();
        let (page, total) = store.recent_events(Some("c"), 2, 0);
        assert_eq!(total, 10);
        assert_eq!(page.len(), 2);
        assert!(page.iter().all(|e| e.killer == "c" || e.victim == "c"));
        assert!(page[0].time > page[1].time, "newest first");
        let (page2, _) = store.recent_events(Some("c"), 2, 2);
        assert_ne!(page[0].id, page2[0].id);

        // .nobody shows up as a participant, with the flag off.
        let (nobody, total) = store.recent_events(Some(NOBODY), 10, 0);
        assert_eq!(total, 2);
        assert!(nobody.iter().all(|e| !e.eligible));
    }
}
