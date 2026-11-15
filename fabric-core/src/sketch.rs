//! Bounded `DDSketch` quantile estimator for latency and size distributions.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Logarithmic-bucket `DDSketch` with deterministic maximum bin count.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DDSketch {
    alpha: f64,
    gamma: f64,
    log_gamma: f64,
    max_bins: usize,
    zero_count: u64,
    negative_count: u64,
    count: u64,
    min: f64,
    max: f64,
    sum: f64,
    bins: BTreeMap<i32, u64>,
}

impl Default for DDSketch {
    fn default() -> Self {
        Self::new(0.01, 2048)
    }
}

impl DDSketch {
    /// Creates a bounded `DDSketch` with relative accuracy `alpha` in `(0.0, 1.0)`.
    #[must_use]
    pub fn new(alpha: f64, max_bins: usize) -> Self {
        let clamped_alpha = alpha.clamp(0.001, 0.5);
        let gamma = (1.0 + clamped_alpha) / (1.0 - clamped_alpha);
        Self {
            alpha: clamped_alpha,
            gamma,
            log_gamma: gamma.ln(),
            max_bins: max_bins.max(8),
            zero_count: 0,
            negative_count: 0,
            count: 0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            sum: 0.0,
            bins: BTreeMap::new(),
        }
    }

    /// Inserts a finite numeric value into the sketch. Non-finite inputs are ignored.
    pub fn insert(&mut self, value: f64) {
        if !value.is_finite() {
            return;
        }
        self.count = self.count.saturating_add(1);
        self.sum += value;
        if value < self.min {
            self.min = value;
        }
        if value > self.max {
            self.max = value;
        }
        if value <= 0.0 {
            if value < 0.0 {
                self.negative_count = self.negative_count.saturating_add(1);
            } else {
                self.zero_count = self.zero_count.saturating_add(1);
            }
            return;
        }
        let idx = self.bucket_index(value);
        *self.bins.entry(idx).or_insert(0) += 1;
        self.enforce_bin_limit();
    }

    /// Merges another sketch with matching parameters into `self`.
    pub fn merge(&mut self, other: &Self) {
        if other.count == 0 {
            return;
        }
        if self.count == 0 {
            self.min = other.min;
            self.max = other.max;
        } else {
            self.min = self.min.min(other.min);
            self.max = self.max.max(other.max);
        }
        self.count = self.count.saturating_add(other.count);
        self.zero_count = self.zero_count.saturating_add(other.zero_count);
        self.negative_count = self.negative_count.saturating_add(other.negative_count);
        self.sum += other.sum;
        for (&idx, &freq) in &other.bins {
            let bucket = if (self.alpha - other.alpha).abs() < 1e-9 {
                idx
            } else {
                self.bucket_index(other.bucket_value(idx))
            };
            *self.bins.entry(bucket).or_insert(0) += freq;
        }
        self.enforce_bin_limit();
    }

    /// Estimates quantile `q` in `[0.0, 1.0]`. Returns `None` when empty.
    #[must_use]
    pub fn quantile(&self, q: f64) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        let q = q.clamp(0.0, 1.0);
        if q == 0.0 {
            return Some(self.min);
        }
        if q == 1.0 {
            return Some(self.max);
        }
        let rank = ((q * (self.count as f64)).ceil() as u64).max(1);
        let non_positive = self.negative_count.saturating_add(self.zero_count);
        if rank <= non_positive {
            return Some(if rank <= self.negative_count {
                self.min
            } else {
                0.0
            });
        }
        let mut cumulative = non_positive;
        for (&idx, &freq) in &self.bins {
            cumulative = cumulative.saturating_add(freq);
            if cumulative >= rank {
                return Some(self.bucket_value(idx).clamp(self.min.max(0.0), self.max));
            }
        }
        Some(self.max)
    }

    #[must_use]
    pub const fn count(&self) -> u64 {
        self.count
    }

    #[must_use]
    pub fn bin_len(&self) -> usize {
        self.bins.len()
    }

    #[must_use]
    pub const fn max_bins(&self) -> usize {
        self.max_bins
    }

    #[must_use]
    pub const fn alpha(&self) -> f64 {
        self.alpha
    }

    #[must_use]
    pub fn min(&self) -> Option<f64> {
        (self.count > 0).then_some(self.min)
    }

    #[must_use]
    pub fn max(&self) -> Option<f64> {
        (self.count > 0).then_some(self.max)
    }

    #[must_use]
    pub fn mean(&self) -> Option<f64> {
        (self.count > 0).then_some(self.sum / (self.count as f64))
    }

    fn bucket_index(&self, value: f64) -> i32 {
        let clamped = value.max(1e-12);
        (clamped.ln() / self.log_gamma).ceil() as i32
    }

    fn bucket_value(&self, idx: i32) -> f64 {
        2.0 * self.gamma.powi(idx) / (1.0 + self.gamma)
    }

    fn enforce_bin_limit(&mut self) {
        while self.bins.len() > self.max_bins {
            let Some((&lowest_k, &lowest_v)) = self.bins.iter().next() else {
                break;
            };
            let _ = self.bins.remove(&lowest_k);
            if let Some((&next_k, _)) = self.bins.iter().next() {
                *self.bins.entry(next_k).or_insert(0) += lowest_v;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn quantile_accuracy_and_bin_bound() {
        let mut sketch = DDSketch::new(0.01, 2048);
        for v in 1..=1000 {
            sketch.insert(v as f64);
        }
        let p50 = sketch.quantile(0.50).unwrap();
        let p99 = sketch.quantile(0.99).unwrap();
        assert!((p50 - 500.0).abs() <= 10.0, "p50={p50}");
        assert!((p99 - 990.0).abs() <= 15.0, "p99={p99}");
        assert!(sketch.bin_len() <= sketch.max_bins());
    }

    proptest! {
        #[test]
        fn proptest_ddsketch_preserves_order_and_bin_cap(vals in proptest::collection::vec(1.0_f64..100_000.0, 20..500)) {
            let mut sketch = DDSketch::new(0.02, 64);
            for v in vals {
                sketch.insert(v);
            }
            prop_assert!(sketch.bin_len() <= 64);
            let p50 = sketch.quantile(0.50).unwrap();
            let p99 = sketch.quantile(0.99).unwrap();
            let min = sketch.min().unwrap();
            let max = sketch.max().unwrap();
            prop_assert!(min <= p50 && p50 <= p99 && p99 <= max);
        }
    }
}
