//! Procedurally synthesised sound effects.
//!
//! No audio files are shipped or downloaded: every effect is generated from
//! oscillators and noise at startup, so the project contains zero third-party
//! assets. The same sample buffers feed `rodio` on the desktop and Web Audio on
//! the web.

use crate::rng::Rng;

pub const SAMPLE_RATE: u32 = 44_100;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sound {
    Flap,
    Score,
    Hit,
    Click,
}

impl Sound {
    pub const ALL: [Sound; 4] = [Sound::Flap, Sound::Score, Sound::Hit, Sound::Click];

    #[inline]
    fn index(self) -> usize {
        match self {
            Sound::Flap => 0,
            Sound::Score => 1,
            Sound::Hit => 2,
            Sound::Click => 3,
        }
    }
}

/// Set of sounds produced by a single input/step, so a frame never plays the
/// same effect twice and backends stay allocation-free in the hot path.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct SoundSet(u8);

impl SoundSet {
    pub const EMPTY: SoundSet = SoundSet(0);

    #[inline]
    pub fn insert(&mut self, s: Sound) {
        self.0 |= 1 << s.index();
    }

    #[inline]
    pub fn contains(self, s: Sound) -> bool {
        self.0 & (1 << s.index()) != 0
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn merge(&mut self, other: SoundSet) {
        self.0 |= other.0;
    }
}

/// One mono `f32` buffer per effect, indexed by [`Sound::index`].
pub struct SoundBank {
    pub sample_rate: u32,
    pub buffers: [Vec<f32>; 4],
}

impl SoundBank {
    pub fn generate(sample_rate: u32) -> Self {
        SoundBank {
            sample_rate,
            buffers: [
                flap(sample_rate),
                score(sample_rate),
                hit(sample_rate),
                click(sample_rate),
            ],
        }
    }

    #[inline]
    pub fn get(&self, s: Sound) -> &[f32] {
        &self.buffers[s.index()]
    }
}

#[inline]
fn frames(sample_rate: u32, seconds: f32) -> usize {
    (sample_rate as f32 * seconds) as usize
}

/// Attack/decay envelope. Both ends are zero so buffers never click.
#[inline]
fn envelope(i: usize, total: usize, attack: f32) -> f32 {
    let t = i as f32 / total as f32;
    let a = if attack <= 0.0 { 0.0 } else { (t / attack).min(1.0) };
    let d = ((1.0 - t) / (1.0 - attack).max(1e-6)).clamp(0.0, 1.0);
    a * d * d
}

fn flap(sample_rate: u32) -> Vec<f32> {
    let n = frames(sample_rate, 0.10);
    let mut out = Vec::with_capacity(n);
    let mut rng = Rng::new(0x5EED_0001);
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / n as f32;
        // Upward sweep: reads as a wing beat rather than a beep.
        let freq = 520.0 + 900.0 * t;
        phase += freq / sample_rate as f32;
        if phase >= 1.0 {
            phase -= 1.0;
        }
        let tri = 4.0 * (phase - 0.5).abs() - 1.0;
        let air = (rng.next_u32() as f32 / u32::MAX as f32) * 2.0 - 1.0;
        let air = air * 0.18 * (1.0 - t);
        out.push((tri * 0.5 + air) * envelope(i, n, 0.05) * 0.55);
    }
    out
}

fn score(sample_rate: u32) -> Vec<f32> {
    let n = frames(sample_rate, 0.16);
    let mut out = Vec::with_capacity(n);
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / n as f32;
        let freq = if t < 0.45 { 880.0 } else { 1318.5 };
        phase += freq / sample_rate as f32;
        if phase >= 1.0 {
            phase -= 1.0;
        }
        let sq = if phase < 0.5 { 1.0 } else { -1.0 };
        out.push(sq * 0.30 * envelope(i, n, 0.03));
    }
    out
}

fn hit(sample_rate: u32) -> Vec<f32> {
    let n = frames(sample_rate, 0.30);
    let mut out = Vec::with_capacity(n);
    let mut rng = Rng::new(0x5EED_0002);
    // One-pole low-pass whose cutoff falls over time => "thud + debris".
    let mut lp = 0.0f32;
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / n as f32;
        let noise = (rng.next_u32() as f32 / u32::MAX as f32) * 2.0 - 1.0;
        let cutoff = 0.55 * (1.0 - t).powi(2) + 0.03;
        lp += (noise - lp) * cutoff;
        let thud_freq = 190.0 - 90.0 * t;
        phase += thud_freq / sample_rate as f32;
        if phase >= 1.0 {
            phase -= 1.0;
        }
        let thud = (phase * std::f32::consts::TAU).sin();
        out.push((lp * 0.55 + thud * 0.45) * envelope(i, n, 0.01) * 0.75);
    }
    out
}

fn click(sample_rate: u32) -> Vec<f32> {
    let n = frames(sample_rate, 0.045);
    let mut out = Vec::with_capacity(n);
    let mut phase = 0.0f32;
    for i in 0..n {
        let freq = 1400.0;
        phase += freq / sample_rate as f32;
        if phase >= 1.0 {
            phase -= 1.0;
        }
        let sq = if phase < 0.5 { 1.0 } else { -1.0 };
        out.push(sq * 0.22 * envelope(i, n, 0.05));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_effect_is_well_formed() {
        let bank = SoundBank::generate(SAMPLE_RATE);
        for s in Sound::ALL {
            let buf = bank.get(s);
            assert!(!buf.is_empty(), "{s:?} produced no samples");
            assert!(
                buf.iter().all(|v| v.is_finite() && v.abs() <= 1.0),
                "{s:?} left the [-1,1] range"
            );
            assert!(
                buf[0].abs() < 0.05 && buf[buf.len() - 1].abs() < 0.05,
                "{s:?} does not start/end near silence (would click)"
            );
            let peak = buf.iter().fold(0.0f32, |a, b| a.max(b.abs()));
            assert!(peak > 0.05, "{s:?} is effectively silent");
        }
    }

    #[test]
    fn sound_set_tracks_membership() {
        let mut set = SoundSet::EMPTY;
        assert!(set.is_empty());
        set.insert(Sound::Score);
        assert!(set.contains(Sound::Score));
        assert!(!set.contains(Sound::Hit));
        let mut other = SoundSet::EMPTY;
        other.insert(Sound::Hit);
        set.merge(other);
        assert!(set.contains(Sound::Score) && set.contains(Sound::Hit));
    }

    #[test]
    fn generation_is_deterministic() {
        let a = SoundBank::generate(SAMPLE_RATE);
        let b = SoundBank::generate(SAMPLE_RATE);
        assert_eq!(a.get(Sound::Hit), b.get(Sound::Hit));
    }
}
