//! Level 0: bits.
//!
//! The floor of the tower: arithmetic on machine words, written as the
//! shifts and masks a processor executes. Everything above is built from
//! these five operations; nothing here allocates, branches on data beyond
//! a loop bound, or calls anything but the integer instructions.
//!
//! The standard library's `to_le_bytes` and `from_le_bytes` are not used
//! by the codec; they are the oracle the proofs and properties check these
//! functions against for every value.

// Shifts are by constants below the word width and masks cannot overflow;
// the array indices are loop counters bounded by the array length. The
// proofs in proofs.rs state the equivalence with the standard library.
#![allow(clippy::arithmetic_side_effects, clippy::indexing_slicing)]

/// The low seven bits, the value group of a varint byte.
#[inline]
pub const fn lo7(v: u64) -> u8 {
    (v & 0x7f) as u8
}

/// The word without its low seven bits.
#[inline]
pub const fn shr7(v: u64) -> u64 {
    v >> 7
}

/// A mask of the low `n` bits (`n` at most 64).
#[inline]
pub const fn mask(n: u32) -> u64 {
    if n >= 64 { u64::MAX } else { (1_u64 << n) - 1 }
}

/// Whether bit `i` of `byte` is set.
#[inline]
pub const fn bit(byte: u8, i: u8) -> bool {
    (byte >> i) & 1 == 1
}

/// The low `N` bytes of `v`, least significant first.
#[inline]
pub const fn pack_le<const N: usize>(v: u64) -> [u8; N] {
    let mut out = [0_u8; N];
    let mut i = 0;
    while i < N && i < 8 {
        out[i] = (v >> (8 * i)) as u8;
        i += 1;
    }
    out
}

/// The word whose low bytes are `bytes`, least significant first (at most eight).
#[inline]
pub const fn unpack_le(bytes: &[u8]) -> u64 {
    let mut v = 0_u64;
    let mut i = 0;
    while i < bytes.len() && i < 8 {
        v |= (bytes[i] as u64) << (8 * i);
        i += 1;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_and_masks() {
        assert_eq!(lo7(0x1FF), 0x7F);
        assert_eq!(shr7(0x1FF), 3);
        assert_eq!(mask(0), 0);
        assert_eq!(mask(7), 0x7F);
        assert_eq!(mask(64), u64::MAX);
        assert!(bit(0b1000_0000, 7));
        assert!(!bit(0b0111_1111, 7));
    }

    #[test]
    fn packing_agrees_with_the_standard_library_on_the_edges() {
        for v in [0_u64, 1, 0xFF, 0x0102_0304_0506_0708, u64::MAX, 1 << 63] {
            assert_eq!(pack_le::<8>(v), v.to_le_bytes());
            assert_eq!(unpack_le(&v.to_le_bytes()), v);
            assert_eq!(pack_le::<4>(v), (v as u32).to_le_bytes());
            assert_eq!(unpack_le(&(v as u32).to_le_bytes()), u64::from(v as u32));
        }
        assert_eq!(unpack_le(&[]), 0);
        assert_eq!(unpack_le(&[1, 2]), 0x0201);
    }
}
