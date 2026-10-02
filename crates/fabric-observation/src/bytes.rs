//! Level 0: bytes.
//!
//! A bounded reader over a byte slice and the fixed-width little-endian
//! primitives every level above is built from. The contract: every read is
//! total (it returns the bytes or an error naming the offset, never panics,
//! never reads past the slice), every error leaves the cursor at the byte
//! where the problem starts, and a write is the exact inverse of its read.
//! Nothing here knows what the bytes mean.

use core::fmt;

/// Why bytes could not be read. `offset` is where the decoder stopped: the
/// first byte of the field it could not accept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError {
    pub offset: usize,
    pub reason: &'static str,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.reason, self.offset)
    }
}

impl std::error::Error for DecodeError {}

/// A bounded reader: a slice and a position that never passes its end.
#[derive(Clone, Copy, Debug)]
pub struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// Position of the next unread byte.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Bytes not yet read.
    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    /// An error at the current position.
    pub fn fail<T>(&self, reason: &'static str) -> Result<T, DecodeError> {
        Err(DecodeError {
            offset: self.pos,
            reason,
        })
    }

    /// An error at an earlier position: the start of the field being read.
    pub fn fail_at<T>(&mut self, at: usize, reason: &'static str) -> Result<T, DecodeError> {
        self.pos = at;
        self.fail(reason)
    }

    pub fn byte(&mut self) -> Result<u8, DecodeError> {
        let Some(b) = self.bytes.get(self.pos) else {
            return self.fail("truncated");
        };
        self.pos = self.pos.saturating_add(1);
        Ok(*b)
    }

    /// Exactly `n` bytes, or `truncated` with the cursor unmoved.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.saturating_add(n);
        let Some(slice) = self.bytes.get(self.pos..end) else {
            return self.fail("truncated");
        };
        self.pos = end;
        Ok(slice)
    }

    /// A fixed-width array.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut out = [0_u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    pub fn u32_le(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn u64_le(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
}

/// The write side: plain appends, each the inverse of a `Cursor` read.
pub fn put_u32_le(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn put_u64_le(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_are_exact_and_errors_name_the_offset() {
        let data = [1_u8, 2, 3, 4, 5];
        let mut cur = Cursor::new(&data);
        assert_eq!(cur.byte().unwrap(), 1);
        assert_eq!(cur.take(2).unwrap(), &[2, 3]);
        assert_eq!(cur.remaining(), 2);
        // Too long: the cursor does not move.
        assert_eq!(
            cur.take(3),
            Err(DecodeError {
                offset: 3,
                reason: "truncated"
            })
        );
        assert_eq!(cur.position(), 3);
        assert_eq!(cur.array::<2>().unwrap(), [4, 5]);
        assert_eq!(cur.byte().unwrap_err().offset, 5);
    }

    #[test]
    fn fixed_width_round_trips() {
        let mut out = Vec::new();
        put_u32_le(&mut out, 0xDEAD_BEEF);
        put_u64_le(&mut out, u64::MAX - 1);
        let mut cur = Cursor::new(&out);
        assert_eq!(cur.u32_le().unwrap(), 0xDEAD_BEEF);
        assert_eq!(cur.u64_le().unwrap(), u64::MAX - 1);
        assert_eq!(cur.remaining(), 0);
    }

    #[test]
    fn fail_at_rewinds_to_the_field_start() {
        let data = [9_u8; 4];
        let mut cur = Cursor::new(&data);
        cur.take(3).unwrap();
        let err: Result<(), _> = cur.fail_at(1, "bad field");
        assert_eq!(err.unwrap_err().offset, 1);
        assert_eq!(cur.position(), 1);
    }
}
