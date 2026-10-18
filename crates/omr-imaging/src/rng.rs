//! A tiny deterministic generator (SplitMix64) for robust-fit sampling (§8.11: seeded RANSAC only).

/// SplitMix64: fast, seedable, identical on every platform.
#[derive(Clone, Debug)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// A generator from a seed.
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..n` (`n > 0`), by the multiply-shift reduction.
    pub fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next_u64()) * n as u128) >> 64) as usize
    }

    /// `k` distinct indices in `0..n` (`k ≤ n`), in draw order.
    pub fn distinct(&mut self, n: usize, k: usize, out: &mut Vec<usize>) {
        out.clear();
        while out.len() < k.min(n) {
            let i = self.below(n);
            if !out.contains(&i) {
                out.push(i);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reproducible_and_in_range() {
        let mut a = SplitMix64::new(7);
        let mut b = SplitMix64::new(7);
        for _ in 0..100 {
            let x = a.below(13);
            assert_eq!(x, b.below(13));
            assert!(x < 13);
        }
        let mut v = Vec::new();
        a.distinct(5, 5, &mut v);
        let mut s = v.clone();
        s.sort_unstable();
        assert_eq!(s, vec![0, 1, 2, 3, 4]);
    }
}
