//! Level 2: zigzag, the bijection between signed and unsigned 64-bit values.
//!
//! `0, -1, 1, -2, 2, ...` map to `0, 1, 2, 3, 4, ...`, so a small magnitude
//! of either sign becomes a short varint. The contract is that `encode` and
//! `decode` are inverses on the whole of `i64` and `u64`, which the Kani
//! harness proves for every value.

/// Shifts by constants below the width cannot overflow; `wrapping_neg` is
/// total. The pair is proved inverse in `proofs.rs`.
#[allow(clippy::arithmetic_side_effects)]
pub fn encode(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

#[allow(clippy::arithmetic_side_effects)]
pub fn decode(v: u64) -> i64 {
    ((v >> 1) as i64) ^ ((v & 1) as i64).wrapping_neg()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_magnitudes_map_to_small_codes() {
        assert_eq!(encode(0), 0);
        assert_eq!(encode(-1), 1);
        assert_eq!(encode(1), 2);
        assert_eq!(encode(-2), 3);
        assert_eq!(encode(i64::MAX), u64::MAX - 1);
        assert_eq!(encode(i64::MIN), u64::MAX);
    }

    #[test]
    fn inverse_on_the_edges() {
        for v in [0_i64, 1, -1, i64::MAX, i64::MIN, 12_345, -98_765] {
            assert_eq!(decode(encode(v)), v);
        }
        for u in [0_u64, 1, 2, u64::MAX, u64::MAX - 1, 1 << 63] {
            assert_eq!(encode(decode(u)), u);
        }
    }
}
