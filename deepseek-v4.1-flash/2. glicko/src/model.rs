//! Domain model: tuning constants, rating categories, events and the
//! per-player / per-pair state that the engine folds events into.

use serde::{Deserialize, Deserializer, Serialize};

use crate::glicko::Rating;

/// Login used by the game for deaths that have no player behind them
/// (suicides, world/environment kills). Such events are excluded from every
/// rating calculation and every statistic.
pub const NOBODY: &str = ".nobody";

pub const DEFAULT_RATING: f64 = 1500.0;
pub const DEFAULT_RD: f64 = 350.0;
pub const DEFAULT_VOLATILITY: f64 = 0.06;
/// Glicko-2 rating scale factor.
pub const SCALE: f64 = 173.7178;
pub const EPSILON: f64 = 1e-6;
pub const TAU: f64 = 0.5;

/// Kills needed to win a match (race to 20, i.e. best of 39).
pub const MATCH_WIN_KILLS: u8 = 20;
pub const SECONDS_PER_DAY: i64 = 86_400;

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 1000;

pub const CATEGORY_COUNT: usize = 3;

/// The three independent rating systems.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum Category {
    /// Every eligible event.
    Total = 0,
    /// Events recorded while exactly two players were in the lobby.
    Duel = 1,
    /// Events recorded while more than two players were in the lobby.
    Pub = 2,
}

impl Category {
    pub const ALL: [Category; CATEGORY_COUNT] = [Category::Total, Category::Duel, Category::Pub];

    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Category::Total => "total",
            Category::Duel => "1v1",
            Category::Pub => "pub",
        }
    }

    pub fn parse(s: &str) -> Option<Category> {
        match s.to_ascii_lowercase().as_str() {
            "total" => Some(Category::Total),
            "1v1" | "duel" => Some(Category::Duel),
            "pub" => Some(Category::Pub),
            _ => None,
        }
    }

    /// Lobby category implied by the player count recorded on an event:
    /// exactly two players is a duel, three or more is a pub game. Anything
    /// below two is not a lobby and only contributes to `total`.
    pub fn from_player_count(players: u32) -> Option<Category> {
        match players {
            2 => Some(Category::Duel),
            n if n >= 3 => Some(Category::Pub),
            _ => None,
        }
    }
}

/// A registered event, in the shape the engine consumes.
#[derive(Clone, Debug)]
pub struct Event {
    pub id: String,
    /// Unix seconds.
    pub time: i64,
    pub killer: String,
    pub victim: String,
    /// Players in the lobby at the moment the event was recorded.
    pub player_count: u32,
    /// Insertion order, used only to break timestamp ties deterministically.
    pub seq: u64,
}

impl Event {
    pub fn eligible(&self) -> bool {
        self.killer != NOBODY && self.victim != NOBODY
    }

    pub fn lobby_category(&self) -> Option<Category> {
        Category::from_player_count(self.player_count)
    }

    /// Rating systems this event feeds: always `total`, plus the lobby category.
    pub fn categories(&self) -> impl Iterator<Item = Category> {
        [Some(Category::Total), self.lobby_category()]
            .into_iter()
            .flatten()
    }
}

/// Cumulative statistics for one player in one category.
///
/// `kills`/`deaths` are lifetime counters that survive match resets;
/// `wins`/`losses` count completed races to 20.
#[derive(Clone, Debug)]
pub struct CatStats {
    pub rating: f64,
    pub rd: f64,
    pub sigma: f64,
    pub wins: u64,
    pub losses: u64,
    pub kills: u64,
    pub deaths: u64,
    /// Timestamp of the last eligible event for this player in this category.
    pub last_active: Option<i64>,
}

impl Default for CatStats {
    fn default() -> Self {
        Self {
            rating: DEFAULT_RATING,
            rd: DEFAULT_RD,
            sigma: DEFAULT_VOLATILITY,
            wins: 0,
            losses: 0,
            kills: 0,
            deaths: 0,
            last_active: None,
        }
    }
}

impl CatStats {
    #[inline]
    pub fn rating(&self) -> Rating {
        Rating::new(self.rating, self.rd, self.sigma)
    }

    #[inline]
    pub fn set(&mut self, r: Rating) {
        self.rating = r.rating;
        self.rd = r.rd;
        self.sigma = r.sigma;
    }

    /// Register an eligible event at `time`: grow the RD by the whole days of
    /// inactivity since the previous event, then move `last_active` forward.
    /// Fractional days are discarded, never carried over.
    pub fn touch(&mut self, time: i64) {
        if let Some(last) = self.last_active {
            let elapsed = time - last;
            if elapsed > 0 {
                let periods = (elapsed / SECONDS_PER_DAY) as u64;
                if periods > 0 {
                    self.rd = crate::glicko::inflate_rd(self.rd, self.sigma, periods);
                }
            }
        }
        self.last_active = Some(time);
    }

    pub fn view(&self) -> StatsView {
        let matches = self.wins + self.losses;
        StatsView {
            rating: self.rating,
            rd: self.rd,
            volatility: self.sigma,
            wins: self.wins,
            losses: self.losses,
            winrate: if matches == 0 {
                0.0
            } else {
                self.wins as f64 / matches as f64
            },
            kills: self.kills,
            deaths: self.deaths,
            kdr: if self.deaths == 0 {
                None
            } else {
                Some(self.kills as f64 / self.deaths as f64)
            },
            last_active: self.last_active.map(fmt_time),
        }
    }
}

/// One player's state across all three rating systems.
#[derive(Clone, Debug)]
pub struct PlayerState {
    pub login: String,
    pub cats: [CatStats; CATEGORY_COUNT],
}

impl PlayerState {
    pub fn new(login: &str) -> Self {
        Self {
            login: login.to_string(),
            cats: Default::default(),
        }
    }
}

/// Everything tracked for an unordered pair of players in one category.
///
/// `kills_*` are lifetime head-to-head counters; `match_*` are the counters of
/// the race currently in progress and reset when that race is won.
#[derive(Copy, Clone, Debug, Default)]
pub struct PairState {
    pub kills_lo: [u64; CATEGORY_COUNT],
    pub kills_hi: [u64; CATEGORY_COUNT],
    pub match_lo: [u8; CATEGORY_COUNT],
    pub match_hi: [u8; CATEGORY_COUNT],
}

/// Stable key for the unordered pair `{a, b}`.
#[inline]
pub fn pair_key(a: u32, b: u32) -> u64 {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    ((lo as u64) << 32) | hi as u64
}

#[inline]
pub fn pair_key_parts(key: u64) -> (u32, u32) {
    ((key >> 32) as u32, (key & 0xffff_ffff) as u32)
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// An event as accepted by `POST /api/v1/events/import`.
#[derive(Clone, Debug, Deserialize)]
pub struct EventInput {
    /// Stable identifier from the source system. Re-importing the same id is a no-op.
    pub id: String,
    #[serde(deserialize_with = "de_time")]
    pub time: i64,
    pub killer: String,
    pub victim: String,
    /// Players in the lobby, as recorded on the event itself.
    #[serde(alias = "players", alias = "playerCount", alias = "lobby_size")]
    pub player_count: u32,
}

impl EventInput {
    pub fn into_event(self, seq: u64) -> Event {
        Event {
            id: self.id,
            time: self.time,
            killer: self.killer,
            victim: self.victim,
            player_count: self.player_count,
            seq,
        }
    }

    /// True when the event carries the same data as an already stored one.
    pub fn same_as(&self, e: &Event) -> bool {
        self.time == e.time
            && self.killer == e.killer
            && self.victim == e.victim
            && self.player_count == e.player_count
    }
}

/// Rating/statistics projection shared by leaderboards and player lookups.
#[derive(Clone, Debug, Serialize)]
pub struct StatsView {
    pub rating: f64,
    pub rd: f64,
    pub volatility: f64,
    pub wins: u64,
    pub losses: u64,
    pub winrate: f64,
    pub kills: u64,
    pub deaths: u64,
    /// `null` when the player has not died yet (division by zero is undefined).
    pub kdr: Option<f64>,
    pub last_active: Option<String>,
}

/// A leaderboard entry. `rank` is absolute across the whole category.
#[derive(Clone, Debug, Serialize)]
pub struct LeaderRow {
    pub rank: usize,
    pub login: String,
    #[serde(flatten)]
    pub stats: StatsView,
}

/// An event as returned by `GET /api/v1/events`.
#[derive(Clone, Debug, Serialize)]
pub struct EventView {
    pub id: String,
    pub time: String,
    pub killer: String,
    pub victim: String,
    pub player_count: u32,
    /// Lobby category of the event, `null` when fewer than two players were recorded.
    pub lobby: Option<&'static str>,
    /// `false` for events involving `.nobody`, which are stored but never rated.
    pub eligible: bool,
}

impl From<&Event> for EventView {
    fn from(e: &Event) -> Self {
        Self {
            id: e.id.clone(),
            time: fmt_time(e.time),
            killer: e.killer.clone(),
            victim: e.victim.clone(),
            player_count: e.player_count,
            lobby: e.lobby_category().map(Category::as_str),
            eligible: e.eligible(),
        }
    }
}

/// Head-to-head line for one opponent.
#[derive(Clone, Debug, Serialize)]
pub struct MatchupRow {
    pub opponent: String,
    pub kills: u64,
    pub deaths: u64,
    pub kdr: Option<f64>,
}

pub fn fmt_time(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

pub fn parse_time(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp())
}

/// Accepts unix seconds, unix milliseconds (values >= 1e11) or RFC 3339 strings.
fn de_time<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(n) => {
            let raw = n
                .as_f64()
                .ok_or_else(|| D::Error::custom("timestamp is not a number"))?;
            Ok(normalize_epoch(raw))
        }
        serde_json::Value::String(s) => {
            if let Some(t) = parse_time(&s) {
                return Ok(t);
            }
            s.trim()
                .parse::<f64>()
                .map(normalize_epoch)
                .map_err(|_| D::Error::custom(format!("unparsable timestamp {s:?}")))
        }
        other => Err(D::Error::custom(format!(
            "timestamp must be a number or RFC 3339 string, got {other}"
        ))),
    }
}

/// Milliseconds are auto-detected: any epoch value that large is not seconds.
fn normalize_epoch(value: f64) -> i64 {
    if value.abs() >= 1e11 {
        (value / 1000.0) as i64
    } else {
        value as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_map_from_recorded_player_count() {
        assert_eq!(Category::from_player_count(1), None);
        assert_eq!(Category::from_player_count(2), Some(Category::Duel));
        assert_eq!(Category::from_player_count(3), Some(Category::Pub));
        assert_eq!(Category::from_player_count(64), Some(Category::Pub));
        assert_eq!(Category::parse("1v1"), Some(Category::Duel));
        assert_eq!(Category::parse("PUB"), Some(Category::Pub));
        assert_eq!(Category::parse("nope"), None);
    }

    #[test]
    fn nobody_events_are_ineligible() {
        let mut e = Event {
            id: "1".into(),
            time: 0,
            killer: "a".into(),
            victim: "b".into(),
            player_count: 2,
            seq: 0,
        };
        assert!(e.eligible());
        e.victim = NOBODY.into();
        assert!(!e.eligible());
        e.victim = "b".into();
        e.killer = NOBODY.into();
        assert!(!e.eligible());
    }

    #[test]
    fn kdr_is_null_until_the_player_has_died() {
        let mut s = CatStats::default();
        s.kills = 10;
        assert_eq!(s.view().kdr, None);
        s.deaths = 4;
        assert_eq!(s.view().kdr, Some(2.5));
        assert_eq!(s.view().winrate, 0.0);
        s.wins = 3;
        s.losses = 1;
        assert_eq!(s.view().winrate, 0.75);
    }
}
