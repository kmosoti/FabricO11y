//! Level 2: canonical unsigned varints (LEB128, shortest form).
//!
//! Seven value bits per byte, low group first, the high bit set on every
//! byte but the last. The contract that makes the level canonical: the
//! encoder writes the shortest form, and the decoder accepts only the
//! shortest form, so there is exactly one byte string per value. A decoder
//! that tolerated `0x80 0x00` for zero would break every level above it,
//! because a hash of the bytes would no longer be a hash of the value.
//!
//! Rejected: a continuation into a final zero byte (overlong), more than ten
//! bytes, and a tenth byte above 1 (overflow past 64 bits).

use crate::bits::{bit, lo7, shr7};
use crate::bytes::{Cursor, DecodeError};
use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;

/// The longest encoding of a `u64`: ten bytes.
pub const MAX_LEN: usize = 10;

pub fn put(out: &mut Vec<u8>, mut v: u64) {
    while shr7(v) != 0 {
        out.push(lo7(v) | 0x80);
        v = shr7(v);
    }
    out.push(lo7(v));
}

/// Bytes `put` would write for `v`.
pub fn len(v: u64) -> usize {
    let bits = 64_u32.saturating_sub(v.leading_zeros()).max(1);
    bits.div_ceil(7) as usize
}

pub fn get(cur: &mut Cursor<'_>) -> Result<u64, DecodeError> {
    let start = cur.position();
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    loop {
        let b = cur.byte()?;
        let group = u64::from(lo7(u64::from(b)));
        if shift == 63 && group > 1 {
            return cur.fail_at(start, "varint overflows 64 bits");
        }
        value |= group << shift;
        if !bit(b, 7) {
            if b == 0 && cur.position().saturating_sub(start) > 1 {
                return cur.fail_at(start, "overlong varint");
            }
            return Ok(value);
        }
        shift = shift.saturating_add(7);
        if shift > 63 {
            return cur.fail_at(start, "varint longer than ten bytes");
        }
    }
}

/// A count of items that the remaining bytes could hold at `at_least`
/// bytes each: the guard that keeps a hostile count from reserving memory.
pub fn count(cur: &mut Cursor<'_>, at_least: usize) -> Result<usize, DecodeError> {
    let start = cur.position();
    let n = get(cur)?;
    let Ok(n) = usize::try_from(n) else {
        return cur.fail_at(start, "count too large");
    };
    if n.checked_mul(at_least)
        .is_none_or(|need| need > cur.remaining())
    {
        return cur.fail_at(start, "count exceeds the bytes left");
    }
    Ok(n)
}

/// A length-prefixed byte string.
pub fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put(out, b.len() as u64);
    out.extend_from_slice(b);
}

pub fn get_bytes<'a>(cur: &mut Cursor<'a>) -> Result<&'a [u8], DecodeError> {
    let n = count(cur, 1)?;
    cur.take(n)
}

/// A length-prefixed UTF-8 string.
pub fn get_string(cur: &mut Cursor<'_>) -> Result<String, DecodeError> {
    let start = cur.position();
    let raw = get_bytes(cur)?;
    match core::str::from_utf8(raw) {
        Ok(s) => Ok(s.to_owned()),
        Err(_) => cur.fail_at(start, "string is not UTF-8"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        put(&mut out, v);
        assert_eq!(out.len(), len(v));
        let mut cur = Cursor::new(&out);
        assert_eq!(get(&mut cur).unwrap(), v);
        assert_eq!(cur.remaining(), 0);
        out
    }

    #[test]
    fn shortest_form_round_trips_at_every_width_boundary() {
        for v in [
            0,
            1,
            127,
            128,
            16_383,
            16_384,
            1 << 62,
            (1 << 63) - 1,
            1 << 63,
            u64::MAX,
        ] {
            roundtrip(v);
        }
        assert_eq!(roundtrip(300), vec![0xAC, 0x02]);
        assert_eq!(roundtrip(u64::MAX).len(), MAX_LEN);
    }

    #[test]
    fn overlong_forms_are_rejected() {
        for bad in [&[0x80, 0x00][..], &[0xFF, 0x00], &[0xAC, 0x82, 0x00]] {
            let mut cur = Cursor::new(bad);
            assert_eq!(get(&mut cur).unwrap_err().reason, "overlong varint");
            assert_eq!(cur.position(), 0, "cursor rewinds to the field start");
        }
    }

    #[test]
    fn overflow_and_overlength_are_rejected() {
        let mut eleven = vec![0x80_u8; 10];
        eleven.push(0x01);
        let mut cur = Cursor::new(&eleven);
        assert_eq!(
            get(&mut cur).unwrap_err().reason,
            "varint longer than ten bytes"
        );
        let mut overflow = vec![0xFF_u8; 9];
        overflow.push(0x02);
        let mut cur = Cursor::new(&overflow);
        assert_eq!(
            get(&mut cur).unwrap_err().reason,
            "varint overflows 64 bits"
        );
        let mut cur = Cursor::new(&[0x80]);
        assert_eq!(get(&mut cur).unwrap_err().reason, "truncated");
    }

    #[test]
    fn counts_are_bounded_by_the_bytes_left() {
        let mut out = Vec::new();
        put(&mut out, 5);
        out.extend_from_slice(&[0; 4]);
        let mut cur = Cursor::new(&out);
        assert_eq!(
            count(&mut cur, 1).unwrap_err().reason,
            "count exceeds the bytes left"
        );
        out.push(0);
        let mut cur = Cursor::new(&out);
        assert_eq!(count(&mut cur, 1).unwrap(), 5);
        let mut huge = Vec::new();
        put(&mut huge, u64::MAX);
        let mut cur = Cursor::new(&huge);
        assert!(count(&mut cur, 16).is_err());
    }

    #[test]
    fn strings_must_be_utf8() {
        let mut out = Vec::new();
        put_bytes(&mut out, &[0xFF, 0xFE]);
        let mut cur = Cursor::new(&out);
        assert_eq!(
            get_string(&mut cur).unwrap_err().reason,
            "string is not UTF-8"
        );
        let mut out = Vec::new();
        put_bytes(&mut out, "héllo".as_bytes());
        assert_eq!(get_string(&mut Cursor::new(&out)).unwrap(), "héllo");
    }
}
