//! Kani proof harnesses. Compiled only by `cargo kani` (`cfg(kani)`), so
//! they add no dependency and no code to ordinary builds.
//!
//! Each harness states a kernel's contract over symbolic inputs: Kani checks
//! it for every value of the input types, within the unwinding bound noted on
//! the harness, and also checks for panics and arithmetic overflow. Run them
//! with `bash formal/kani/check.sh`. Kani's function-contract attributes are
//! deliberately not used: they are unstable (`-Z function-contracts`).

use crate::collection::{
    CounterValue, CursorCheck, CursorFacts, FileFacts, MAX_PREFIX_BYTES, bounded_text,
    check_cursor, counter_start,
};
use crate::control::{Status, authorizes, next_revision, set_status};
use crate::delivery::{
    BatchDigest, BindingState, CommittedStrand, DeliveryDecision, IncomingBatch, decide_delivery,
};
use crate::query::{CounterPoint, CounterStep, MAX_LIMIT, check_limit, counter_step};
use crate::retention::{Retention, SegmentFacts, age_cutoff_ns, segments_to_delete};
use crate::strand::{SpindleId, StrandId, next_sequence};
use core::num::NonZeroU64;

fn any_status() -> Status {
    match kani::any::<u8>() % 3 {
        0 => Status::Active,
        1 => Status::Paused,
        _ => Status::Revoked,
    }
}

/// Digests from a two-value domain: equality is all the kernel looks at.
fn any_digest() -> BatchDigest {
    BatchDigest([kani::any::<bool>() as u8; 32])
}

/// A Strand never accepts a successor of `u64::MAX`, and otherwise the next
/// sequence is exactly one more.
#[kani::proof]
fn next_sequence_is_one_more_or_none_at_the_end() {
    let last: u64 = kani::any();
    match next_sequence(last) {
        Some(next) => assert!(last < u64::MAX && next == last + 1),
        None => assert!(last == u64::MAX),
    }
}

/// Revisions advance by one and never wrap.
#[kani::proof]
fn revisions_advance_by_one_and_never_wrap() {
    let revision: u64 = kani::any();
    match next_revision(revision) {
        Ok(next) => assert!(revision < u64::MAX && next == revision + 1),
        Err(_) => assert!(revision == u64::MAX),
    }
}

/// CTRL-2: a revoked enrollment accepts no status change and never
/// authenticates; any other enrollment accepts every change.
#[kani::proof]
fn revocation_is_terminal() {
    let current = any_status();
    let target = any_status();
    let result = set_status(current, target);
    if current == Status::Revoked {
        assert!(result.is_err());
        assert!(!authorizes(current));
    } else {
        assert!(result == Ok(target));
    }
}

/// ADR-0013 for every sequence number and committed state: the decision is
/// the one the table names, and it never panics or overflows.
#[kani::proof]
fn delivery_decisions_follow_the_table() {
    let committed = if kani::any() {
        let last: u64 = kani::any();
        kani::assume(last >= 1);
        Some(CommittedStrand {
            last,
            digest: any_digest(),
        })
    } else {
        None
    };
    let sequence: u64 = kani::any();
    kani::assume(sequence >= 1);
    let spindle = SpindleId::new([kani::any::<bool>() as u8; 16]);
    let incoming = IncomingBatch {
        strand: StrandId::new(spindle, 1).unwrap(),
        sequence: NonZeroU64::new(sequence).unwrap(),
        digest: any_digest(),
    };
    let binding = BindingState {
        credential_spindle: if kani::any() {
            Some(SpindleId::new([kani::any::<bool>() as u8; 16]))
        } else {
            None
        },
        spindle_bound_elsewhere: kani::any(),
    };
    let permitted =
        binding.credential_spindle.is_none_or(|s| s == spindle) && !binding.spindle_bound_elsewhere;
    let last = committed.map_or(0, |c| c.last);
    let decision = decide_delivery(committed, &incoming, binding);
    match decision {
        DeliveryDecision::Forbidden => assert!(!permitted),
        DeliveryDecision::Commit { sequence: s } => {
            assert!(permitted && s == sequence && last < u64::MAX && sequence == last + 1)
        }
        DeliveryDecision::Duplicate { committed_through } => assert!(
            permitted
                && committed_through == last
                && sequence == last
                && committed.unwrap().digest == incoming.digest
        ),
        DeliveryDecision::Conflict { committed_through } => assert!(
            permitted
                && committed_through == last
                && sequence == last
                && committed.unwrap().digest != incoming.digest
        ),
        DeliveryDecision::Stale { committed_through } => {
            assert!(permitted && committed_through == last && sequence < last)
        }
        DeliveryDecision::Gap { committed_through } => {
            assert!(
                permitted && committed_through == last && sequence > last && sequence - last >= 2
            )
        }
    }
}

/// SPOOL-1: a cursor continues only a file it can identify, never skips a
/// file without a witness, and asks to verify at most the recorded prefix.
#[kani::proof]
fn cursors_never_skip_an_unidentified_file() {
    let old = CursorFacts {
        device: kani::any(),
        inode: kani::any(),
        offset: kani::any(),
        prefix_len: kani::any(),
    };
    let file = FileFacts {
        device: kani::any(),
        inode: kani::any(),
        len: kani::any(),
    };
    let same_file = old.device == file.device && old.inode == file.inode && file.len >= old.offset;
    match check_cursor(old, file) {
        Ok(CursorCheck::Continue) => assert!(same_file && old.offset == 0),
        Ok(CursorCheck::VerifyPrefix { len }) => assert!(
            same_file
                && len == old.prefix_len
                && len <= MAX_PREFIX_BYTES
                && u64::from(len) <= old.offset
        ),
        Ok(CursorCheck::Restart) => assert!(!same_file || (old.offset > 0 && old.prefix_len == 0)),
        Err(_) => assert!(same_file && old.prefix_len > 0),
    }
}

/// A counter keeps its start exactly while it neither decreases nor changes
/// value type.
#[kani::proof]
fn counter_starts_follow_the_series() {
    let old: u64 = kani::any();
    let new: u64 = kani::any();
    let prior_start: u64 = kani::any();
    let now: u64 = kani::any();
    let type_change: bool = kani::any();
    let current = if type_change {
        CounterValue::Double(0.0)
    } else {
        CounterValue::Int(new)
    };
    let start = counter_start(
        Some((CounterValue::Int(old), prior_start)),
        current,
        None,
        now,
    );
    if !type_change && new >= old {
        assert!(start == prior_start);
    } else {
        assert!(start == now);
    }
}

/// HIST-6: a rate is produced only for an uninterrupted, advancing,
/// non-decreasing series, and it is never negative.
#[kani::proof]
fn rates_come_only_from_uninterrupted_series() {
    let a = CounterPoint {
        start_ns: kani::any(),
        time_ns: kani::any(),
        value: kani::any(),
    };
    let b = CounterPoint {
        start_ns: kani::any(),
        time_ns: kani::any(),
        value: kani::any(),
    };
    if let CounterStep::Rate(rate) = counter_step(a, b) {
        assert!(a.start_ns == b.start_ns && b.time_ns > a.time_ns && b.value >= a.value);
        assert!(rate.is_finite() && rate >= 0.0);
    }
}

/// Retention's age cutoff saturates instead of wrapping.
#[kani::proof]
fn age_cutoff_never_wraps() {
    let now: u64 = kani::any();
    let max_age_s: u64 = kani::any();
    let cutoff = age_cutoff_ns(now, max_age_s);
    assert!(cutoff <= now);
}

/// HIST-5 over every history of up to three Segments: retention deletes the
/// shortest oldest-first prefix that brings the rest within both limits.
#[kani::proof]
#[kani::unwind(5)]
fn retention_deletes_the_shortest_sufficient_prefix() {
    let segments = [
        SegmentFacts {
            received_max_ns: kani::any(),
            bytes: kani::any(),
        },
        SegmentFacts {
            received_max_ns: kani::any(),
            bytes: kani::any(),
        },
        SegmentFacts {
            received_max_ns: kani::any(),
            bytes: kani::any(),
        },
    ];
    let count: usize = kani::any();
    kani::assume(count <= 3);
    let segments = &segments[..count];
    let retention = Retention {
        max_age_s: kani::any(),
        max_bytes: kani::any(),
    };
    let now: u64 = kani::any();
    let cutoff = age_cutoff_ns(now, retention.max_age_s);
    let sufficient = |n: usize| {
        let rest = &segments[n..];
        let mut total: u128 = 0;
        for s in rest {
            total += u128::from(s.bytes);
        }
        rest.first().is_none_or(|s| s.received_max_ns >= cutoff)
            && total <= u128::from(retention.max_bytes)
    };
    let deleted = segments_to_delete(segments, retention, now);
    assert!(deleted <= segments.len());
    assert!(sufficient(deleted));
    assert!(deleted == 0 || !sufficient(deleted - 1));
}

/// Query limits: exactly 1 to `MAX_LIMIT` is accepted.
#[kani::proof]
fn limits_are_accepted_exactly_in_range() {
    let limit: u32 = kani::any();
    assert!(check_limit(limit).is_ok() == (limit >= 1 && limit <= MAX_LIMIT));
}

/// Gap texts over every 4-byte UTF-8 message and cap: the result is a prefix
/// within the cap that ends on a character boundary, and it is the longest.
#[kani::proof]
#[kani::unwind(6)]
fn bounded_text_is_the_longest_valid_prefix() {
    let bytes: [u8; 4] = kani::any();
    let Ok(message) = core::str::from_utf8(&bytes) else {
        return;
    };
    let cap: usize = kani::any();
    kani::assume(cap <= 5);
    let bounded = bounded_text(message, cap);
    assert!(bounded.len() <= cap);
    assert!(message.is_char_boundary(bounded.len()));
    assert!(message.as_bytes().starts_with(bounded.as_bytes()));
    let rest = &message[bounded.len()..];
    if let Some(c) = rest.chars().next() {
        assert!(bounded.len() + c.len_utf8() > cap);
    }
}
