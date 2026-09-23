//! Domain types shared by the engine, the store and the HTTP layer.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

/// The three independent rating ladders.
///
/// `Total` receives every eligible event; `OneVsOne` only events recorded with
/// exactly two players in the lobby, `Pub` only events with more than two.
/// Classification uses the player count recorded *on the event*, never the
/// lobby size at the start or end of a race.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub enum Category {
    #[serde(rename = "total")]
    Total,
    #[serde(rename = "1v1")]
    OneVsOne,
    #[serde(rename = "pub")]
    Pub,
}

impl Category {
    pub const fn index(self) -> usize {
        match self {
            Category::Total => 0,
            Category::OneVsOne => 1,
            Category::Pub => 2,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Category::Total => "total",
            Category::OneVsOne => "1v1",
            Category::Pub => "pub",
        }
    }

    /// Accepts `total`, `1v1`, `pub` (case-insensitive).
    pub fn parse(value: &str) -> Option<Category> {
        match value.trim().to_ascii_lowercase().as_str() {
            "total" | "all" | "" => Some(Category::Total),
            "1v1" | "1vs1" | "duel" => Some(Category::OneVsOne),
            "pub" | "public" => Some(Category::Pub),
            _ => None,
        }
    }

    /// Categories an event with `player_count` players contributes to.
    /// Fewer than two players is malformed input: it still feeds `total`.
    pub fn for_player_count(player_count: u32) -> (Category, Option<Category>) {
        match player_count {
            2 => (Category::Total, Some(Category::OneVsOne)),
            n if n > 2 => (Category::Total, Some(Category::Pub)),
            _ => (Category::Total, None),
        }
    }
}

/// A kill event exactly as it is imported, persisted and exported.
///
/// This is also the on-disk line format of `events.jsonl`, so the file can be
/// posted back to the import endpoint verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EventInput {
    /// Stable identifier from the source system; re-imports are idempotent.
    pub id: String,
    /// RFC 3339 timestamp, e.g. `2026-05-16T18:36:04Z`.
    pub time: DateTime<Utc>,
    pub killer: String,
    pub victim: String,
    /// Number of players in the lobby at the moment of the kill.
    pub player_count: u32,
}

/// An event after interning: logins are `u32` handles, time is pre-flattened to
/// whole seconds for the inactivity arithmetic.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub id: String,
    pub time: DateTime<Utc>,
    pub time_secs: i64,
    pub killer: u32,
    pub victim: u32,
    pub player_count: u32,
}

/// Maps logins to dense `u32` handles so the hot rating loop never hashes a
/// string or allocates.
#[derive(Debug, Default)]
pub struct Interner {
    map: HashMap<String, u32>,
    names: Vec<String>,
}

impl Interner {
    pub fn intern(&mut self, login: &str) -> u32 {
        if let Some(&id) = self.map.get(login) {
            return id;
        }
        let id = self.names.len() as u32;
        self.names.push(login.to_string());
        self.map.insert(login.to_string(), id);
        id
    }

    pub fn get(&self, login: &str) -> Option<u32> {
        self.map.get(login).copied()
    }

    pub fn name(&self, id: u32) -> &str {
        &self.names[id as usize]
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }
}

impl Interner {
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// One alt-account mapping. `id` is the source system's row id and is carried
/// through untouched so an existing alt file round-trips unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AltEntry {
    pub alt: String,
    pub main: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_classification_uses_recorded_player_count() {
        assert_eq!(
            Category::for_player_count(2),
            (Category::Total, Some(Category::OneVsOne))
        );
        assert_eq!(
            Category::for_player_count(3),
            (Category::Total, Some(Category::Pub))
        );
        assert_eq!(
            Category::for_player_count(12),
            (Category::Total, Some(Category::Pub))
        );
        assert_eq!(Category::for_player_count(1), (Category::Total, None));
        assert_eq!(Category::for_player_count(0), (Category::Total, None));
    }

    #[test]
    fn category_parsing_round_trips() {
        for cat in [Category::Total, Category::OneVsOne, Category::Pub] {
            assert_eq!(Category::parse(cat.as_str()), Some(cat));
        }
        assert_eq!(Category::parse("PUB"), Some(Category::Pub));
        assert_eq!(Category::parse("nope"), None);
    }

    #[test]
    fn interner_is_stable() {
        let mut i = Interner::default();
        assert_eq!(i.intern("a"), 0);
        assert_eq!(i.intern("b"), 1);
        assert_eq!(i.intern("a"), 0);
        assert_eq!(i.name(1), "b");
        assert_eq!(i.len(), 2);
    }
}
