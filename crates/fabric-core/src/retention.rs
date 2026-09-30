//! Retention eligibility of the [retained-history contract]: which sealed
//! Segments, oldest first, are deleted when the age or byte limit is
//! exceeded. The decision is data; renaming and removing directories stay in
//! the server's sealer.
//!
//! [retained-history contract]: ../../../docs/architecture/retained-history.md

/// Retention limits: keep Segments whose newest record is at most `max_age_s`
/// old, and at most `max_bytes` of Segments in total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    pub max_age_s: u64,
    pub max_bytes: u64,
}

/// What retention needs to know about one sealed Segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentFacts {
    /// Newest server receive time of any record in the Segment.
    pub received_max_ns: u64,
    pub bytes: u64,
}

/// Receive times below this are older than the age limit. Saturates, so an
/// age limit longer than the epoch keeps everything.
pub const fn age_cutoff_ns(now_ns: u64, max_age_s: u64) -> u64 {
    now_ns.saturating_sub(max_age_s.saturating_mul(1_000_000_000))
}

/// How many of `segments` (oldest first) to delete: while the oldest remaining
/// Segment's newest record is older than the cutoff, or the remaining total
/// exceeds the byte limit, delete it. Whole Segments only; never reorders.
pub fn segments_to_delete(segments: &[SegmentFacts], retention: Retention, now_ns: u64) -> usize {
    let cutoff = age_cutoff_ns(now_ns, retention.max_age_s);
    // Exact, not saturating: once a saturated total is reduced it undercounts
    // what remains. A `u128` sum of `u64` sizes cannot overflow.
    let mut total: u128 = segments.iter().map(|s| u128::from(s.bytes)).sum();
    let max_bytes = u128::from(retention.max_bytes);
    let mut deleted: usize = 0;
    for segment in segments {
        if segment.received_max_ns >= cutoff && total <= max_bytes {
            break;
        }
        // `total` includes this Segment, so this never saturates.
        total = total.saturating_sub(u128::from(segment.bytes));
        deleted = deleted.saturating_add(1);
    }
    deleted
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    fn seg(received_max_ns: u64, bytes: u64) -> SegmentFacts {
        SegmentFacts {
            received_max_ns,
            bytes,
        }
    }

    /// Counterexample found by Kani (CX-RETENTION-SATURATED-TOTAL): with a
    /// saturating total, two Segments of `u64::MAX` bytes left the total at 0
    /// after one deletion, so retention stopped while the remaining Segments
    /// still exceeded the byte limit.
    #[test]
    fn totals_beyond_u64_are_counted_exactly() {
        let huge = SegmentFacts {
            received_max_ns: 10,
            bytes: u64::MAX,
        };
        let small = SegmentFacts {
            received_max_ns: 10,
            bytes: 5,
        };
        let limits = Retention {
            max_age_s: u64::MAX,
            max_bytes: 5,
        };
        assert_eq!(segments_to_delete(&[huge, huge, small], limits, 10), 2);
    }

    #[test]
    fn old_segments_are_deleted_oldest_first_and_stop_at_the_first_young_one() {
        let limits = Retention {
            max_age_s: 10,
            max_bytes: u64::MAX,
        };
        let segments = [seg(80 * S, 1), seg(95 * S, 1), seg(85 * S, 1)];
        // Cutoff 90 s: the first is old, the second is young, so deletion
        // stops there even though the third is old again.
        assert_eq!(segments_to_delete(&segments, limits, 100 * S), 1);
    }

    #[test]
    fn the_byte_limit_deletes_until_the_total_fits() {
        let limits = Retention {
            max_age_s: u64::MAX,
            max_bytes: 5,
        };
        let segments = [seg(1, 4), seg(2, 3), seg(3, 2)];
        // 9 bytes exceed 5; deleting the oldest leaves 5, which fits.
        assert_eq!(segments_to_delete(&segments, limits, 100 * S), 1);
        let tighter = Retention {
            max_age_s: u64::MAX,
            max_bytes: 4,
        };
        assert_eq!(segments_to_delete(&segments, tighter, 100 * S), 2);
        let fits = Retention {
            max_age_s: u64::MAX,
            max_bytes: 9,
        };
        assert_eq!(segments_to_delete(&segments, fits, 100 * S), 0);
    }

    #[test]
    fn saturating_age_keeps_everything_and_empty_input_deletes_nothing() {
        assert_eq!(age_cutoff_ns(5, u64::MAX), 0);
        let keep = Retention {
            max_age_s: u64::MAX,
            max_bytes: u64::MAX,
        };
        assert_eq!(segments_to_delete(&[seg(0, u64::MAX)], keep, 0), 0);
        // Two such Segments exceed even a `u64::MAX` byte limit; this used to
        // expect 0 because the saturated total equalled the limit.
        assert_eq!(
            segments_to_delete(&[seg(0, u64::MAX), seg(0, u64::MAX)], keep, 0),
            1
        );
        assert_eq!(
            segments_to_delete(
                &[],
                Retention {
                    max_age_s: 0,
                    max_bytes: 0
                },
                100
            ),
            0
        );
    }
}
