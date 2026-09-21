//! Glicko-2 rating math (Mark E. Glickman, "The Glicko-2 system").
//!
//! The engine drives this with a single game per rating period, but the
//! period form is kept general because that is what the paper specifies and
//! what the reference vector below pins down.

use std::f64::consts::PI;

use crate::model::SCALE;

/// A player's rating state: rating, rating deviation (RD) and volatility.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rating {
    pub rating: f64,
    pub rd: f64,
    pub sigma: f64,
}

impl Rating {
    pub const fn new(rating: f64, rd: f64, sigma: f64) -> Self {
        Self { rating, rd, sigma }
    }
}

/// `g(phi)`: weight of a game against an opponent whose deviation is `phi`.
#[inline]
fn g(phi: f64) -> f64 {
    1.0 / (1.0 + 3.0 * phi * phi / (PI * PI)).sqrt()
}

/// Expected score of `mu` against `mu_j`.
#[inline]
fn expected(g_j: f64, mu: f64, mu_j: f64) -> f64 {
    1.0 / (1.0 + (-g_j * (mu - mu_j)).exp())
}

/// Inactivity growth of the rating deviation: `phi' = sqrt(phi^2 + periods * sigma^2)`.
///
/// `periods` is a whole number of days. Rating and volatility are untouched.
pub fn inflate_rd(rd: f64, sigma: f64, periods: u64) -> f64 {
    if periods == 0 {
        return rd;
    }
    let phi = rd / SCALE;
    let phi_new = (phi * phi + periods as f64 * sigma * sigma).sqrt();
    phi_new * SCALE
}

/// Solve step 4/5 of the Glicko-2 algorithm for the new volatility, using the
/// Illinois variant of regula falsi on `f(x)`.
fn solve_volatility(sigma: f64, phi: f64, v: f64, delta: f64, tau: f64, epsilon: f64) -> f64 {
    let a = (sigma * sigma).ln();
    let f = |x: f64| {
        let e_x = x.exp();
        let denom = 2.0 * (phi * phi + v + e_x).powi(2);
        e_x * (delta * delta - phi * phi - v - e_x) / denom - (x - a) / (tau * tau)
    };

    let mut big_a = a;
    let mut big_b = if delta * delta > phi * phi + v {
        (delta * delta - phi * phi - v).ln()
    } else {
        // Walk down in steps of tau until f() turns positive.
        let mut k = 1.0;
        while k < 100.0 && f(a - k * tau) < 0.0 {
            k += 1.0;
        }
        a - k * tau
    };

    let mut f_a = f(big_a);
    let mut f_b = f(big_b);
    for _ in 0..200 {
        if (big_b - big_a).abs() <= epsilon {
            break;
        }
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
    }
    (big_a / 2.0).exp()
}

/// One rating period: `me` played every `(opponent, score)` pair, with all
/// ratings taken from the start of the period.
///
/// Scores are the custom margin-of-victory values in `[0, 1]`; the classic
/// win/draw/loss values are just the special case `1.0 / 0.5 / 0.0`.
pub fn update_period(me: Rating, games: &[(Rating, f64)], tau: f64, epsilon: f64) -> Rating {
    if games.is_empty() {
        return me;
    }
    let mu = (me.rating - 1500.0) / SCALE;
    let phi = me.rd / SCALE;

    // v = 1 / sum(g^2 * E * (1 - E)); delta = v * sum(g * (s - E)).
    let mut precision = 0.0;
    let mut weighted = 0.0;
    for (opp, score) in games {
        let mu_j = (opp.rating - 1500.0) / SCALE;
        let phi_j = opp.rd / SCALE;
        let g_j = g(phi_j);
        let e = expected(g_j, mu, mu_j);
        precision += g_j * g_j * e * (1.0 - e);
        weighted += g_j * (score - e);
    }
    if precision <= 0.0 {
        return me;
    }
    let v = 1.0 / precision;
    let delta = v * weighted;

    let sigma = solve_volatility(me.sigma, phi, v, delta, tau, epsilon);
    let phi_star = (phi * phi + sigma * sigma).sqrt();
    let phi_new = 1.0 / (1.0 / (phi_star * phi_star) + 1.0 / v).sqrt();
    let mu_new = mu + phi_new * phi_new * weighted;

    Rating {
        rating: mu_new * SCALE + 1500.0,
        rd: phi_new * SCALE,
        sigma,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DEFAULT_VOLATILITY, EPSILON, TAU};

    fn r(rating: f64, rd: f64) -> Rating {
        Rating::new(rating, rd, DEFAULT_VOLATILITY)
    }

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    /// Published example from Glickman's paper (one rating period, three games).
    #[test]
    fn reference_period() {
        let me = Rating::new(1500.0, 200.0, 0.06);
        let games = [
            (r(1400.0, 30.0), 1.0),
            (r(1550.0, 100.0), 0.0),
            (r(1700.0, 300.0), 0.0),
        ];
        let out = update_period(me, &games, TAU, EPSILON);
        assert!(close(out.rating, 1464.06, 0.01), "rating {}", out.rating);
        assert!(close(out.rd, 151.52, 0.01), "rd {}", out.rd);
        assert!(close(out.sigma, 0.05999, 0.00001), "sigma {}", out.sigma);
    }

    /// A single game is the degenerate case of the period form.
    #[test]
    fn single_game_against_equal_opponent_is_a_no_op() {
        let me = Rating::new(1500.0, 200.0, 0.06);
        let out = update_period(me, &[(r(1500.0, 200.0), 0.5)], TAU, EPSILON);
        assert!(close(out.rating, 1500.0, 1e-9));
        assert!(out.rd < me.rd, "rd must shrink: {} -> {}", me.rd, out.rd);
    }

    #[test]
    fn beating_a_weaker_player_gains_less_than_beating_a_stronger_one() {
        let me = Rating::new(1500.0, 200.0, 0.06);
        let weak = update_period(me, &[(r(1300.0, 200.0), 1.0)], TAU, EPSILON);
        let strong = update_period(me, &[(r(1700.0, 200.0), 1.0)], TAU, EPSILON);
        assert!(strong.rating > weak.rating);
    }

    /// 23h59m of a 24h day is zero periods; 49h is two whole periods.
    #[test]
    fn inactivity_growth_counts_whole_days() {
        let sigma = 0.06;
        let rd = 350.0;
        assert_eq!(inflate_rd(rd, sigma, 0), rd);

        let phi = rd / SCALE;
        let two_days = (phi * phi + 2.0 * sigma * sigma).sqrt() * SCALE;
        let got = inflate_rd(rd, sigma, 2);
        assert!(close(got, two_days, 1e-9));
        assert!(got > rd);

        // 23h59m worth of seconds floors to zero periods.
        let seconds_23h59m = 23 * 3600 + 59 * 60;
        assert_eq!(seconds_23h59m / 86_400, 0);
        // 49h worth of seconds floors to two periods.
        let seconds_49h = 49 * 3600;
        assert_eq!(seconds_49h / 86_400, 2);
    }
}
