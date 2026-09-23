//! The store: the event log, the alt-account map, and every derived view.
//!
//! The log is the single source of truth. Ratings are recomputed from it in one
//! chronological pass whenever the log or the alt map changes, which is what
//! makes late alt mappings ("this player was playing on that account all along")
//! exact instead of incremental guesswork. A full pass is linear in the number
//! of events and runs on two threads (alt-aware and no-alt views).

use crate::config::Config;
use crate::engine::{self, ComputeInput, ComputeStats, Derived};
use crate::glicko::GlickoParams;
use crate::model::{AltEntry, Category, EventInput, Interner, StoredEvent};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::time::Instant;
use utoipa::ToSchema;

/// Maximum alt-chain length followed before giving up (cycles are rejected on
/// insert, this is belt-and-braces for hand-edited files).
const MAX_ALT_HOPS: usize = 64;

/// Failure of a store operation, split by whose fault it is so the HTTP layer
/// can answer 400 instead of 500 for bad input.
#[derive(Debug)]
pub enum StoreError {
    /// The caller supplied something unusable (a malformed payload, a cyclic
    /// alt mapping).
    Invalid(String),
    /// The server could not do its job (disk, serialization).
    Internal(String),
}

impl StoreError {
    fn invalid(message: impl Into<String>) -> Self {
        StoreError::Invalid(message.into())
    }

    fn internal(message: impl Into<String>) -> Self {
        StoreError::Internal(message.into())
    }
}

impl From<String> for StoreError {
    fn from(message: String) -> Self {
        StoreError::Internal(message)
    }
}

impl std::error::Error for StoreError {}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Invalid(message) | StoreError::Internal(message) => {
                formatter.write_str(message)
            }
        }
    }
}

/// Which view of the data a request is asking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Alt accounts collapsed into their main.
    Alt,
    /// Every login stands on its own.
    NoAlt,
}

/// What an import or an alt-map change did.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ImportReport {
    /// Events read from the request.
    pub received: u64,
    /// Events newly added to the log.
    pub imported: u64,
    /// Events skipped because their id was already known.
    pub duplicates: u64,
    /// Total events in the log afterwards.
    pub total_events: u64,
    /// Registered players afterwards (alt-aware view).
    pub players: u64,
    /// Eligible events applied to ratings (alt-aware view).
    pub eligible_events: u64,
    /// Events dropped because the killer or victim is excluded.
    pub skipped_excluded: u64,
    /// Events dropped because killer and victim are the same player.
    pub skipped_self: u64,
    /// Races completed in the log (alt-aware view).
    pub matches: u64,
    /// Time spent recomputing the derived state.
    pub recompute_ms: u64,
}

pub struct Store {
    params: GlickoParams,
    excluded: HashSet<String>,
    events_path: std::path::PathBuf,
    alts_path: std::path::PathBuf,
    interner: Interner,
    events: Vec<StoredEvent>,
    id_index: HashMap<String, u32>,
    /// Event indices in chronological order.
    order: Vec<u32>,
    alts: BTreeMap<String, AltEntry>,
    excluded_ids: Vec<bool>,
    alt_state: Derived,
    noalt_state: Derived,
    alt_stats: ComputeStats,
    noalt_stats: ComputeStats,
}

impl Store {
    // ---------------------------------------------------------------- loading

    /// Loads the persisted log and alt map from `config.data_dir`, then builds
    /// the derived state.
    pub fn load(config: &Config) -> Result<Self, StoreError> {
        fs::create_dir_all(&config.data_dir).map_err(|e| {
            format!(
                "cannot create data directory {}: {e}",
                config.data_dir.display()
            )
        })?;
        let mut store = Store {
            params: config.glicko_params(),
            excluded: config.excluded_logins(),
            events_path: config.events_path(),
            alts_path: config.alts_path(),
            interner: Interner::default(),
            events: Vec::new(),
            id_index: HashMap::new(),
            order: Vec::new(),
            alts: BTreeMap::new(),
            excluded_ids: Vec::new(),
            alt_state: Derived::default(),
            noalt_state: Derived::default(),
            alt_stats: ComputeStats::default(),
            noalt_stats: ComputeStats::default(),
        };
        store.load_events()?;
        store.load_alts()?;
        let mut report = ImportReport::default();
        store.refresh(&mut report, Some(0));
        tracing::info!(
            events = store.events.len(),
            players = store.alt_state.registered,
            matches = store.alt_stats.matches,
            alts = store.alts.len(),
            "loaded event history"
        );
        Ok(store)
    }

    fn load_events(&mut self) -> Result<(), StoreError> {
        if !self.events_path.exists() {
            return Ok(());
        }
        let file = File::open(&self.events_path)
            .map_err(|e| format!("cannot open {}: {e}", self.events_path.display()))?;
        let reader = BufReader::new(file);
        let mut skipped = 0u64;
        for (line_no, line) in reader.lines().enumerate() {
            let line =
                line.map_err(|e| format!("cannot read {}: {e}", self.events_path.display()))?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<EventInput>(line) {
                Ok(event) => {
                    self.insert_event(event);
                }
                Err(e) => {
                    skipped += 1;
                    tracing::warn!(
                        file = %self.events_path.display(),
                        line = line_no + 1,
                        error = %e,
                        "skipping unparseable event"
                    );
                }
            }
        }
        if skipped > 0 {
            tracing::warn!(skipped, "some persisted events could not be read back");
        }
        Ok(())
    }

    fn load_alts(&mut self) -> Result<(), StoreError> {
        if !self.alts_path.exists() {
            return Ok(());
        }
        let entries: Vec<AltEntry> =
            crate::config::read_json_array(&self.alts_path).map_err(StoreError::internal)?;
        for entry in entries {
            self.alts.insert(entry.alt.clone(), entry);
        }
        Ok(())
    }

    // ---------------------------------------------------------------- import

    /// Imports already-parsed events (the HTTP path).
    pub fn import_events(&mut self, events: Vec<EventInput>) -> Result<ImportReport, StoreError> {
        let from = self.events.len();
        let mut writer = self.open_events_writer()?;
        let mut report = ImportReport::default();
        let mut line: Vec<u8> = Vec::with_capacity(256);
        for event in events {
            report.received += 1;
            line.clear();
            serde_json::to_writer(&mut line, &event)
                .map_err(|e| format!("cannot serialize event `{}`: {e}", event.id))?;
            line.push(b'\n');
            if self.insert_event(event) {
                writer
                    .write_all(&line)
                    .map_err(|e| format!("cannot append to {}: {e}", self.events_path.display()))?;
                report.imported += 1;
            } else {
                report.duplicates += 1;
            }
        }
        writer
            .flush()
            .map_err(|e| format!("cannot flush {}: {e}", self.events_path.display()))?;
        self.refresh(&mut report, Some(from));
        Ok(report)
    }

    /// Streams a JSON array of events from a file, applying each event as it is
    /// read: a multi-million event history never has to fit in memory.
    pub fn import_json_file(&mut self, path: &Path) -> Result<ImportReport, StoreError> {
        let file = File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        self.import_json_reader(BufReader::new(file), &path.display().to_string())
    }

    /// Streaming import from any reader holding a top-level JSON array of events.
    ///
    /// Events already accepted stay accepted if the stream turns out to be
    /// malformed: the error names how many were applied.
    pub fn import_json_reader<R: std::io::Read>(
        &mut self,
        reader: R,
        source: &str,
    ) -> Result<ImportReport, StoreError> {
        let from = self.events.len();
        let mut writer = self.open_events_writer()?;
        let mut report = ImportReport::default();
        let mut line: Vec<u8> = Vec::with_capacity(256);
        let outcome = {
            let mut deserializer = serde_json::Deserializer::from_reader(reader);
            let sink = EventSink {
                store: self,
                writer: &mut writer,
                report: &mut report,
                line: &mut line,
            };
            serde::Deserializer::deserialize_seq(&mut deserializer, sink)
        };
        let flush = writer
            .flush()
            .map_err(|e| format!("cannot flush {}: {e}", self.events_path.display()));
        self.refresh(&mut report, Some(from));
        flush?;
        match outcome {
            Ok(()) => Ok(report),
            Err(e) => Err(StoreError::invalid(format!(
                "{source}: {e} ({} events were applied before the error)",
                report.imported
            ))),
        }
    }

    fn open_events_writer(&self) -> Result<BufWriter<File>, StoreError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.events_path)
            .map_err(|e| format!("cannot open {} for append: {e}", self.events_path.display()))?;
        Ok(BufWriter::with_capacity(64 * 1024, file))
    }

    /// Adds an event to the log in memory. Returns `false` for a known id.
    fn insert_event(&mut self, event: EventInput) -> bool {
        if self.id_index.contains_key(&event.id) {
            return false;
        }
        let killer = self.interner.intern(&event.killer);
        let victim = self.interner.intern(&event.victim);
        self.id_index
            .insert(event.id.clone(), self.events.len() as u32);
        self.events.push(StoredEvent {
            id: event.id,
            time_secs: event.time.timestamp(),
            time: event.time,
            killer,
            victim,
            player_count: event.player_count,
        });
        true
    }

    // ------------------------------------------------------------------ alts

    /// Adds or replaces alt mappings. Rejects self-mappings and cycles, then
    /// recalculates every rating from the log.
    pub fn upsert_alts(&mut self, entries: Vec<AltEntry>) -> Result<ImportReport, StoreError> {
        let mut candidate = self.alts.clone();
        let mut added = 0u64;
        let mut updated = 0u64;
        for entry in entries {
            if entry.alt == entry.main {
                return Err(StoreError::invalid(format!(
                    "alt `{}` cannot be its own main account",
                    entry.alt
                )));
            }
            match candidate.get(&entry.alt) {
                Some(existing) if existing.main != entry.main => updated += 1,
                Some(_) => {}
                None => added += 1,
            }
            candidate.insert(entry.alt.clone(), entry);
        }
        if let Some(login) = find_cycle(&candidate) {
            return Err(StoreError::invalid(format!(
                "alt mapping for `{login}` forms a cycle"
            )));
        }
        self.alts = candidate;
        self.persist_alts()?;
        let mut report = ImportReport::default();
        self.refresh(&mut report, None);
        tracing::info!(added, updated, total = self.alts.len(), "alt map updated");
        Ok(report)
    }

    /// Removes a mapping; the account becomes a player of its own again.
    pub fn remove_alt(&mut self, alt: &str) -> Result<Option<ImportReport>, StoreError> {
        if self.alts.remove(alt).is_none() {
            return Ok(None);
        }
        self.persist_alts()?;
        let mut report = ImportReport::default();
        self.refresh(&mut report, None);
        Ok(Some(report))
    }

    fn persist_alts(&self) -> Result<(), StoreError> {
        let entries: Vec<&AltEntry> = self.alts.values().collect();
        let json = serde_json::to_string_pretty(&entries)
            .map_err(|e| format!("cannot serialize alt map: {e}"))?;
        let temp = self.alts_path.with_extension("json.tmp");
        fs::write(&temp, json).map_err(|e| format!("cannot write {}: {e}", temp.display()))?;
        fs::rename(&temp, &self.alts_path)
            .map_err(|e| format!("cannot replace {}: {e}", self.alts_path.display()))?;
        Ok(())
    }

    // ------------------------------------------------------------- recompute

    /// Rebuilds chronological order (optionally from a known-ordered prefix) and
    /// recomputes both derived views.
    fn refresh(&mut self, report: &mut ImportReport, rebuild_order_from: Option<usize>) {
        if let Some(from) = rebuild_order_from {
            self.rebuild_order(from);
        }
        let started = Instant::now();
        self.recompute();
        report.recompute_ms = started.elapsed().as_millis() as u64;
        report.total_events = self.events.len() as u64;
        report.players = self.alt_state.registered as u64;
        report.eligible_events = self.alt_stats.processed;
        report.skipped_excluded = self.alt_stats.skipped_excluded;
        report.skipped_self = self.alt_stats.skipped_self;
        report.matches = self.alt_stats.matches;
    }

    /// Keeps the chronological index sorted by (timestamp, insertion order).
    /// Appending in order — the common case — avoids a full re-sort.
    fn rebuild_order(&mut self, from: usize) {
        let total = self.events.len() as u32;
        // Keys are snapshotted so the sort closure does not borrow `self`.
        let keys: Vec<(i64, u32)> = (0..total).map(|i| self.key(i)).collect();
        let sort_tail = |indices: &mut Vec<u32>| {
            indices.sort_unstable_by_key(|&i| keys[i as usize]);
        };
        if from == 0 {
            self.order = (0..total).collect();
            sort_tail(&mut self.order);
            return;
        }
        let mut tail: Vec<u32> = (from as u32..total).collect();
        sort_tail(&mut tail);
        let appends_cleanly = match (self.order.last(), tail.first()) {
            (Some(&last), Some(&first)) => keys[last as usize] <= keys[first as usize],
            _ => true,
        };
        if appends_cleanly {
            self.order.extend_from_slice(&tail);
        } else {
            self.order = (0..total).collect();
            sort_tail(&mut self.order);
        }
    }

    fn key(&self, index: u32) -> (i64, u32) {
        (self.events[index as usize].time_secs, index)
    }

    fn recompute(&mut self) {
        // Alt logins may never appear in the log but still name a player.
        let names: Vec<String> = self
            .alts
            .values()
            .flat_map(|e| [e.alt.clone(), e.main.clone()])
            .collect();
        for name in names {
            self.interner.intern(&name);
        }
        self.refresh_excluded();

        let resolve_alt = build_resolve(&self.interner, &self.alts);
        let resolve_noalt: Vec<u32> = (0..self.interner.len() as u32).collect();
        let params = self.params;

        let (alt, noalt) = std::thread::scope(|scope| {
            let alt_handle = scope.spawn(|| {
                engine::compute(&ComputeInput {
                    events: &self.events,
                    order: &self.order,
                    resolve: &resolve_alt,
                    names: self.interner.names(),
                    excluded: &self.excluded_ids,
                    params: &params,
                })
            });
            let noalt_handle = scope.spawn(|| {
                engine::compute(&ComputeInput {
                    events: &self.events,
                    order: &self.order,
                    resolve: &resolve_noalt,
                    names: self.interner.names(),
                    excluded: &self.excluded_ids,
                    params: &params,
                })
            });
            (alt_handle.join(), noalt_handle.join())
        });
        let (alt, alt_stats) = alt.expect("alt rating pass");
        let (noalt, noalt_stats) = noalt.expect("no-alt rating pass");
        self.alt_state = alt;
        self.alt_stats = alt_stats;
        self.noalt_state = noalt;
        self.noalt_stats = noalt_stats;
    }

    fn refresh_excluded(&mut self) {
        if self.excluded_ids.len() == self.interner.len() {
            return;
        }
        let excluded = &self.excluded;
        let flags: Vec<bool> = self
            .interner
            .names()
            .iter()
            .map(|name| excluded.contains(name.as_str()))
            .collect();
        self.excluded_ids = flags;
    }

    // ---------------------------------------------------------------- access

    pub fn params(&self) -> &GlickoParams {
        &self.params
    }

    pub fn excluded(&self) -> &HashSet<String> {
        &self.excluded
    }

    pub fn events(&self) -> &[StoredEvent] {
        &self.events
    }

    pub fn alts(&self) -> &BTreeMap<String, AltEntry> {
        &self.alts
    }

    pub fn interner(&self) -> &Interner {
        &self.interner
    }

    pub fn events_path(&self) -> &Path {
        &self.events_path
    }

    pub fn alts_path(&self) -> &Path {
        &self.alts_path
    }

    pub fn state(&self, mode: Mode) -> &Derived {
        match mode {
            Mode::Alt => &self.alt_state,
            Mode::NoAlt => &self.noalt_state,
        }
    }

    pub fn compute_stats(&self, mode: Mode) -> ComputeStats {
        match mode {
            Mode::Alt => self.alt_stats,
            Mode::NoAlt => self.noalt_stats,
        }
    }

    pub fn name(&self, id: u32) -> &str {
        self.interner.name(id)
    }

    // --------------------------------------------------------------- queries

    /// Canonical player id for a login in the given view, or `None` when the
    /// login is unknown or never took part in an eligible event.
    pub fn resolve_login(&self, mode: Mode, login: &str) -> Option<u32> {
        let raw = self.interner.get(login)?;
        let state = self.state(mode);
        let canonical = state.canonical(raw);
        state.player(canonical).map(|_| canonical)
    }

    pub fn leaderboard(&self, mode: Mode, category: Category) -> &[u32] {
        &self.state(mode).leaderboards[category.index()]
    }

    /// Stats for a registered player in one category. A player registered only
    /// in another category still resolves; they simply hold the fresh state
    /// (`last_active == None`).
    pub fn player_stats(
        &self,
        mode: Mode,
        login: &str,
        category: Category,
    ) -> Option<(u32, engine::CatStats)> {
        let id = self.resolve_login(mode, login)?;
        let stats = *self.state(mode).stats(id, category)?;
        Some((id, stats))
    }

    /// Lifetime head-to-head against every opponent, most kills first.
    pub fn matchups(
        &self,
        mode: Mode,
        login: &str,
        category: Category,
    ) -> Option<Vec<(u32, engine::OppStats)>> {
        let id = self.resolve_login(mode, login)?;
        let player = self.state(mode).player(id)?;
        let mut list: Vec<(u32, engine::OppStats)> = player.opponents[category.index()]
            .iter()
            .map(|(&k, &v)| (k, v))
            .collect();
        list.sort_unstable_by(|a, b| {
            b.1.kills
                .cmp(&a.1.kills)
                .then_with(|| a.1.deaths.cmp(&b.1.deaths))
                .then_with(|| self.interner.name(a.0).cmp(self.interner.name(b.0)))
        });
        Some(list)
    }

    /// Kills and deaths for `login` against exactly one opponent.
    pub fn head_to_head(
        &self,
        mode: Mode,
        login: &str,
        opponent: &str,
        category: Category,
    ) -> Option<(u32, u32, engine::OppStats)> {
        let player_id = self.resolve_login(mode, login)?;
        let opponent_id = self.resolve_login(mode, opponent)?;
        let player = self.state(mode).player(player_id)?;
        let stats = player.opponents[category.index()]
            .get(&opponent_id)
            .copied()
            .unwrap_or_default();
        Some((player_id, opponent_id, stats))
    }

    /// Newest registered events first, optionally filtered to one player (as
    /// killer or victim). Returns the page and the total number of matches.
    ///
    /// The unfiltered feed walks the log backwards; the filtered feed reads the
    /// player's precomputed event index, so neither scans the whole history.
    pub fn events_page(
        &self,
        mode: Mode,
        player: Option<u32>,
        offset: usize,
        limit: usize,
    ) -> (Vec<&StoredEvent>, u64) {
        let state = self.state(mode);
        let mut page = Vec::with_capacity(limit.min(256));
        match player {
            None => {
                let total = self.events.len() as u64;
                for event in self.events.iter().rev().skip(offset).take(limit) {
                    page.push(event);
                }
                (page, total)
            }
            Some(player) => {
                let positions = state.events_of(player);
                let total = positions.len() as u64;
                for &position in positions.iter().rev().skip(offset).take(limit) {
                    page.push(&self.events[position as usize]);
                }
                (page, total)
            }
        }
    }
}

/// Streaming sink for a top-level JSON array of events.
struct EventSink<'a, W: Write> {
    store: &'a mut Store,
    writer: &'a mut W,
    report: &'a mut ImportReport,
    line: &'a mut Vec<u8>,
}

impl<'de, W: Write> serde::de::Visitor<'de> for EventSink<'_, W> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON array of kill events")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<(), A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        while let Some(event) = seq.next_element::<EventInput>()? {
            self.report.received += 1;
            self.line.clear();
            serde_json::to_writer(&mut *self.line, &event).map_err(serde::de::Error::custom)?;
            self.line.push(b'\n');
            if self.store.insert_event(event) {
                self.writer
                    .write_all(self.line)
                    .map_err(|e| serde::de::Error::custom(format!("cannot append event: {e}")))?;
                self.report.imported += 1;
            } else {
                self.report.duplicates += 1;
            }
        }
        Ok(())
    }
}

/// Collapses every raw login onto its main account.
fn build_resolve(interner: &Interner, alts: &BTreeMap<String, AltEntry>) -> Vec<u32> {
    let mut resolve: Vec<u32> = (0..interner.len() as u32).collect();
    if alts.is_empty() {
        return resolve;
    }
    for (raw, slot) in resolve.iter_mut().enumerate() {
        let mut current: &str = interner.name(raw as u32);
        let mut hops = 0;
        while let Some(entry) = alts.get(current) {
            if entry.main == current {
                break;
            }
            current = &entry.main;
            hops += 1;
            if hops > MAX_ALT_HOPS {
                break;
            }
        }
        if hops > 0 {
            *slot = interner.get(current).unwrap_or(raw as u32);
        }
    }
    resolve
}

/// Returns a login that is part of a cycle, if any.
fn find_cycle(alts: &BTreeMap<String, AltEntry>) -> Option<String> {
    for start in alts.keys() {
        let mut current: &str = start;
        let mut hops = 0;
        while let Some(entry) = alts.get(current) {
            current = &entry.main;
            hops += 1;
            if hops > alts.len() {
                return Some(start.clone());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_EVENTS_FILE;
    use crate::model::Category;

    fn config(dir: &Path) -> Config {
        use clap::Parser;
        Config::parse_from([
            "glicko-api",
            "--data-dir",
            dir.to_str().expect("utf-8 path"),
        ])
    }

    fn event(id: &str, secs: i64, killer: &str, victim: &str, player_count: u32) -> EventInput {
        EventInput {
            id: id.to_string(),
            time: chrono::DateTime::from_timestamp(secs, 0).expect("valid timestamp"),
            killer: killer.to_string(),
            victim: victim.to_string(),
            player_count,
        }
    }

    #[test]
    fn events_are_rated_in_timestamp_order_not_arrival_order() {
        // The same race, once arriving shuffled and once arriving sorted: the
        // derived state must be identical, because the log is replayed by time.
        let mut shuffled = vec![event("win", 1_000_000, "a", "b", 2)];
        for i in 0..19 {
            shuffled.push(event(&format!("k{i}"), 1_000_000 - 100 + i, "a", "b", 2));
        }
        // the losing side got 19 kills earlier still, so the final score is 20-19
        for i in 0..19 {
            shuffled.push(event(&format!("d{i}"), 1_000_000 - 200 + i, "b", "a", 2));
        }
        let mut sorted = shuffled.clone();
        sorted.sort_by_key(|event| event.time);

        let dir_a = tempfile::tempdir().expect("temp dir");
        let dir_b = tempfile::tempdir().expect("temp dir");
        let mut store_a = Store::load(&config(dir_a.path())).expect("load");
        let mut store_b = Store::load(&config(dir_b.path())).expect("load");
        store_a.import_events(shuffled).expect("shuffled import");
        store_b.import_events(sorted).expect("sorted import");

        let read = |store: &Store| {
            let id = store.resolve_login(Mode::Alt, "a").expect("a is a player");
            *store
                .state(Mode::Alt)
                .stats(id, Category::Total)
                .expect("stats")
        };
        let from_shuffled = read(&store_a);
        let from_sorted = read(&store_b);
        assert_eq!((from_shuffled.wins, from_shuffled.losses), (1, 0));
        assert_eq!(from_shuffled.kills, 20);
        assert_eq!(from_shuffled.rating, from_sorted.rating);
        assert_eq!(from_shuffled.rd, from_sorted.rd);
        assert_eq!(from_shuffled.volatility, from_sorted.volatility);
        // a 20-19 win is close to a draw, so it gains far less than a 20-0 win
        assert!(
            from_shuffled.rating > 1500.0 && from_shuffled.rating < 1520.0,
            "rating {}",
            from_shuffled.rating
        );
    }

    #[test]
    fn incremental_imports_keep_the_chronological_index_sorted() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = Store::load(&config(dir.path())).expect("load");

        store
            .import_events(vec![
                event("c", 300, "a", "b", 2),
                event("a", 100, "a", "b", 2),
            ])
            .expect("first import");
        assert_eq!(store.events()[store.order[0] as usize].id, "a");
        assert_eq!(store.events()[store.order[1] as usize].id, "c");

        // appended in order: the fast path must keep the index valid
        store
            .import_events(vec![
                event("d", 400, "a", "b", 2),
                event("e", 500, "a", "b", 2),
            ])
            .expect("second import");
        let ids: Vec<&str> = store
            .order
            .iter()
            .map(|&i| store.events()[i as usize].id.as_str())
            .collect();
        assert_eq!(ids, vec!["a", "c", "d", "e"]);

        // appended out of order: the index must be rebuilt
        store
            .import_events(vec![event("b", 200, "a", "b", 2)])
            .expect("third import");
        let ids: Vec<&str> = store
            .order
            .iter()
            .map(|&i| store.events()[i as usize].id.as_str())
            .collect();
        assert_eq!(ids, vec!["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn streaming_import_reads_a_json_array_from_disk() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = Store::load(&config(dir.path())).expect("load");

        // A 40k-event file: 2000 complete 20-0 races.
        let path = dir.path().join("bulk.json");
        let mut file = std::io::BufWriter::new(File::create(&path).expect("create"));
        file.write_all(b"[").expect("write");
        for race in 0..2000i64 {
            for kill in 0..20i64 {
                let id = race * 20 + kill;
                if id > 0 {
                    file.write_all(b",").expect("write");
                }
                let event = event(
                    &format!("e{id}"),
                    race * 100 + kill,
                    if race % 2 == 0 { "a" } else { "b" },
                    if race % 2 == 0 { "b" } else { "a" },
                    2,
                );
                serde_json::to_writer(&mut file, &event).expect("serialize");
            }
        }
        file.write_all(b"]").expect("write");
        file.flush().expect("flush");
        drop(file);

        let report = store.import_json_file(&path).expect("import");
        assert_eq!(report.imported, 40_000);
        assert_eq!(
            report.matches, 4_000,
            "total and 1v1 for each of 2000 races"
        );
        assert_eq!(report.players, 2);

        let a = store.resolve_login(Mode::Alt, "a").expect("a");
        let stats = *store
            .state(Mode::Alt)
            .stats(a, Category::Total)
            .expect("stats");
        assert_eq!((stats.wins, stats.losses), (1000, 1000));
        assert_eq!((stats.kills, stats.deaths), (20_000, 20_000));
        assert!(
            (stats.rating - 1500.0).abs() < 50.0,
            "an even series must stay near the default rating, got {}",
            stats.rating
        );

        // and it survives a reload
        let reloaded = Store::load(&config(dir.path())).expect("reload");
        assert_eq!(reloaded.events().len(), 40_000);
        assert_eq!(reloaded.compute_stats(Mode::Alt).matches, 4_000);
    }

    #[test]
    fn a_malformed_stream_reports_how_much_was_applied() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = Store::load(&config(dir.path())).expect("load");
        let json = br#"[
            {"id":"ok","time":"2026-05-16T18:36:04Z","killer":"a","victim":"b","player_count":2},
            {"id":"bad","time":"nonsense","killer":"a","victim":"b","player_count":2}
        ]"#;
        let error = store
            .import_json_reader(&json[..], "test")
            .expect_err("must reject the bad event");
        assert!(matches!(error, StoreError::Invalid(_)), "{error:?}");
        assert!(
            error.to_string().contains("1 events were applied"),
            "{error}"
        );
        assert_eq!(store.events().len(), 1);
    }

    #[test]
    fn alt_mappings_reject_cycles_and_are_persisted() {
        let entry = |alt: &str, main: &str| AltEntry {
            alt: alt.to_string(),
            main: main.to_string(),
            id: None,
        };
        let names = |store: &Store, mode: Mode| {
            let mut names: Vec<String> = store
                .leaderboard(mode, Category::Total)
                .iter()
                .map(|&id| store.name(id).to_string())
                .collect();
            names.sort();
            names
        };

        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = Store::load(&config(dir.path())).expect("load");
        store
            .import_events(vec![event("e1", 10, "smurf", "bob", 2)])
            .expect("import");

        assert!(matches!(
            store.upsert_alts(vec![entry("x", "x")]),
            Err(StoreError::Invalid(_))
        ));
        assert!(matches!(
            store.upsert_alts(vec![entry("x", "y"), entry("y", "x")]),
            Err(StoreError::Invalid(_))
        ));
        assert!(store.alts().is_empty(), "rejected mappings are not kept");

        store
            .upsert_alts(vec![entry("smurf", "pro")])
            .expect("upsert");
        assert_eq!(names(&store, Mode::Alt), vec!["bob", "pro"]);
        assert_eq!(names(&store, Mode::NoAlt), vec!["bob", "smurf"]);

        // persisted, so a fresh store over the same directory picks it up
        let reloaded = Store::load(&config(dir.path())).expect("reload");
        assert_eq!(reloaded.alts().get("smurf").expect("mapping").main, "pro");
        assert_eq!(names(&reloaded, Mode::Alt), vec!["bob", "pro"]);

        // removing the mapping restores the standalone player
        let mut reopened = Store::load(&config(dir.path())).expect("reload");
        reopened
            .remove_alt("smurf")
            .expect("remove")
            .expect("existed");
        assert_eq!(names(&reopened, Mode::Alt), vec!["bob", "smurf"]);
        assert!(reopened.alts().is_empty());
    }

    #[test]
    fn chained_alt_mappings_collapse_transitively() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = Store::load(&config(dir.path())).expect("load");
        store
            .import_events(vec![event("e1", 10, "smurf", "bob", 2)])
            .expect("import");
        let entry = |alt: &str, main: &str| AltEntry {
            alt: alt.to_string(),
            main: main.to_string(),
            id: None,
        };
        // the mapping order is deliberately inverted
        store
            .upsert_alts(vec![entry("smurf", "pro"), entry("pro", "grandmaster")])
            .expect("upsert");

        let id = store.resolve_login(Mode::Alt, "smurf").expect("resolves");
        assert_eq!(store.name(id), "grandmaster");
        let id = store.resolve_login(Mode::Alt, "pro").expect("resolves");
        assert_eq!(store.name(id), "grandmaster");
        assert_eq!(store.state(Mode::Alt).registered, 2, "grandmaster + bob");
        // without alt resolution only logins that actually played are players
        assert_eq!(store.state(Mode::NoAlt).registered, 2, "smurf + bob");
    }

    #[test]
    fn unparseable_log_lines_are_skipped_without_losing_the_rest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(DEFAULT_EVENTS_FILE);
        std::fs::write(
            &path,
            concat!(
                "{\"id\":\"good1\",\"time\":\"2026-05-16T18:36:04Z\",\"killer\":\"a\",\"victim\":\"b\",\"player_count\":2}\n",
                "{\"id\":\"truncated\",\"time\":\n",
                "\n",
                "{\"id\":\"good2\",\"time\":\"2026-05-16T18:36:05Z\",\"killer\":\"b\",\"victim\":\"a\",\"player_count\":2}\n",
            ),
        )
        .expect("write log");

        let store = Store::load(&config(dir.path())).expect("load");
        assert_eq!(store.events().len(), 2);
        assert_eq!(store.compute_stats(Mode::Alt).processed, 2);
    }

    #[test]
    fn queries_answer_for_both_views() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = Store::load(&config(dir.path())).expect("load");
        store
            .import_events(vec![
                event("e1", 10, "smurf", "bob", 2),
                event("e2", 20, "bob", "smurf", 3),
            ])
            .expect("import");
        store
            .upsert_alts(vec![AltEntry {
                alt: "smurf".into(),
                main: "pro".into(),
                id: None,
            }])
            .expect("upsert");

        assert!(store.resolve_login(Mode::Alt, "smurf").is_some());
        assert!(store.resolve_login(Mode::NoAlt, "pro").is_none());
        assert!(store.resolve_login(Mode::Alt, "ghost").is_none());

        // the event feed resolves logins in the alt view and not in the other
        let (page, total) = store.events_page(Mode::Alt, None, 0, 10);
        assert_eq!(total, 2);
        assert_eq!(page[0].id, "e2", "newest first");

        let pro = store.resolve_login(Mode::Alt, "pro").expect("pro");
        let (_, filtered) = store.events_page(Mode::Alt, Some(pro), 0, 10);
        assert_eq!(filtered, 2);

        let smurf = store.resolve_login(Mode::NoAlt, "smurf").expect("smurf");
        let (noalt_page, noalt_total) = store.events_page(Mode::NoAlt, Some(smurf), 0, 10);
        assert_eq!(noalt_total, 2);
        assert_eq!(noalt_page.len(), 2);

        // matchups are per category
        let matchups = store
            .matchups(Mode::Alt, "pro", Category::Total)
            .expect("matchups");
        assert_eq!(matchups.len(), 1);
        assert_eq!(
            matchups[0].1,
            crate::engine::OppStats {
                kills: 1,
                deaths: 1
            }
        );
        let duel = store
            .matchups(Mode::Alt, "pro", Category::OneVsOne)
            .expect("matchups");
        assert_eq!(
            duel[0].1,
            crate::engine::OppStats {
                kills: 1,
                deaths: 0
            }
        );
    }
}
