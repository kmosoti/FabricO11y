//! Level 3: deltas, the bijection that turns a monotone or slowly moving
//! sequence of `u64` into small codes.
//!
//! `step(a, b)` is `a - b` taken wrapping and zigzagged, so it is total and
//! invertible for every pair; `unstep(d, b)` recovers `a`. A sequence is
//! coded as its first value and then one step per element; a regular series
//! (timestamps at fixed intervals) is coded as the step of the steps, so a
//! constant interval costs one byte per element. Nothing is lost at a wrap:
//! the arithmetic is modular and the inverse is exact (Kani proves the pair).

use crate::zigzag;

/// The code for `a` given the previous value `b`.
pub fn step(a: u64, b: u64) -> u64 {
    zigzag::encode(a.wrapping_sub(b) as i64)
}

/// The value whose code given `b` is `d`.
pub fn unstep(d: u64, b: u64) -> u64 {
    b.wrapping_add(zigzag::decode(d) as u64)
}

/// Codes a sequence as first value, then second-order steps.
pub struct SecondOrderEncoder {
    prev: u64,
    prev_step: u64,
    first: bool,
}

impl Default for SecondOrderEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SecondOrderEncoder {
    pub fn new() -> Self {
        Self {
            prev: 0,
            prev_step: 0,
            first: true,
        }
    }
    /// The code to write for the next value.
    pub fn code(&mut self, value: u64) -> u64 {
        let code = if self.first {
            self.first = false;
            value
        } else {
            let d = value.wrapping_sub(self.prev);
            let c = step(d, self.prev_step);
            self.prev_step = d;
            c
        };
        self.prev = value;
        code
    }
}

/// The inverse of [`SecondOrderEncoder`].
pub struct SecondOrderDecoder {
    prev: u64,
    prev_step: u64,
    first: bool,
}

impl Default for SecondOrderDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SecondOrderDecoder {
    pub fn new() -> Self {
        Self {
            prev: 0,
            prev_step: 0,
            first: true,
        }
    }
    /// The value a code stands for.
    pub fn value(&mut self, code: u64) -> u64 {
        let value = if self.first {
            self.first = false;
            code
        } else {
            let d = unstep(code, self.prev_step);
            self.prev_step = d;
            self.prev.wrapping_add(d)
        };
        self.prev = value;
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_inverts_for_every_sample_pair() {
        for (a, b) in [(0_u64, 0_u64), (5, 9), (u64::MAX, 0), (0, u64::MAX), (7, 7)] {
            assert_eq!(unstep(step(a, b), b), a);
        }
        assert_eq!(step(10, 7), zigzag::encode(3));
    }

    #[test]
    fn a_regular_series_costs_one_small_code_per_element() {
        let series: Vec<u64> = (0..10).map(|i| 1_000 + i * 15).collect();
        let mut enc = SecondOrderEncoder::new();
        let codes: Vec<u64> = series.iter().map(|v| enc.code(*v)).collect();
        assert_eq!(codes[0], 1_000);
        assert_eq!(codes[1], zigzag::encode(15));
        assert!(
            codes[2..].iter().all(|c| *c == 0),
            "constant interval codes to zero"
        );
        let mut dec = SecondOrderDecoder::new();
        let back: Vec<u64> = codes.iter().map(|c| dec.value(*c)).collect();
        assert_eq!(back, series);
    }

    #[test]
    fn wrapping_sequences_round_trip() {
        let series = [u64::MAX, 0, u64::MAX, 3, 1, u64::MAX / 2];
        let mut enc = SecondOrderEncoder::new();
        let mut dec = SecondOrderDecoder::new();
        for v in series {
            assert_eq!(dec.value(enc.code(v)), v);
        }
    }
}
