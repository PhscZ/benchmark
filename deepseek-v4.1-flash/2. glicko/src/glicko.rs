//! Glicko-2 core maths (Glickman, *Example of the Glicko-2 system*).
//!
//! Pure functions only: no clock, no state, no I/O. The rating engine feeds one
//! game per completed match; the published three-opponent worked example is
//! pinned by the unit tests below.

use std::f64::consts::PI;

/// Inactivity periods are whole days of 86 400 seconds.
pub const SECONDS_PER_DAY: i64 = 86_400;

/// Illinois-algorithm guard; the published algorithm converges in <10 rounds,
/// this only exists so pathological inputs cannot spin forever.
const MAX_VOLATILITY_ITERATIONS: u32 = 1000;
/// Clamp for the logistic expectation so `E * (1 - E)` can never underflow to
/// zero (which would make the estimated variance `v` infinite).
const E_CLAMP: f64 = 1e-15;

/// Rating-system constants. All of them are overridable from the CLI/env; the
/// defaults are the ones requested for this service.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlickoParams {
    pub initial_rating: f64,
    pub initial_rd: f64,
    pub initial_volatility: f64,
    pub scale: f64,
    pub epsilon: f64,
    pub tau: f64,
}

impl Default for GlickoParams {
    fn default() -> Self {
        Self {
            initial_rating: 1500.0,
            initial_rd: 350.0,
            initial_volatility: 0.06,
            scale: 173.7178,
            epsilon: 1e-6,
            tau: 0.5,
        }
    }
}

impl GlickoParams {
    pub fn validate(&self) -> Result<(), String> {
        let checks: [(&str, f64); 6] = [
            ("initial-rating", self.initial_rating),
            ("initial-rd", self.initial_rd),
            ("scale", self.scale),
            ("epsilon", self.epsilon),
            ("tau", self.tau),
            ("initial-volatility", self.initial_volatility),
        ];
        for (name, value) in checks {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!(
                    "{name} must be a finite positive number, got {value}"
                ));
            }
        }
        if self.epsilon >= 1.0 {
            return Err(format!("epsilon must be < 1, got {}", self.epsilon));
        }
        Ok(())
    }
}

/// A player's Glicko-2 state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rating {
    pub rating: f64,
    pub rd: f64,
    pub volatility: f64,
}

/// One rated game: the opponent's state at the time of the game plus the score
/// this player obtained. `score` is 1.0/0.0 for a plain win/loss, and this
/// service uses the margin-of-victory value in between.
#[derive(Debug, Clone, Copy)]
pub struct Game {
    pub opponent_rating: f64,
    pub opponent_rd: f64,
    pub score: f64,
}

/// `g(phi)`, the RD dampening factor.
fn g(phi: f64) -> f64 {
    1.0 / (1.0 + 3.0 * phi * phi / (PI * PI)).sqrt()
}

/// `E(mu, mu_j, phi_j)`, the expected score.
fn expected(mu: f64, mu_j: f64, g_j: f64) -> f64 {
    (1.0 / (1.0 + (-g_j * (mu - mu_j)).exp())).clamp(E_CLAMP, 1.0 - E_CLAMP)
}

/// Runs one rating period for a single player.
///
/// An empty game list is a no-op (no opponent, no update). Non-finite inputs
/// cannot escape: they leave the rating untouched instead of poisoning it.
pub fn update(player: Rating, games: &[Game], params: &GlickoParams) -> Rating {
    if games.is_empty() {
        return player;
    }
    let scale = params.scale;
    let mu = (player.rating - params.initial_rating) / scale;
    let phi = player.rd / scale;
    let sigma = player.volatility;

    // v^-1 = sum g^2 E (1-E) ; delta_sum = sum g (s - E)
    let mut v_inv = 0.0;
    let mut delta_sum = 0.0;
    for game in games {
        let mu_j = (game.opponent_rating - params.initial_rating) / scale;
        let phi_j = game.opponent_rd / scale;
        let g_j = g(phi_j);
        let e = expected(mu, mu_j, g_j);
        v_inv += g_j * g_j * e * (1.0 - e);
        delta_sum += g_j * (game.score - e);
    }
    if !v_inv.is_finite() || v_inv <= 0.0 || !delta_sum.is_finite() {
        return player;
    }
    let v = 1.0 / v_inv;
    let delta = v * delta_sum;

    // --- volatility: Illinois algorithm on f(x) -------------------------
    let a = (sigma * sigma).ln();
    let tau2 = params.tau * params.tau;
    let f = |x: f64| {
        let e_x = x.exp();
        let denom = 2.0 * (phi * phi + v + e_x).powi(2);
        (e_x * (delta * delta - phi * phi - v - e_x)) / denom - (x - a) / tau2
    };

    let mut big_a = a;
    let mut big_b = if delta * delta > phi * phi + v {
        (delta * delta - phi * phi - v).ln()
    } else {
        let mut k = 1.0;
        while f(a - k * params.tau) < 0.0 && k < 100.0 {
            k += 1.0;
        }
        a - k * params.tau
    };
    let mut f_a = f(big_a);
    let mut f_b = f(big_b);
    let mut iterations = 0;
    while (big_b - big_a).abs() > params.epsilon && iterations < MAX_VOLATILITY_ITERATIONS {
        let c = big_a + (big_a - big_b) * f_a / (f_b - f_a);
        let f_c = f(c);
        if f_c * f_b <= 0.0 {
            big_a = big_b;
            f_a = f_b;
        } else {
            f_a /= 2.0;
        }
        big_b = c;
        f_b = f_c;
        iterations += 1;
    }
    let sigma_new = (big_a / 2.0).exp();

    // --- rating / RD ----------------------------------------------------
    let phi_star = (phi * phi + sigma_new * sigma_new).sqrt();
    let phi_new = 1.0 / (1.0 / (phi_star * phi_star) + 1.0 / v).sqrt();
    let mu_new = mu + phi_new * phi_new * delta_sum;

    Rating {
        rating: mu_new * scale + params.initial_rating,
        rd: phi_new * scale,
        volatility: sigma_new,
    }
}

/// Inactivity inflation of RD: `phi' = sqrt(phi^2 + periods * sigma^2)`.
///
/// Only RD moves — rating and volatility are untouched. `periods` is the number
/// of *whole* 24h periods since the player's last registered event.
pub fn inflate_rd(rd: f64, volatility: f64, periods: i64, scale: f64) -> f64 {
    if periods <= 0 {
        return rd;
    }
    let phi = rd / scale;
    let periods = periods as f64;
    (phi * phi + periods * volatility * volatility).sqrt() * scale
}

/// Margin-of-victory scoring used instead of plain 1/0/0.5 outcomes.
///
/// `winScore = 0.5 + 0.5 * (winner - loser) / winner`, `lossScore = 1 - winScore`.
/// A 20-19 win yields 0.525, a 20-0 win yields 1.0. These are the *scores fed to
/// Glicko-2*; the win/loss counters stay integral.
pub fn margin_of_victory(winner_score: u32, loser_score: u32) -> (f64, f64) {
    if winner_score == 0 {
        return (0.5, 0.5);
    }
    let diff = winner_score.saturating_sub(loser_score) as f64;
    let win = 0.5 + 0.5 * diff / winner_score as f64;
    (win, 1.0 - win)
}

/// Kills per death; a player with no deaths is credited with their kill count.
pub fn kd_ratio(kills: u64, deaths: u64) -> f64 {
    if deaths == 0 {
        kills as f64
    } else {
        kills as f64 / deaths as f64
    }
}

/// Wins over played matches; `0.0` when nothing has been played.
pub fn winrate(wins: u64, losses: u64) -> f64 {
    let played = wins + losses;
    if played == 0 {
        0.0
    } else {
        wins as f64 / played as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> GlickoParams {
        GlickoParams::default()
    }

    /// Worked example from Glickman's Glicko-2 note: 1500/200/0.06 against
    /// 1400/30 (win), 1550/100 (loss), 1700/300 (loss) -> 1464.06 / 151.52 / 0.05999.
    #[test]
    fn matches_published_example() {
        let player = Rating {
            rating: 1500.0,
            rd: 200.0,
            volatility: 0.06,
        };
        let games = [
            Game {
                opponent_rating: 1400.0,
                opponent_rd: 30.0,
                score: 1.0,
            },
            Game {
                opponent_rating: 1550.0,
                opponent_rd: 100.0,
                score: 0.0,
            },
            Game {
                opponent_rating: 1700.0,
                opponent_rd: 300.0,
                score: 0.0,
            },
        ];
        let out = update(player, &games, &params());
        assert!((out.rating - 1464.06).abs() < 0.01, "rating {}", out.rating);
        assert!((out.rd - 151.52).abs() < 0.01, "rd {}", out.rd);
        assert!(
            (out.volatility - 0.05999).abs() < 0.0001,
            "sigma {}",
            out.volatility
        );
    }

    #[test]
    fn empty_game_list_is_a_noop() {
        let player = Rating {
            rating: 1600.0,
            rd: 120.0,
            volatility: 0.05,
        };
        assert_eq!(update(player, &[], &params()), player);
    }

    /// Beating a stronger player must move you up more than beating a weaker one.
    #[test]
    fn upset_moves_rating_more() {
        let player = Rating {
            rating: 1500.0,
            rd: 100.0,
            volatility: 0.06,
        };
        let beat_strong = update(
            player,
            &[Game {
                opponent_rating: 1800.0,
                opponent_rd: 100.0,
                score: 1.0,
            }],
            &params(),
        );
        let beat_weak = update(
            player,
            &[Game {
                opponent_rating: 1200.0,
                opponent_rd: 100.0,
                score: 1.0,
            }],
            &params(),
        );
        assert!(beat_strong.rating > beat_weak.rating);
    }

    #[test]
    fn margin_of_victory_spans_the_expected_range() {
        assert!((margin_of_victory(20, 19).0 - 0.525).abs() < 1e-12);
        assert!((margin_of_victory(20, 0).0 - 1.0).abs() < 1e-12);
        assert!((margin_of_victory(20, 10).0 - 0.75).abs() < 1e-12);
        let (w, l) = margin_of_victory(20, 13);
        assert!((w + l - 1.0).abs() < 1e-12);
        // a close win is worth barely more than a coin flip
        assert!(margin_of_victory(20, 19).0 < margin_of_victory(20, 15).0);
    }

    #[test]
    fn inactivity_inflation_uses_whole_days() {
        let scale = 173.7178;
        let rd = 200.0;
        let sigma = 0.06;
        // 23h59m -> no periods
        assert_eq!(inflate_rd(rd, sigma, 0, scale), rd);
        // 2 periods
        let expected = ((rd / scale).powi(2) + 2.0 * sigma * sigma).sqrt() * scale;
        assert!((inflate_rd(rd, sigma, 2, scale) - expected).abs() < 1e-12);
        assert!(inflate_rd(rd, sigma, 2, scale) > rd);
    }

    #[test]
    fn ratios_handle_zero_deaths() {
        assert_eq!(kd_ratio(0, 0), 0.0);
        assert_eq!(kd_ratio(7, 0), 7.0);
        assert_eq!(kd_ratio(6, 3), 2.0);
        assert_eq!(winrate(0, 0), 0.0);
        assert_eq!(winrate(3, 1), 0.75);
    }
}
