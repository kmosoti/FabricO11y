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

const fn build() -> [[u32; 256]; 8] {
    // Slicing-by-eight: table[k][b] is the CRC contribution of byte b placed
    // k bytes further along, derived from table[0] by the shift-and-xor step.
    let mut table = [[0_u32; 256]; 8];
    let mut i = 0;
    while i < 256 {
        table[0][i] = entry(i as u32);
        i += 1;
    }
    let mut k = 1;
    while k < 8 {
        let mut i = 0;
        while i < 256 {
            let prev = table[k - 1][i];
            table[k][i] = table[0][(prev & 0xFF) as usize] ^ (prev >> 8);
            i += 1;
        }
        k += 1;
    }
    table
}

/// Filled at compile time from [`entry`]: `TABLE[0]` is the bytewise table,
/// `TABLE[1..8]` the slicing tables derived from it.
pub const TABLE: [[u32; 256]; 8] = build();

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

/// The same function, eight bytes per step through the slicing tables and
/// one byte per step through `TABLE[0]` for the remainder.
pub fn hash(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    let mut chunks = bytes.chunks_exact(8);
    for c in &mut chunks {
        let lo = crc ^ unpack_le4(c[0], c[1], c[2], c[3]);
        crc = TABLE[7][(lo & 0xFF) as usize]
            ^ TABLE[6][((lo >> 8) & 0xFF) as usize]
            ^ TABLE[5][((lo >> 16) & 0xFF) as usize]
            ^ TABLE[4][(lo >> 24) as usize]
            ^ TABLE[3][c[4] as usize]
            ^ TABLE[2][c[5] as usize]
            ^ TABLE[1][c[6] as usize]
            ^ TABLE[0][c[7] as usize];
    }
    for b in chunks.remainder() {
        crc = TABLE[0][((crc ^ u32::from(*b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

#[inline]
const fn unpack_le4(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
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
            assert_eq!(TABLE[0][b as usize], entry(u32::from(b)));
        }
        let data: Vec<u8> = (0..1024_u32).map(|i| (i * 7 + 3) as u8).collect();
        // every length, so the eight-byte steps and the remainder are both exercised
        for len in 0..=64 {
            assert_eq!(hash(&data[..len]), bitwise(&data[..len]), "length {len}");
        }
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
