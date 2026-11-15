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
