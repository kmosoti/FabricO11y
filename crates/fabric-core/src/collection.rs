//! Collection decisions of the Spindle: when a cumulative counter's start
//! time is kept or reset, whether a committed log cursor still describes a
//! file, and how a gap text is bounded. Reading `/proc`, opening files and
//! computing checksums stay in the Linux adapter.

/// A sampled cumulative counter value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CounterValue {
    Int(u64),
    Double(f64),
}

/// A decrease, or a change of value type, means the counter restarted.
pub fn decreased(current: CounterValue, previous: CounterValue) -> bool {
    match (current, previous) {
        (CounterValue::Int(a), CounterValue::Int(b)) => a < b,
        (CounterValue::Double(a), CounterValue::Double(b)) => a < b,
        _ => true,
    }
}

/// The start time to report for a counter sample. A continuing series keeps
/// its start; a restarted series starts at this sample; a new series uses a
/// start the source knows (such as boot time) or this sample.
pub fn counter_start(
    previous: Option<(CounterValue, u64)>,
    current: CounterValue,
    known_start_ns: Option<u64>,
    now_ns: u64,
) -> u64 {
    match previous {
        Some((old, prior_start)) if !decreased(current, old) => prior_start,
        Some(_) => now_ns,
        None => known_start_ns.unwrap_or(now_ns),
    }
}

/// Longest consumed prefix a cursor may witness.
pub const MAX_PREFIX_BYTES: u32 = 64;

/// What a committed cursor recorded about a log file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorFacts {
    pub device: u64,
    pub inode: u64,
    pub offset: u64,
    pub prefix_len: u32,
}

/// What the open file is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileFacts {
    pub device: u64,
    pub inode: u64,
    pub len: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorCheck {
    /// The cursor does not describe this file: report unknown prior coverage
    /// and read from byte zero.
    Restart,
    /// The cursor continues this file.
    Continue,
    /// Continue only if the first `len` bytes still hash to the witness.
    VerifyPrefix { len: u32 },
}

/// The cursor itself is malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidCursor;

pub fn check_cursor(old: CursorFacts, file: FileFacts) -> Result<CursorCheck, InvalidCursor> {
    if old.device != file.device || old.inode != file.inode || file.len < old.offset {
        return Ok(CursorCheck::Restart);
    }
    if old.offset > 0 && old.prefix_len == 0 {
        // A cursor without a witness cannot silently skip a recreated file.
        return Ok(CursorCheck::Restart);
    }
    if old.prefix_len == 0 {
        return Ok(CursorCheck::Continue);
    }
    if old.prefix_len > MAX_PREFIX_BYTES || u64::from(old.prefix_len) > old.offset {
        return Err(InvalidCursor);
    }
    Ok(CursorCheck::VerifyPrefix {
        len: old.prefix_len,
    })
}

/// The longest prefix of `message` of at most `cap` bytes that ends on a
/// character boundary.
pub fn bounded_text(message: &str, cap: usize) -> &str {
    let mut end = message.len().min(cap);
    while !message.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    message.get(..end).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use CounterValue::*;

    #[test]
    fn counter_start_resets_on_decrease_or_type_change() {
        assert_eq!(counter_start(Some((Int(5), 100)), Int(7), None, 900), 100);
        assert_eq!(counter_start(Some((Int(5), 100)), Int(5), None, 900), 100);
        assert_eq!(counter_start(Some((Int(5), 100)), Int(4), None, 900), 900);
        assert_eq!(
            counter_start(Some((Int(5), 100)), Double(9.0), None, 900),
            900
        );
        assert_eq!(counter_start(None, Int(1), Some(50), 900), 50);
        assert_eq!(counter_start(None, Int(1), None, 900), 900);
    }

    #[test]
    fn cursor_checks_follow_identity_length_and_witness() {
        let file = FileFacts {
            device: 1,
            inode: 2,
            len: 100,
        };
        let c = |offset, prefix_len| CursorFacts {
            device: 1,
            inode: 2,
            offset,
            prefix_len,
        };
        assert_eq!(check_cursor(c(0, 0), file), Ok(CursorCheck::Continue));
        assert_eq!(check_cursor(c(10, 0), file), Ok(CursorCheck::Restart));
        assert_eq!(
            check_cursor(c(10, 8), file),
            Ok(CursorCheck::VerifyPrefix { len: 8 })
        );
        assert_eq!(check_cursor(c(101, 8), file), Ok(CursorCheck::Restart));
        assert_eq!(check_cursor(c(10, 11), file), Err(InvalidCursor));
        assert_eq!(check_cursor(c(100, 65), file), Err(InvalidCursor));
        let moved = FileFacts { inode: 3, ..file };
        assert_eq!(check_cursor(c(10, 8), moved), Ok(CursorCheck::Restart));
    }

    #[test]
    fn bounded_text_never_splits_a_character() {
        assert_eq!(bounded_text("abcdef", 4), "abcd");
        assert_eq!(bounded_text("ab", 4), "ab");
        // "é" is two bytes: a cap of 2 after "a" must not split it.
        assert_eq!(bounded_text("aé", 2), "a");
        assert_eq!(bounded_text("éé", 3), "é");
    }
}
