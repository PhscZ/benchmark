//! Deterministic PRNG so gameplay is identical on native and wasm without
//! pulling in `rand`/`getrandom` (which needs different backends per target).
//!
//! PCG-XSH-RR 32/64: tiny, fast, well-distributed. Seeded from a platform
//! clock by the backends; tests seed it explicitly.

#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
    inc: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut r = Rng {
            state: 0,
            inc: (seed << 1) | 1,
        };
        r.next_u32();
        r.state = r.state.wrapping_add(seed);
        r.next_u32();
        r
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old
            .wrapping_mul(6364136223846793005)
            .wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform in `[lo, hi]` inclusive. Returns `lo` if the range is empty.
    #[inline]
    pub fn range_i32(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        let span = (hi - lo) as u32 + 1;
        lo + (self.next_u32() % span) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_in_range_and_varies() {
        let mut r = Rng::new(0xC0FFEE);
        let mut seen_low = false;
        let mut seen_high = false;
        for _ in 0..10_000 {
            let v = r.range_i32(10, 20);
            assert!((10..=20).contains(&v), "out of range: {v}");
            seen_low |= v == 10;
            seen_high |= v == 20;
        }
        assert!(seen_low && seen_high, "distribution never hit the bounds");
    }

    #[test]
    fn same_seed_same_stream() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..64 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }

    #[test]
    fn empty_range_is_safe() {
        let mut r = Rng::new(7);
        assert_eq!(r.range_i32(5, 5), 5);
        assert_eq!(r.range_i32(9, 3), 9);
    }
}
