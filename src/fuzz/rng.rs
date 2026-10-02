//! Tiny deterministic PRNG used by the fuzzer.
//!
//! SplitMix64 / xorshift64* — enough for property tests, no `rand` crate.

fn splitmix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[derive(Debug, Clone)]
pub struct FuzzRng {
    state: u64,
}

impl FuzzRng {
    /// The seed is mixed through one SplitMix64 step so that every seed
    /// (including 0 and adjacent pairs 2k / 2k+1) starts a distinct stream.
    /// (An earlier `seed | 1` made seeds 2k and 2k+1 run identical campaigns.)
    pub fn new(seed: u64) -> Self {
        FuzzRng {
            state: splitmix(seed),
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn int(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        let span = (hi as i64) - (lo as i64) + 1;
        lo + (self.next_u64() % span as u64) as i32
    }

    pub fn bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }

    pub fn choose<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.int(0, (items.len() - 1) as i32) as usize]
    }

    pub fn random_bytes(&mut self, n: usize) -> String {
        let mut buf = vec![0u8; n];
        for b in &mut buf {
            *b = (self.next_u64() & 0xFF) as u8;
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    pub fn random_ascii(&mut self, n: usize) -> String {
        let mut s = String::with_capacity(n);
        for _ in 0..n {
            let c = self.int(32, 126) as u8 as char;
            s.push(c);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacent_seeds_differ_and_zero_works() {
        let first = |s: u64| FuzzRng::new(s).next_u64();
        for k in 0..64u64 {
            assert_ne!(first(2 * k), first(2 * k + 1), "seeds {} and {}", 2 * k, 2 * k + 1);
        }
        assert_ne!(first(0), first(1));
        assert_ne!(first(0), 0);
    }
}
