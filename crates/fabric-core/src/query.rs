//! Query semantics of the [retained-history contract]: which rows a query
//! window admits, how pages continue, when a page's snapshot is gone, whether
//! an answer is complete, and what one step of a cumulative counter means.
//! Reading journal frames and Parquet Segments, and rendering JSON, stay in
//! the server.
//!
//! [retained-history contract]: ../../../docs/architecture/retained-history.md

pub mod spec;

/// Largest `limit` a paginated query accepts.
pub const MAX_LIMIT: u32 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryRejection {
    /// `from_ns` must be below `to_ns`.
    EmptyWindow,
    /// `limit` must be 1 to [`MAX_LIMIT`].
    LimitOutOfRange,
}

/// A half-open time window `[from_ns, to_ns)` over one snapshot's groups
/// `[oldest_group, newest_group]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub from_ns: u64,
    pub to_ns: u64,
}

impl Window {
    pub const fn new(from_ns: u64, to_ns: u64) -> Result<Self, QueryRejection> {
        if from_ns >= to_ns {
            return Err(QueryRejection::EmptyWindow);
        }
        Ok(Self { from_ns, to_ns })
    }

    pub const fn contains(&self, time_ns: u64) -> bool {
        time_ns >= self.from_ns && time_ns < self.to_ns
    }
}

pub const fn check_limit(limit: u32) -> Result<u32, QueryRejection> {
    if limit >= 1 && limit <= MAX_LIMIT {
        Ok(limit)
    } else {
        Err(QueryRejection::LimitOutOfRange)
    }
}

/// The groups a snapshot covers: from the oldest retained to the newest
/// committed when the first page was answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub oldest_group: u64,
    pub newest_group: u64,
}

impl Snapshot {
    pub const fn contains(&self, group: u64) -> bool {
        group >= self.oldest_group && group <= self.newest_group
    }
}

/// A later page is answered only while every group of its snapshot is still
/// retained. `oldest_retained` is the oldest group now retained.
pub const fn page_snapshot_retained(page_floor: u64, oldest_retained: u64) -> bool {
    oldest_retained <= page_floor
}

/// The total row order: time, then Spindle identity, Strand sequence and the
/// row's index within its Batch. Pages continue strictly after a key.
pub type RowKey = (u64, [u8; 16], u64, u32);

pub fn after_page(key: &RowKey, after: Option<&RowKey>) -> bool {
    after.is_none_or(|a| key > a)
}

/// An answer is complete only if no Segment or journal file that could hold
/// matching rows was unavailable.
pub const fn complete(unavailable: usize) -> bool {
    unavailable == 0
}

/// One point of a monotonic cumulative counter series.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CounterPoint {
    pub start_ns: u64,
    pub time_ns: u64,
    pub value: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CounterStep {
    /// Units per second between the two points.
    Rate(f64),
    /// The counter restarted (new start time or a decrease), or time did not
    /// advance: no rate is fabricated for the interval.
    Reset,
}

/// The meaning of two consecutive points of one series.
pub fn counter_step(previous: CounterPoint, next: CounterPoint) -> CounterStep {
    if previous.start_ns == next.start_ns
        && next.value >= previous.value
        && next.time_ns > previous.time_ns
        && previous.value.is_finite()
        && next.value.is_finite()
    {
        let seconds = next.time_ns.saturating_sub(previous.time_ns) as f64 / 1e9;
        let rate = (next.value - previous.value) / seconds;
        // Non-finite values (or an overflowing difference) give no number a
        // rate row could carry; no rate is fabricated for the interval.
        if rate.is_finite() {
            return CounterStep::Rate(rate);
        }
    }
    CounterStep::Reset
}

/// Counter values retain their OTLP type until the delta has been formed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CounterNumber {
    Int(i64),
    Double(f64),
}

/// A typed counter point; integer precision is part of the input contract.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CounterSample {
    pub start_ns: u64,
    pub time_ns: u64,
    pub value: CounterNumber,
}

// Compare an integer with a double without rounding the integer to 53 bits.
// Inside the i64 range, truncation is exact; the fractional part breaks ties.
fn int_double_cmp(integer: i64, double: f64) -> Option<core::cmp::Ordering> {
    use core::cmp::Ordering;
    if !double.is_finite() {
        return None;
    }
    if double >= 9_223_372_036_854_775_808.0 {
        return Some(Ordering::Less);
    }
    if double < -9_223_372_036_854_775_808.0 {
        return Some(Ordering::Greater);
    }
    let truncated = double as i64;
    match integer.cmp(&truncated) {
        Ordering::Equal => (truncated as f64).partial_cmp(&double),
        order => Some(order),
    }
}

/// Typed refinement of `counter_step`: integer deltas are exact in i128,
/// then rounded once for the rate. Mixed deltas use floating subtraction,
/// but decrease detection compares the represented values without rounding.
pub fn counter_step_numbers(previous: CounterSample, next: CounterSample) -> CounterStep {
    use CounterNumber::{Double, Int};
    use core::cmp::Ordering;
    let order = match (previous.value, next.value) {
        (Int(a), Int(b)) => Some(a.cmp(&b)),
        (Int(a), Double(b)) => int_double_cmp(a, b),
        (Double(a), Int(b)) => int_double_cmp(b, a).map(Ordering::reverse),
        (Double(a), Double(b)) if a.is_finite() && b.is_finite() => a.partial_cmp(&b),
        _ => None,
    };
    // Retain the original temporal/reset kernel as the deciding boundary.
    // A represented decrease is encoded as a decreasing normalized pair even
    // when floating subtraction would round its delta to zero.
    let normalized = |a, b| {
        counter_step(
            CounterPoint {
                start_ns: previous.start_ns,
                time_ns: previous.time_ns,
                value: a,
            },
            CounterPoint {
                start_ns: next.start_ns,
                time_ns: next.time_ns,
                value: b,
            },
        )
    };
    match order {
        Some(Ordering::Greater) => return normalized(1.0, 0.0),
        None => return CounterStep::Reset,
        _ => {}
    }
    let delta = match (previous.value, next.value) {
        (Int(a), Int(b)) => i128::from(b).checked_sub(i128::from(a)).map(|d| d as f64),
        (Int(a), Double(b)) => Some(b - a as f64),
        (Double(a), Int(b)) => Some(b as f64 - a),
        (Double(a), Double(b)) => Some(b - a),
    };
    if let Some(delta) = delta {
        return normalized(0.0, delta);
    }
    CounterStep::Reset
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_are_half_open_and_never_empty() {
        let w = Window::new(10, 20).unwrap();
        assert!(w.contains(10) && w.contains(19) && !w.contains(20) && !w.contains(9));
        assert_eq!(Window::new(20, 20), Err(QueryRejection::EmptyWindow));
        assert_eq!(Window::new(21, 20), Err(QueryRejection::EmptyWindow));
    }

    #[test]
    fn limits_are_one_to_ten_thousand() {
        assert_eq!(check_limit(0), Err(QueryRejection::LimitOutOfRange));
        assert_eq!(check_limit(1), Ok(1));
        assert_eq!(check_limit(MAX_LIMIT), Ok(MAX_LIMIT));
        assert_eq!(
            check_limit(MAX_LIMIT + 1),
            Err(QueryRejection::LimitOutOfRange)
        );
    }

    #[test]
    fn a_page_is_gone_once_retention_passes_its_floor() {
        assert!(page_snapshot_retained(5, 5));
        assert!(page_snapshot_retained(5, 3));
        assert!(!page_snapshot_retained(5, 6));
        let s = Snapshot {
            oldest_group: 3,
            newest_group: 7,
        };
        assert!(s.contains(3) && s.contains(7) && !s.contains(2) && !s.contains(8));
    }

    #[test]
    fn pages_continue_strictly_after_the_last_key() {
        let k = (5, [1; 16], 2, 0);
        assert!(after_page(&k, None));
        assert!(!after_page(&k, Some(&k)));
        assert!(after_page(&(5, [1; 16], 2, 1), Some(&k)));
        assert!(!after_page(&(4, [9; 16], 9, 9), Some(&k)));
        assert!(complete(0) && !complete(1));
    }

    #[test]
    fn counter_steps_rate_only_within_one_unbroken_run() {
        let p = |start_ns, time_ns, value| CounterPoint {
            start_ns,
            time_ns,
            value,
        };
        assert_eq!(
            counter_step(p(1, 1_000_000_000, 10.0), p(1, 3_000_000_000, 30.0)),
            CounterStep::Rate(10.0)
        );
        assert_eq!(
            counter_step(p(1, 1, 10.0), p(1, 2, 10.0)),
            CounterStep::Rate(0.0)
        );
        assert_eq!(
            counter_step(p(1, 1, 10.0), p(2, 2, 30.0)),
            CounterStep::Reset
        );
        assert_eq!(
            counter_step(p(1, 1, 10.0), p(1, 2, 9.0)),
            CounterStep::Reset
        );
        assert_eq!(
            counter_step(p(1, 2, 10.0), p(1, 2, 20.0)),
            CounterStep::Reset
        );
    }

    /// Counterexample found by Kani (CX-RATE-NON-FINITE): two `+inf` counter
    /// values gave `inf - inf = NaN` as a rate on a row that is not a reset,
    /// which no answer can carry (a non-reset row needs a numeric rate).
    #[test]
    fn non_finite_rates_are_resets() {
        let p = |time_ns, value| CounterPoint {
            start_ns: 1,
            time_ns,
            value,
        };
        assert_eq!(
            counter_step(p(1, f64::INFINITY), p(2, f64::INFINITY)),
            CounterStep::Reset
        );
        assert_eq!(
            counter_step(p(1, -f64::MAX), p(2, f64::MAX)),
            CounterStep::Reset
        );
        assert_eq!(counter_step(p(1, 1.0), p(2, f64::NAN)), CounterStep::Reset);
    }
}

#[cfg(test)]
mod typed_counter_tests {
    use super::*;
    use CounterNumber::{Double, Int};

    fn step(a: CounterNumber, b: CounterNumber) -> CounterStep {
        counter_step_numbers(
            CounterSample {
                start_ns: 1,
                time_ns: 1_000_000_000,
                value: a,
            },
            CounterSample {
                start_ns: 1,
                time_ns: 2_000_000_000,
                value: b,
            },
        )
    }

    #[test]
    fn integer_deltas_and_mixed_comparisons_keep_precision() {
        assert_eq!(
            step(Int(1 << 53), Int((1 << 53) + 1)),
            CounterStep::Rate(1.0)
        );
        assert_eq!(step(Int((1 << 53) + 1), Int(1 << 53)), CounterStep::Reset);
        assert_eq!(
            step(Int(i64::MIN), Int(i64::MAX)),
            CounterStep::Rate(u64::MAX as f64)
        );
        assert_eq!(
            step(Int(i64::MAX), Double(9_223_372_036_854_775_808.0)),
            CounterStep::Rate(0.0)
        );
        assert_eq!(
            step(Double(9_223_372_036_854_775_808.0), Int(i64::MAX)),
            CounterStep::Reset
        );
        assert_eq!(
            step(Int((1 << 53) + 1), Double((1u64 << 53) as f64)),
            CounterStep::Reset
        );
        assert_eq!(
            step(Double((1u64 << 53) as f64), Int((1 << 53) + 1)),
            CounterStep::Rate(0.0)
        );
        assert_eq!(step(Int(-2), Double(-1.5)), CounterStep::Rate(0.5));
        assert_eq!(step(Double(-1.5), Int(-2)), CounterStep::Reset);
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(step(Int(0), Double(value)), CounterStep::Reset);
            assert_eq!(step(Double(value), Int(0)), CounterStep::Reset);
        }
    }

    #[test]
    fn double_refinement_preserves_the_legacy_step() {
        let values = [
            -f64::MAX,
            -1.5,
            0.0,
            0.5,
            1.0,
            f64::MAX,
            f64::INFINITY,
            f64::NAN,
        ];
        for a in values {
            for b in values {
                let old = counter_step(
                    CounterPoint {
                        start_ns: 1,
                        time_ns: 1_000_000_000,
                        value: a,
                    },
                    CounterPoint {
                        start_ns: 1,
                        time_ns: 2_000_000_000,
                        value: b,
                    },
                );
                assert_eq!(step(Double(a), Double(b)), old);
            }
        }
    }
}
