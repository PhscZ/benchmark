//! Persisted settings (best score + mute) in a tiny, human-readable format.
//!
//! The core owns parsing/serialising so both backends persist the same shape
//! (a file under `%APPDATA%` on Windows, `localStorage` in the browser).
//! Parsing is deliberately forgiving: anything unrecognised or out of range
//! falls back to a safe default instead of failing, so a corrupted store can
//! never make the game unplayable.

/// Anything above this is treated as corruption rather than a real score.
pub const MAX_PLAUSIBLE_SCORE: u32 = 1_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    pub best: u32,
    pub muted: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            best: 0,
            muted: false,
        }
    }
}

impl Settings {
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "best" => {
                    // Reject negatives, junk and absurd values outright.
                    if let Ok(v) = value.parse::<u32>() {
                        if v <= MAX_PLAUSIBLE_SCORE {
                            s.best = v;
                        }
                    }
                }
                "muted" => match value.to_ascii_lowercase().as_str() {
                    "true" | "1" | "yes" | "on" => s.muted = true,
                    "false" | "0" | "no" | "off" => s.muted = false,
                    _ => {}
                },
                _ => {}
            }
        }
        s
    }

    pub fn serialize(&self) -> String {
        format!("best={}\nmuted={}\n", self.best, self.muted)
    }

    /// Records a run, returning `true` when it set a new best.
    pub fn record_score(&mut self, score: u32) -> bool {
        if score > self.best {
            self.best = score;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = Settings {
            best: 42,
            muted: true,
        };
        assert_eq!(Settings::parse(&s.serialize()), s);
    }

    #[test]
    fn empty_and_garbage_fall_back_to_defaults() {
        assert_eq!(Settings::parse(""), Settings::default());
        assert_eq!(Settings::parse("\u{0}\u{1}not a config"), Settings::default());
        assert_eq!(Settings::parse("best=abc\nmuted=maybe"), Settings::default());
        assert_eq!(Settings::parse("best=-5"), Settings::default());
    }

    #[test]
    fn absurd_scores_are_rejected() {
        assert_eq!(Settings::parse("best=99999999999999999999").best, 0);
        assert_eq!(Settings::parse("best=4000000000").best, 0);
        assert_eq!(
            Settings::parse(&format!("best={}", MAX_PLAUSIBLE_SCORE)).best,
            MAX_PLAUSIBLE_SCORE
        );
    }

    #[test]
    fn tolerates_whitespace_crlf_and_unknown_keys() {
        let s = Settings::parse("  BEST = 7 \r\nmuted = YES\r\nfuture=1\r\n# comment\r\n");
        assert_eq!(s.best, 7);
        assert!(s.muted);
    }

    #[test]
    fn record_score_only_raises() {
        let mut s = Settings::default();
        assert!(s.record_score(3));
        assert!(!s.record_score(3));
        assert!(!s.record_score(1));
        assert_eq!(s.best, 3);
    }
}
