//! Showdown's PRNG, bit for bit: the Gen 5 linear congruential generator (`sim/prng.ts`,
//! `Gen5RNG`) and the helpers built on it (`random`, `randomChance`, `sample`, `shuffle`).
//! Same seed + same calls in the same order = the same numbers as Showdown.

#[derive(Clone, Debug)]
pub struct Prng {
    /// 64-bit state (Showdown keeps it as four 16-bit words, high first)
    state: u64,
    /// optional record of every draw: (range low, range high, result, log length) for checking
    /// against Showdown
    pub trace: Option<Vec<(u32, u32, u32, u32)>>,
    /// the battle log's length (kept up to date by the battle when tracing)
    pub log_len: u32,
}

impl Prng {
    /// From Showdown's four 16-bit seed words, e.g. `[1, 2, 3, 4]`.
    pub fn from_words(w: [u16; 4]) -> Prng {
        let state = ((w[0] as u64) << 48) | ((w[1] as u64) << 32) | ((w[2] as u64) << 16) | w[3] as u64;
        Prng { state, trace: None, log_len: 0 }
    }
    pub fn from_u64(seed: u64) -> Prng {
        Prng { state: seed, trace: None, log_len: 0 }
    }
    pub fn words(&self) -> [u16; 4] {
        [(self.state >> 48) as u16, (self.state >> 32) as u16, (self.state >> 16) as u16, self.state as u16]
    }

    /// next raw 32-bit number (upper half of the advanced state)
    #[inline]
    fn next(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(0x5D588B656C078965).wrapping_add(0x269EC3);
        (self.state >> 32) as u32
    }

    #[inline]
    fn record(&mut self, lo: u32, hi: u32, r: u32) {
        if let Some(t) = &mut self.trace {
            t.push((lo, hi, r, self.log_len));
            // debugging aid: VGCSIM_RNG_BT=<call index> prints where that draw came from
            if let Ok(v) = std::env::var("VGCSIM_RNG_BT") {
                if v.parse::<usize>().ok() == Some(t.len() - 1) {
                    let bt = std::backtrace::Backtrace::force_capture().to_string();
                    for l in bt.lines().filter(|l| l.contains("vgcsim::")).take(14) {
                        eprintln!("  rng bt: {}", l.trim());
                    }
                }
            }
        }
    }

    /// `random()`: a real number in [0, 1)
    pub fn random_float(&mut self) -> f64 {
        let r = self.next();
        self.record(0, 0, r);
        r as f64 / 4294967296.0
    }

    /// `random(n)`: an integer in [0, n)
    #[inline]
    pub fn random(&mut self, n: u32) -> u32 {
        let r = self.next();
        let v = ((r as u64 * n as u64) >> 32) as u32;
        self.record(0, n, v);
        v
    }

    /// `random(m, n)`: an integer in [m, n)
    #[inline]
    pub fn random_range(&mut self, m: u32, n: u32) -> u32 {
        let r = self.next();
        let v = ((r as u64 * (n - m) as u64) >> 32) as u32 + m;
        self.record(m, n, v);
        v
    }

    /// `randomChance(numerator, denominator)`
    #[inline]
    pub fn chance(&mut self, num: u32, den: u32) -> bool {
        self.random(den) < num
    }

    /// `sample(items)`: index of a uniformly chosen element
    #[inline]
    pub fn sample_index(&mut self, len: usize) -> usize {
        self.random(len as u32) as usize
    }

    /// Fisher-Yates shuffle of `items[start..end]`, exactly as Showdown does it.
    pub fn shuffle<T>(&mut self, items: &mut [T], mut start: usize, end: usize) {
        while start + 1 < end {
            let next = self.random_range(start as u32, end as u32) as usize;
            if start != next {
                items.swap(start, next);
            }
            start += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gen5_lcg_matches_showdown() {
        // Showdown: new PRNG('1,2,3,4'): first next() values
        let mut p = Prng::from_words([1, 2, 3, 4]);
        let a = p.next();
        // seed * a + c with seed = 0x0001000200030004
        let s = 0x0001000200030004u64.wrapping_mul(0x5D588B656C078965).wrapping_add(0x269EC3);
        assert_eq!(a, (s >> 32) as u32);
    }
}
