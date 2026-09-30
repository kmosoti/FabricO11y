//! Behavior preservation for the semantic kernels extracted in the
//! semantic-kernels milestone.
//!
//! Each `legacy_*` function transcribes, verbatim apart from its inputs and
//! outputs, the decision as it was implemented at `main` 6521e06 before the
//! extraction:
//! - control: `Control::change` / `set_status` / `set_config` in
//!   crates/fabric-server/src/control.rs;
//! - rate: the pair rule in `rates` in crates/fabric-server/src/query.rs;
//! - retention: the deletion loop in `sealer::pass`;
//! - counter start: the `start` computation in `metric_request` and
//!   `decreased` in src/spindle/runtime.rs;
//! - cursor: `cursor_still_valid` in src/spindle/log_source.rs, with the
//!   checksum comparison given as a boolean;
//! - gap text: `bounded_gap` in src/spindle/runtime.rs.
//!
//! These are frozen references, not code to fix. Each test compares the
//! kernel with its reference over an exhaustive small domain.

use fabric_core::collection::{
    CounterValue, CursorCheck, CursorFacts, FileFacts, InvalidCursor, bounded_text, check_cursor,
    counter_start,
};
use fabric_core::control::{ControlRejection, Status, next_revision, set_status};
use fabric_core::query::{CounterPoint, CounterStep, counter_step};
use fabric_core::retention::{Retention, SegmentFacts, segments_to_delete};

// ---- control ----------------------------------------------------------------

/// `change` with the `set_status` edit: refuse a revoked record, otherwise
/// set the status and bump the revision.
fn legacy_set_status(status: Status, target: Status, revision: u64) -> Result<(Status, u64), ()> {
    if status == Status::Revoked {
        return Err(());
    }
    Ok((target, revision + 1))
}

#[test]
fn control_transitions_match_the_base_for_every_status_pair() {
    let all = [Status::Active, Status::Paused, Status::Revoked];
    for status in all {
        for target in all {
            for revision in [1, 2, 41, u64::MAX - 1] {
                let new = set_status(status, target)
                    .and_then(|s| next_revision(revision).map(|r| (s, r)))
                    .map_err(|_| ());
                assert_eq!(
                    new,
                    legacy_set_status(status, target, revision),
                    "{status:?} -> {target:?} at {revision}"
                );
            }
        }
    }
    // Where the base would overflow, the kernel refuses instead.
    assert_eq!(
        next_revision(u64::MAX),
        Err(ControlRejection::RevisionExhausted)
    );
}

// ---- rate -------------------------------------------------------------------

/// The `rates` pair rule: `Some(rate)`, or `None` for a reset marker.
fn legacy_rate(a: (u64, u64, f64), b: (u64, u64, f64)) -> Option<f64> {
    let (a_start, a_time, a_value) = a;
    let (b_start, b_time, b_value) = b;
    if a_start == b_start && b_value >= a_value && b_time > a_time {
        Some((b_value - a_value) / ((b_time - a_time) as f64 / 1e9))
    } else {
        None
    }
}

#[test]
fn counter_steps_match_the_base_bit_for_bit() {
    let starts = [1, 2];
    let times = [0, 1, 999_999_999, 1_000_000_000, 7_000_000_003];
    let values = [0.0, 1.0, 2.5, 1e12, f64::MAX];
    let mut compared = 0;
    for &sa in &starts {
        for &ta in &times {
            for &va in &values {
                for &sb in &starts {
                    for &tb in &times {
                        for &vb in &values {
                            let new = counter_step(
                                CounterPoint {
                                    start_ns: sa,
                                    time_ns: ta,
                                    value: va,
                                },
                                CounterPoint {
                                    start_ns: sb,
                                    time_ns: tb,
                                    value: vb,
                                },
                            );
                            let old = legacy_rate((sa, ta, va), (sb, tb, vb));
                            match (new, old) {
                                (CounterStep::Rate(n), Some(o)) => {
                                    assert_eq!(n.to_bits(), o.to_bits())
                                }
                                (CounterStep::Reset, None) => {}
                                // The one deliberate difference from the base
                                // (CX-RATE-NON-FINITE): a rate that is not a
                                // finite number is now a reset.
                                (CounterStep::Reset, Some(o)) if !o.is_finite() => {}
                                other => panic!(
                                    "disagree at {:?}: {other:?}",
                                    ((sa, ta, va), (sb, tb, vb))
                                ),
                            }
                            compared += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(compared, 2500);
}

// ---- retention ----------------------------------------------------------------

/// `sealer::pass` after sealing: `(received_max_ns, bytes)` oldest first.
fn legacy_deleted(segments: &[(u64, u64)], max_age_s: u64, max_bytes: u64, now: u64) -> usize {
    let mut segments = segments.to_vec();
    let mut total: u64 = segments.iter().map(|(_, b)| b).sum();
    let cutoff = now.saturating_sub(max_age_s.saturating_mul(1_000_000_000));
    let mut deleted = 0;
    while let Some((received_max_ns, bytes)) = segments.first().copied() {
        if received_max_ns >= cutoff && total <= max_bytes {
            break;
        }
        total -= bytes;
        segments.remove(0);
        deleted += 1;
    }
    deleted
}

#[test]
fn retention_matches_the_base_on_every_small_history() {
    let s = 1_000_000_000_u64;
    let received = [5 * s, 9 * s, 10 * s, 20 * s];
    let sizes = [0, 1, 3, 7];
    let mut compared = 0;
    for len in 0..=3_usize {
        let combos = (received.len() * sizes.len()).pow(len as u32);
        for code in 0..combos {
            let mut rest = code;
            let mut segments = Vec::new();
            for _ in 0..len {
                let pick = rest % (received.len() * sizes.len());
                rest /= received.len() * sizes.len();
                segments.push((received[pick / sizes.len()], sizes[pick % sizes.len()]));
            }
            for max_age_s in [0, 5, 11, 1_000] {
                for max_bytes in [0, 3, 8, 100] {
                    let facts: Vec<_> = segments
                        .iter()
                        .map(|&(received_max_ns, bytes)| SegmentFacts {
                            received_max_ns,
                            bytes,
                        })
                        .collect();
                    let now = 20 * s;
                    assert_eq!(
                        segments_to_delete(
                            &facts,
                            Retention {
                                max_age_s,
                                max_bytes
                            },
                            now
                        ),
                        legacy_deleted(&segments, max_age_s, max_bytes, now),
                        "{segments:?} age {max_age_s} bytes {max_bytes}"
                    );
                    compared += 1;
                }
            }
        }
    }
    assert_eq!(compared, 69_904);
}

// ---- counter start --------------------------------------------------------------

#[derive(Clone, Copy)]
enum LegacyValue {
    Int(u64),
    Double(f64),
}

fn legacy_decreased(current: &LegacyValue, previous: &LegacyValue) -> bool {
    match (current, previous) {
        (LegacyValue::Int(a), LegacyValue::Int(b)) => a < b,
        (LegacyValue::Double(a), LegacyValue::Double(b)) => a < b,
        _ => true,
    }
}

fn legacy_start(
    history: Option<(LegacyValue, u64)>,
    value: LegacyValue,
    known: Option<u64>,
    now: u64,
) -> u64 {
    match history {
        Some((old, prior_start)) if !legacy_decreased(&value, &old) => prior_start,
        Some(_) => now,
        None => known.unwrap_or(now),
    }
}

fn both(v: LegacyValue) -> (LegacyValue, CounterValue) {
    match v {
        LegacyValue::Int(i) => (v, CounterValue::Int(i)),
        LegacyValue::Double(d) => (v, CounterValue::Double(d)),
    }
}

#[test]
fn counter_start_matches_the_base_for_every_small_case() {
    let values = [
        LegacyValue::Int(0),
        LegacyValue::Int(5),
        LegacyValue::Int(u64::MAX),
        LegacyValue::Double(0.0),
        LegacyValue::Double(5.0),
        LegacyValue::Double(f64::NAN),
    ];
    for &current in &values {
        for previous in values.iter().map(Some).chain([None]) {
            for known in [None, Some(7)] {
                let (lc, nc) = both(current);
                let old = legacy_start(previous.map(|&p| (p, 100)), lc, known, 900);
                let new = counter_start(previous.map(|&p| (both(p).1, 100)), nc, known, 900);
                assert_eq!(new, old);
            }
        }
    }
}

// ---- log cursor -------------------------------------------------------------------

/// `cursor_still_valid`; `prefix_matches` stands for the CRC comparison.
fn legacy_cursor(
    old: (u64, u64, u64, u32),
    file: (u64, u64, u64),
    prefix_matches: bool,
) -> Result<bool, ()> {
    let (device, inode, offset, prefix_len) = old;
    let (dev, ino, len) = file;
    if device != dev || inode != ino || len < offset {
        return Ok(false);
    }
    if offset > 0 && prefix_len == 0 {
        return Ok(false);
    }
    if prefix_len == 0 {
        return Ok(true);
    }
    if prefix_len as usize > 64 || u64::from(prefix_len) > offset {
        return Err(());
    }
    Ok(prefix_matches)
}

#[test]
fn cursor_checks_match_the_base_for_every_small_case() {
    let mut compared = 0;
    for device in [1, 2] {
        for inode in [1, 2] {
            for offset in [0, 1, 8, 64, 65, 100] {
                for prefix_len in [0, 1, 8, 64, 65] {
                    for len in [0, 8, 64, 100] {
                        for prefix_matches in [false, true] {
                            let old = legacy_cursor(
                                (device, inode, offset, prefix_len),
                                (1, 1, len),
                                prefix_matches,
                            );
                            let new = match check_cursor(
                                CursorFacts {
                                    device,
                                    inode,
                                    offset,
                                    prefix_len,
                                },
                                FileFacts {
                                    device: 1,
                                    inode: 1,
                                    len,
                                },
                            ) {
                                Ok(CursorCheck::Restart) => Ok(false),
                                Ok(CursorCheck::Continue) => Ok(true),
                                Ok(CursorCheck::VerifyPrefix { len }) => {
                                    assert_eq!(len, prefix_len);
                                    Ok(prefix_matches)
                                }
                                Err(InvalidCursor) => Err(()),
                            };
                            assert_eq!(new, old);
                            compared += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(compared, 960);
}

// ---- gap text ---------------------------------------------------------------------

fn legacy_bounded(message: &str, cap: usize) -> String {
    let mut end = message.len().min(cap);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_owned()
}

#[test]
fn bounded_text_matches_the_base_on_multibyte_text() {
    for message in [
        "",
        "abc",
        "é",
        "aé",
        "日本語のパス",
        "a😀b😀c",
        "/var/log/日本/x",
    ] {
        for cap in 0..=20 {
            assert_eq!(
                bounded_text(message, cap),
                legacy_bounded(message, cap),
                "{message:?} cap {cap}"
            );
        }
    }
}
