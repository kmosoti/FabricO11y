//! Level 1 (integrity): CRC-32, the IEEE 802.3 polynomial in its reflected
//! form, as every ZIP, PNG and Ethernet frame computes it.
//!
//! The definition is the bit-serial division [`bitwise`]: for each bit,
//! shift and conditionally xor the polynomial. [`hash`] is the same
//! function eight bits at a time through a 256-entry table that the
//! compiler fills from the definition (`const fn`); the exhaustive test and
//! the proof check that the two agree on every byte. Known answer:
//! `hash(b"123456789") == 0xCBF4_3926`.
//!
//! A CRC detects corruption; it is not a custody hash. The block carries it
//! so that a damaged byte string is refused before any field is believed.

// Shifts by one and by eight on a 32-bit word; table indices are masked to a byte.
#![allow(clippy::arithmetic_side_effects, clippy::indexing_slicing)]

/// The reflected IEEE polynomial.
pub const POLYNOMIAL: u32 = 0xEDB8_8320;

const fn entry(mut c: u32) -> u32 {
    let mut k = 0;
    while k < 8 {
        c = if c & 1 == 1 {
            POLYNOMIAL ^ (c >> 1)
        } else {
            c >> 1
        };
        k += 1;
    }
    c
}

const fn build() -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut i = 0;
    while i < 256 {
        table[i] = entry(i as u32);
        i += 1;
    }
    table
}

/// Filled at compile time from [`entry`].
pub const TABLE: [u32; 256] = build();

/// The definition: one bit at a time.
pub fn bitwise(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for b in bytes {
        crc ^= u32::from(*b);
        let mut k = 0;
        while k < 8 {
            crc = if crc & 1 == 1 {
                POLYNOMIAL ^ (crc >> 1)
            } else {
                crc >> 1
            };
            k += 1;
        }
    }
    !crc
}

/// The same function, one byte at a time through the table.
pub fn hash(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for b in bytes {
        crc = TABLE[((crc ^ u32::from(*b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn known_answers() {
        assert_eq!(hash(b"123456789"), 0xCBF4_3926);
        assert_eq!(hash(b""), 0);
        assert_eq!(hash(b"a"), 0xE8B7_BE43);
        assert_eq!(bitwise(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn the_table_is_the_definition_for_every_byte() {
        for b in 0..=255_u8 {
            assert_eq!(hash(&[b]), bitwise(&[b]), "byte {b}");
            assert_eq!(TABLE[b as usize], entry(u32::from(b)));
        }
        let data: Vec<u8> = (0..1024_u32).map(|i| (i * 7 + 3) as u8).collect();
        assert_eq!(hash(&data), bitwise(&data));
    }

    #[test]
    fn a_single_flipped_bit_changes_the_check() {
        let data = b"the quick brown fox";
        let base = hash(data);
        for i in 0..data.len() {
            for bit in 0..8 {
                let mut m = data.to_vec();
                m[i] ^= 1 << bit;
                assert_ne!(hash(&m), base);
            }
        }
    }
}
