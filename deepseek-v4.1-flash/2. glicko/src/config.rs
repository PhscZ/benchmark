//! Runtime configuration. Every knob has a CLI flag, an environment variable
//! and a default; the defaults are the ones this service was specified with.

use crate::glicko::GlickoParams;
use clap::Parser;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const DEFAULT_EVENTS_FILE: &str = "events.jsonl";
pub const DEFAULT_ALTS_FILE: &str = "alts.json";

#[derive(Debug, Clone, Parser)]
#[command(
    name = "glicko-api",
    about = "Glicko-2 rating REST API for kill-based game events",
    version
)]
pub struct Config {
    /// Interface to bind.
    #[arg(long, env = "GLICKO_HOST", default_value = "0.0.0.0")]
    pub host: String,

    /// TCP port to bind.
    #[arg(long, env = "GLICKO_PORT", default_value_t = 8080)]
    pub port: u16,

    /// Directory holding `events.jsonl` and `alts.json`.
    #[arg(long, env = "GLICKO_DATA_DIR", default_value = "data")]
    pub data_dir: PathBuf,

    /// Logins excluded from every rating calculation and statistic.
    #[arg(
        long,
        env = "GLICKO_EXCLUDE",
        value_delimiter = ',',
        default_value = ".nobody,.self"
    )]
    pub exclude: Vec<String>,

    /// Starting rating for a player with no history.
    #[arg(long, env = "GLICKO_INITIAL_RATING", default_value_t = 1500.0)]
    pub initial_rating: f64,

    /// Starting rating deviation for a player with no history.
    #[arg(long, env = "GLICKO_INITIAL_RD", default_value_t = 350.0)]
    pub initial_rd: f64,

    /// Starting volatility for a player with no history.
    #[arg(long, env = "GLICKO_INITIAL_VOLATILITY", default_value_t = 0.06)]
    pub initial_volatility: f64,

    /// Glicko-2 rating scale.
    #[arg(long, env = "GLICKO_SCALE", default_value_t = 173.7178)]
    pub scale: f64,

    /// Convergence tolerance of the volatility iteration.
    #[arg(long, env = "GLICKO_EPSILON", default_value_t = 1e-6)]
    pub epsilon: f64,

    /// System constant tau, which caps volatility change over time.
    #[arg(long, env = "GLICKO_TAU", default_value_t = 0.5)]
    pub tau: f64,

    /// Maximum accepted request body (event imports are usually the big ones).
    #[arg(long, env = "GLICKO_MAX_BODY_BYTES", default_value_t = 256 * 1024 * 1024)]
    pub max_body_bytes: usize,

    /// Seed an event history from this JSON file at startup (same format as the
    /// import endpoint). Safe to repeat: event ids are de-duplicated.
    #[arg(long, env = "GLICKO_IMPORT_FILE")]
    pub import_file: Option<PathBuf>,

    /// Seed alt-account mappings from this JSON file at startup.
    #[arg(long, env = "GLICKO_ALTS_FILE")]
    pub alts_file: Option<PathBuf>,

    /// Page size used when a request omits `limit`.
    #[arg(long, env = "GLICKO_DEFAULT_PAGE_SIZE", default_value_t = 50)]
    pub default_page_size: usize,

    /// Largest page size a request may ask for.
    #[arg(long, env = "GLICKO_MAX_PAGE_SIZE", default_value_t = 1000)]
    pub max_page_size: usize,
}

impl Config {
    pub fn glicko_params(&self) -> GlickoParams {
        GlickoParams {
            initial_rating: self.initial_rating,
            initial_rd: self.initial_rd,
            initial_volatility: self.initial_volatility,
            scale: self.scale,
            epsilon: self.epsilon,
            tau: self.tau,
        }
    }

    /// Excluded logins, trimmed and with empty entries dropped.
    pub fn excluded_logins(&self) -> HashSet<String> {
        self.exclude
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }

    pub fn events_path(&self) -> PathBuf {
        self.data_dir.join(DEFAULT_EVENTS_FILE)
    }

    pub fn alts_path(&self) -> PathBuf {
        self.data_dir.join(DEFAULT_ALTS_FILE)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.glicko_params().validate()?;
        if self.default_page_size == 0 {
            return Err("default-page-size must be at least 1".into());
        }
        if self.max_page_size == 0 {
            return Err("max-page-size must be at least 1".into());
        }
        if self.default_page_size > self.max_page_size {
            return Err(format!(
                "default-page-size ({}) cannot exceed max-page-size ({})",
                self.default_page_size, self.max_page_size
            ));
        }
        Ok(())
    }
}

/// Reads a JSON file holding a single array of `T`.
pub fn read_json_array<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>, String> {
    let file =
        std::fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    serde_json::from_reader(file).map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config::parse_from(["glicko-api"])
    }

    #[test]
    fn defaults_match_the_specification() {
        let c = config();
        let p = c.glicko_params();
        assert_eq!(p.initial_rating, 1500.0);
        assert_eq!(p.initial_rd, 350.0);
        assert_eq!(p.initial_volatility, 0.06);
        assert_eq!(p.scale, 173.7178);
        assert_eq!(p.epsilon, 1e-6);
        assert_eq!(p.tau, 0.5);
        assert_eq!(
            c.excluded_logins(),
            HashSet::from([".nobody".to_string(), ".self".to_string()])
        );
        c.validate().unwrap();
    }

    #[test]
    fn exclusions_are_configurable() {
        let c = Config::parse_from(["glicko-api", "--exclude", ".nobody, .self ,bot"]);
        assert_eq!(
            c.excluded_logins(),
            HashSet::from([
                ".nobody".to_string(),
                ".self".to_string(),
                "bot".to_string()
            ])
        );
        let c = Config::parse_from(["glicko-api", "--exclude", ""]);
        assert!(c.excluded_logins().is_empty());
    }

    #[test]
    fn rejects_nonsense_parameters() {
        let c = Config::parse_from(["glicko-api", "--tau", "0"]);
        assert!(c.validate().is_err());
        let c = Config::parse_from([
            "glicko-api",
            "--default-page-size",
            "500",
            "--max-page-size",
            "100",
        ]);
        assert!(c.validate().is_err());
    }
}
