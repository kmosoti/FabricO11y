//! Executable converged telemetry bounds, monotonicity, and self-preservation
//! predicates verified by Verus and extracted via Charon/Aeneas.
use vstd::prelude::*;

verus! {
/// Advances a durable watermark strictly monotonically after an atomic flush.
pub fn watermark_advance(
    committed: u64,
    candidate: u64,
    flush_succeeded: bool,
) -> (next: u64)
    ensures
        committed <= next,
        !flush_succeeded ==> next == committed,
        (flush_succeeded && candidate > committed) ==> next == candidate,
        (flush_succeeded && candidate <= committed) ==> next == committed,
{
    if flush_succeeded && candidate > committed {
        candidate
    } else {
        committed
    }
}

/// Advances a projection frontier through one contiguous, published source range.
/// The caller owns origin authentication and establishes that the whole range was
/// published. A rejected range leaves the last durable prefix intact.
pub fn projection_frontier_advance(
    committed: u64,
    first: u64,
    last: u64,
    origin_matches: bool,
    flush_succeeded: bool,
) -> (next: u64)
    ensures
        committed <= next,
        (origin_matches && flush_succeeded && first as int == committed as int + 1 && first <= last)
            ==> next == last,
        !(origin_matches && flush_succeeded && first as int == committed as int + 1 && first <= last)
            ==> next == committed,
{
    if origin_matches && flush_succeeded && first > committed && last >= first {
        if first - committed == 1 {
            last
        } else {
            committed
        }
    } else {
        committed
    }
}

/// Permits accepting another unflushed metric sample while preserving at least
/// 25% (minimum 512 B) headroom below the soft half-ring cap (minimum 2048 B).
pub fn ring_can_buffer(unflushed_bytes: u64, budget_bytes: u64) -> (ok: bool)
    ensures
        ({
            let half = budget_bytes / 2;
            let unflushed_cap = if half > 2048 { half } else { 2048 };
            let quarter = unflushed_cap / 4;
            let headroom = if quarter > 512 { quarter } else { 512 };
            let limit = unflushed_cap - headroom;
            ok == (unflushed_bytes < limit)
        }),
        ok ==> unflushed_bytes < (if budget_bytes / 2 > 2048 { budget_bytes / 2 } else { 2048 }),
{
    let half = budget_bytes / 2;
    let unflushed_cap = if half > 2048 { half } else { 2048 };
    let quarter = unflushed_cap / 4;
    let headroom = if quarter > 512 { quarter } else { 512 };
    let limit = unflushed_cap - headroom;
    unflushed_bytes < limit
}

/// Triggers cold Parquet partition eviction strictly when disk usage exceeds cap.
pub fn disk_needs_eviction(total_bytes: u64, max_disk_bytes: u64) -> (evict: bool)
    ensures evict == (total_bytes > max_disk_bytes),
{
    total_bytes > max_disk_bytes
}

/// Triggers daemon self-shedding strictly when resident set size exceeds limit.
pub fn rss_shedding_decision(rss_bytes: u64, shed_limit_bytes: u64) -> (shed: bool)
    ensures shed == (rss_bytes > shed_limit_bytes),
{
    rss_bytes > shed_limit_bytes
}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_frontier_matches_independent_wide_range_contract() {
        let ids = [0, 1, 2, 3, u64::MAX - 1, u64::MAX];
        for committed in ids {
            for first in ids {
                for last in ids {
                    for origin_matches in [false, true] {
                        for flush_succeeded in [false, true] {
                            let expected = if origin_matches
                                && flush_succeeded
                                && u128::from(first) == u128::from(committed) + 1
                                && last >= first
                            {
                                last
                            } else {
                                committed
                            };
                            assert_eq!(
                                projection_frontier_advance(
                                    committed,
                                    first,
                                    last,
                                    origin_matches,
                                    flush_succeeded,
                                ),
                                expected,
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn projection_frontier_retains_earlier_commit_after_later_failure() {
        let committed = projection_frontier_advance(5, 6, 8, true, true);
        assert_eq!(committed, 8);
        assert_eq!(
            projection_frontier_advance(committed, 9, 10, true, false),
            8
        );
        assert_eq!(
            projection_frontier_advance(committed, 11, 12, true, true),
            8
        );
        assert_eq!(
            projection_frontier_advance(committed, 9, 10, false, true),
            8
        );
        assert_eq!(
            projection_frontier_advance(committed, 9, 10, true, true),
            10
        );
    }

    #[test]
    fn telemetry_predicates_match_wider_arithmetic_at_boundaries() {
        let values = [
            0u64,
            1,
            512,
            1535,
            1536,
            1537,
            2047,
            2048,
            4096,
            32 * 1024 * 1024,
            u64::MAX - 1,
            u64::MAX,
        ];
        for committed in values {
            for candidate in values {
                assert_eq!(watermark_advance(committed, candidate, false), committed);
                assert_eq!(
                    watermark_advance(committed, candidate, true),
                    committed.max(candidate)
                );
                assert_eq!(
                    disk_needs_eviction(committed, candidate),
                    committed > candidate
                );
                assert_eq!(
                    rss_shedding_decision(committed, candidate),
                    committed > candidate
                );
            }
        }
        for budget in values {
            let half = (budget as u128) / 2;
            let cap = half.max(2048);
            let headroom = (cap / 4).max(512);
            let limit = cap - headroom;
            for unflushed in values {
                assert_eq!(
                    ring_can_buffer(unflushed, budget),
                    (unflushed as u128) < limit
                );
            }
        }
    }
}
