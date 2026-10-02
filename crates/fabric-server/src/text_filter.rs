//! The per-row-group trigram filter of a Segment's logs table
//! ([ADR-0024](../../../docs/decisions/ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md), part 2).
//!
//! For each row group, a Bloom filter over the distinct byte trigrams of its bodies.
//! A `contains` needle of three or more bytes can occur in a body only if every one of
//! its trigrams occurs there, so a group whose filter lacks one of the needle's
//! trigrams holds no match and the walk may skip it. A Bloom filter has false
//! positives (a group is read needlessly) and no false negatives, as long as its bytes
//! are the ones the sealer wrote; the reader therefore uses a filter only after its
//! SHA-256 matches the manifest, and scans exactly otherwise (the contract's rule for
//! optional indexes).
//!
//! File `text_filter.bin`, little-endian:
//!
//! ```text
//! "FTF1" | u32 groups G | G × u32 bits m_i | G × (m_i / 8) bytes of bits
//! ```
//!
//! Each `m_i` is a power of two in `[2^12, 2^20]`, chosen as about eight bits per
//! distinct trigram of the group, so the chance that one absent trigram passes is
//! (1 − e^(−2/8))² ≈ 5 % and falls geometrically with the needle's trigrams. Two
//! positions per trigram, from two multiplicative hashes of its 24 bits. The decoder
//! accepts only this form: the right magic, every `m_i` in range, and the exact length.

use std::collections::HashSet;
use std::io;

pub const FILE: &str = "text_filter.bin";
const MAGIC: &[u8; 4] = b"FTF1";
const MIN_BITS: usize = 1 << 12;
const MAX_BITS: usize = 1 << 20;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

fn trigram(w: &[u8]) -> u32 {
    u32::from(w[0]) | u32::from(w[1]) << 8 | u32::from(w[2]) << 16
}

fn positions(t: u32, bits: usize) -> (usize, usize) {
    let mask = bits - 1;
    let a = (t.wrapping_mul(0x9E37_79B1) >> 8) as usize & mask;
    let b = (t
        .wrapping_mul(0x85EB_CA6B)
        .rotate_left(13)
        .wrapping_mul(0xC2B2_AE35)
        >> 8) as usize
        & mask;
    (a, b)
}

/// The filter of one row group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupFilter {
    words: Vec<u64>,
}

impl GroupFilter {
    /// The filter of the bodies of one row group.
    pub fn build<'a>(bodies: impl IntoIterator<Item = &'a str>) -> Self {
        let mut distinct: HashSet<u32> = HashSet::new();
        for body in bodies {
            for w in body.as_bytes().windows(3) {
                distinct.insert(trigram(w));
            }
        }
        let bits = (distinct.len() * 8)
            .next_power_of_two()
            .clamp(MIN_BITS, MAX_BITS);
        let mut words = vec![0_u64; bits / 64];
        for t in distinct {
            let (a, b) = positions(t, bits);
            words[a / 64] |= 1 << (a % 64);
            words[b / 64] |= 1 << (b % 64);
        }
        Self { words }
    }

    fn bits(&self) -> usize {
        self.words.len() * 64
    }

    fn has(&self, p: usize) -> bool {
        self.words[p / 64] & (1 << (p % 64)) != 0
    }

    /// False only if no body of the group contains `needle`. A needle shorter than
    /// three bytes has no trigram and may match anywhere.
    pub fn may_contain(&self, needle: &[u8]) -> bool {
        let bits = self.bits();
        needle.windows(3).all(|w| {
            let (a, b) = positions(trigram(w), bits);
            self.has(a) && self.has(b)
        })
    }
}

/// The bytes of `text_filter.bin` for the given groups, in row-group order.
pub fn encode(groups: &[GroupFilter]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + groups.iter().map(|g| 4 + g.bits() / 8).sum::<usize>());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(groups.len() as u32).to_le_bytes());
    for g in groups {
        out.extend_from_slice(&(g.bits() as u32).to_le_bytes());
    }
    for g in groups {
        for w in &g.words {
            out.extend_from_slice(&w.to_le_bytes());
        }
    }
    out
}

/// The groups of a `text_filter.bin`, refusing any other form.
pub fn decode(bytes: &[u8]) -> io::Result<Vec<GroupFilter>> {
    let word = |at: usize| -> io::Result<u32> {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| invalid("text filter truncated"))
    };
    if bytes.get(..4) != Some(MAGIC.as_slice()) {
        return Err(invalid("text filter magic differs"));
    }
    let groups = word(4)? as usize;
    let mut sizes = Vec::with_capacity(groups.min(1 << 16));
    let mut at = 8;
    for _ in 0..groups {
        let bits = word(at)? as usize;
        if !bits.is_power_of_two() || !(MIN_BITS..=MAX_BITS).contains(&bits) {
            return Err(invalid("text filter group size out of range"));
        }
        sizes.push(bits);
        at += 4;
    }
    let expected = at + sizes.iter().map(|b| b / 8).sum::<usize>();
    if bytes.len() != expected {
        return Err(invalid("text filter length differs from its header"));
    }
    let mut out = Vec::with_capacity(groups);
    for bits in sizes {
        let words = bytes[at..at + bits / 8]
            .chunks_exact(8)
            .map(|c| u64::from_le_bytes(c.try_into().expect("eight bytes")))
            .collect();
        out.push(GroupFilter { words });
        at += bits / 8;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_never_rejects_a_substring_of_its_bodies() {
        let bodies = [
            "GET /index.html 200",
            "session opened for user root",
            "blk_-1608999 replicated",
        ];
        let f = GroupFilter::build(bodies);
        for body in bodies {
            for i in 0..body.len() {
                for j in i + 1..=body.len() {
                    assert!(f.may_contain(&body.as_bytes()[i..j]), "{:?}", &body[i..j]);
                }
            }
        }
        assert!(
            f.may_contain(b"ab"),
            "a needle under three bytes may match anywhere"
        );
    }

    #[test]
    fn absent_needles_are_mostly_rejected() {
        let bodies: Vec<String> = (0..2000)
            .map(|i| format!("worker {i} finished task {} in {} ms", i * 7, i % 97))
            .collect();
        let f = GroupFilter::build(bodies.iter().map(String::as_str));
        let absent = [
            "zq9",
            "Exception",
            "no such token anywhere",
            "PacketResponder",
        ];
        assert!(absent.iter().all(|n| !f.may_contain(n.as_bytes())));
    }

    #[test]
    fn the_encoding_round_trips_and_refuses_other_forms() {
        let groups = vec![
            GroupFilter::build(["alpha beta"]),
            GroupFilter::build(["gamma delta epsilon"; 3]),
        ];
        let bytes = encode(&groups);
        assert_eq!(decode(&bytes).unwrap(), groups);
        let mut wrong_magic = bytes.clone();
        wrong_magic[0] = b'X';
        assert!(decode(&wrong_magic).is_err());
        assert!(decode(&bytes[..bytes.len() - 1]).is_err());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(decode(&longer).is_err());
        let mut bad_size = bytes.clone();
        bad_size[8..12].copy_from_slice(&(3000_u32).to_le_bytes());
        assert!(decode(&bad_size).is_err());
        assert!(decode(b"FTF1").is_err());
    }
}
