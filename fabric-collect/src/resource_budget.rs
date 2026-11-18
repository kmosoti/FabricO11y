//! Collector budget policy over shared Linux resource observations.

use admission_kernel::telemetry::elastic_spendable;
use fabric::resources::process_memory;
use fabric_core::DEFAULT_RING_BUDGET_BYTES;

/// The configured hot cache budget and the Linux limit used to derive it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingBudget {
    pub budget_bytes: usize,
    pub cgroup_limit_bytes: Option<u64>,
}

fn ring_budget(limit: Option<u64>) -> RingBudget {
    let budget_bytes = limit.map_or(DEFAULT_RING_BUDGET_BYTES, |limit| {
        (elastic_spendable(limit) / 8).clamp(4096, DEFAULT_RING_BUDGET_BYTES as u64) as usize
    });
    RingBudget {
        budget_bytes,
        cgroup_limit_bytes: limit,
    }
}

/// Allocate one eighth of spendable cgroup memory to the optional hot ring.
#[must_use]
pub fn automatic_ring_budget() -> RingBudget {
    ring_budget(process_memory().limit_bytes)
}

pub(crate) fn cgroup_memory_pressure() -> bool {
    process_memory().pressured
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_extreme_limits_have_repeatable_defaults() {
        assert_eq!(ring_budget(None).budget_bytes, DEFAULT_RING_BUDGET_BYTES);
        assert_eq!(
            ring_budget(Some(u64::MAX)).budget_bytes,
            DEFAULT_RING_BUDGET_BYTES
        );
        assert_eq!(ring_budget(Some(1)).budget_bytes, 4096);
        assert_eq!(elastic_spendable(5), 3);
    }
}
