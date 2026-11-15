//! Bounded `<= 32 MiB` RAM ring buffer using Gorilla delta-of-delta timestamps
//! and IEEE-754 XOR bit encoding (implementation in Sans-I/O `fabric_core::ring`).

pub use fabric_core::ring::{
    DEFAULT_RING_BUDGET_BYTES, HotMetricRing, RingStats, roundtrip_series_block,
};
