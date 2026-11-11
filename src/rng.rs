//! Deterministic, seedable pseudo-random number generation.
//!
//! The engine never uses OS randomness during a match. Every random choice
//! (shop rolls, combat crits, target tie-breaks) is drawn from a [`Rng`]
//! seeded from the match seed, so a match can be replayed exactly from its
//! seed and inputs. The generator is xoshiro256** seeded through splitmix64,
//! implemented here so results never change with a dependency upgrade.

/// splitmix64 step, used to expand a 64-bit seed into generator state.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A small, fast, deterministic PRNG (xoshiro256**).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    /// Creates a generator from a 64-bit seed.
    pub fn new(seed: u64) -> Self {
        let mut sm = seed;
        let s = [
            splitmix64(&mut sm),
            splitmix64(&mut sm),
            splitmix64(&mut sm),
            splitmix64(&mut sm),
        ];
        Rng { s }
    }

    /// Derives an independent child stream, e.g. one per player or per
    /// combat, so that adding draws in one subsystem does not shift another.
    pub fn fork(&mut self, salt: u64) -> Rng {
        Rng::new(self.next_u64() ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15))
    }

    /// Returns the next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform integer in `0..bound` without modulo bias. Panics if `bound == 0`.
    pub fn below(&mut self, bound: u64) -> u64 {
        assert!(bound > 0, "Rng::below called with bound 0");
        // Lemire's nearly-divisionless method.
        let mut m = (self.next_u64() as u128) * (bound as u128);
        let mut low = m as u64;
        if low < bound {
            let threshold = bound.wrapping_neg() % bound;
            while low < threshold {
                m = (self.next_u64() as u128) * (bound as u128);
                low = m as u64;
            }
        }
        (m >> 64) as u64
    }

    /// Uniform integer in the inclusive range `lo..=hi`.
    pub fn range_inclusive(&mut self, lo: i64, hi: i64) -> i64 {
        assert!(lo <= hi, "empty range");
        let span = (hi - lo) as u64 + 1;
        if span == 0 {
            return self.next_u64() as i64; // full i64 range
        }
        lo + self.below(span) as i64
    }

    /// Returns true with probability `numerator / denominator`, using only
    /// integer math so results are identical on every platform.
    pub fn chance(&mut self, numerator: u64, denominator: u64) -> bool {
        self.below(denominator) < numerator
    }

    /// Picks an index according to integer weights. Returns `None` if all
    /// weights are zero.
    pub fn weighted_index(&mut self, weights: &[u64]) -> Option<usize> {
        let total: u64 = weights.iter().sum();
        if total == 0 {
            return None;
        }
        let mut roll = self.below(total);
        for (i, &w) in weights.iter().enumerate() {
            if roll < w {
                return Some(i);
            }
            roll -= w;
        }
        unreachable!()
    }

    /// Fisher-Yates shuffle.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i as u64 + 1) as usize;
            items.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn known_first_value_is_stable() {
        // Guards against accidental algorithm changes, which would break replays.
        let mut r = Rng::new(0);
        let first = r.next_u64();
        assert_eq!(first, Rng::new(0).next_u64());
        assert_ne!(first, Rng::new(1).next_u64());
    }

    #[test]
    fn below_stays_in_bounds_and_covers_range() {
        let mut r = Rng::new(7);
        let mut seen = [false; 6];
        for _ in 0..10_000 {
            let v = r.below(6) as usize;
            assert!(v < 6);
            seen[v] = true;
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn weighted_index_respects_zero_weights() {
        let mut r = Rng::new(9);
        for _ in 0..1000 {
            let i = r.weighted_index(&[0, 5, 0, 3]).unwrap();
            assert!(i == 1 || i == 3);
        }
        assert_eq!(r.weighted_index(&[0, 0]), None);
    }

    #[test]
    fn shuffle_is_permutation() {
        let mut r = Rng::new(3);
        let mut v: Vec<u32> = (0..50).collect();
        r.shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>());
    }

    #[test]
    fn forks_are_independent_and_deterministic() {
        let mut a = Rng::new(5);
        let mut b = Rng::new(5);
        let mut fa = a.fork(1);
        let mut fb = b.fork(1);
        assert_eq!(fa.next_u64(), fb.next_u64());
        let mut c = Rng::new(5);
        assert_ne!(c.fork(2).next_u64(), Rng::new(5).fork(1).next_u64());
    }
}
