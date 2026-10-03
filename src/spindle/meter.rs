//! The Spindle's metering of its own output: what it commits and delivers, reported
//! as its own OTLP metrics, and an optional cap on its delivery rate.
//!
//! Counters are cumulative from the process start, which is their `start_ns`, so a
//! restart reads as a counter reset under the contract's rate rules. They describe
//! the state before the Batch that carries them.
//!
//! - `fabric.spindle.committed.bytes{signal=logs|metrics|traces}` (By): encoded OTLP
//!   bytes committed to the Spool;
//! - `fabric.spindle.committed.records{signal=logs|metrics|traces}` (1): log lines,
//!   metric points and spans committed;
//! - `fabric.spindle.delivered.batches` (1) and `fabric.spindle.delivered.bytes` (By):
//!   Batches the server acknowledged and their stored bytes;
//! - `fabric.spindle.throttled.seconds` (s): time delivery waited on the rate cap;
//! - gauges `fabric.spindle.spool.bytes` (By), `fabric.spindle.unacked.batches` (1)
//!   and `fabric.spindle.log.backlog.bytes` (By).
//!
//! The cap (`max_output_bytes_per_s`) is a token bucket over delivered Batch bytes
//! with a burst of one second's worth or one full Batch, whichever is larger, so a
//! Batch is never refused by the cap, only delayed.

use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::metrics::v1::{
    AggregationTemporality, Gauge, Metric, NumberDataPoint, Sum, metric, number_data_point,
};
use std::time::{Duration, Instant};

const SIGNALS: [&str; 3] = ["logs", "metrics", "traces"];
/// Every meter metric name starts with this.
pub const PREFIX: &str = "fabric.spindle.";

/// What one Batch committed, by signal: (encoded bytes, records).
#[derive(Clone, Copy, Debug, Default)]
pub struct Committed {
    pub logs: (u64, u64),
    pub metrics: (u64, u64),
    pub traces: (u64, u64),
}

#[derive(Debug)]
struct Bucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
}

/// Output counters and the optional rate cap.
#[derive(Debug)]
pub struct Meter {
    start_ns: u64,
    committed: [(u64, u64); 3],
    delivered_batches: u64,
    delivered_bytes: u64,
    throttled: Duration,
    bucket: Option<Bucket>,
}

fn attr(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.into())),
        }),
        ..Default::default()
    }
}

fn point(start_ns: u64, now: u64, value: f64, attributes: Vec<KeyValue>) -> NumberDataPoint {
    NumberDataPoint {
        attributes,
        start_time_unix_nano: start_ns,
        time_unix_nano: now,
        value: Some(number_data_point::Value::AsDouble(value)),
        ..Default::default()
    }
}

fn counter(name: &str, unit: &str, points: Vec<NumberDataPoint>) -> Metric {
    Metric {
        name: name.into(),
        unit: unit.into(),
        data: Some(metric::Data::Sum(Sum {
            data_points: points,
            aggregation_temporality: AggregationTemporality::Cumulative as i32,
            is_monotonic: true,
        })),
        ..Default::default()
    }
}

fn gauge(name: &str, unit: &str, now: u64, value: f64) -> Metric {
    Metric {
        name: name.into(),
        unit: unit.into(),
        data: Some(metric::Data::Gauge(Gauge {
            data_points: vec![point(0, now, value, vec![])],
        })),
        ..Default::default()
    }
}

impl Meter {
    /// A meter started at `start_ns`, capping delivery at `max_bytes_per_s` if given.
    pub fn new(start_ns: u64, max_bytes_per_s: Option<u64>, max_batch: usize) -> Self {
        let bucket = max_bytes_per_s.map(|rate| {
            let burst = (rate as f64).max(max_batch as f64);
            Bucket {
                rate: rate as f64,
                burst,
                tokens: burst,
                last: Instant::now(),
            }
        });
        Self {
            start_ns,
            committed: [(0, 0); 3],
            delivered_batches: 0,
            delivered_bytes: 0,
            throttled: Duration::ZERO,
            bucket,
        }
    }

    pub fn record_commit(&mut self, c: Committed) {
        for (slot, (bytes, records)) in self.committed.iter_mut().zip([c.logs, c.metrics, c.traces])
        {
            slot.0 += bytes;
            slot.1 += records;
        }
    }

    pub fn record_delivery(&mut self, bytes: usize) {
        self.delivered_batches += 1;
        self.delivered_bytes += bytes as u64;
    }

    pub fn delivered_bytes(&self) -> u64 {
        self.delivered_bytes
    }

    pub fn committed_bytes(&self) -> u64 {
        self.committed.iter().map(|c| c.0).sum()
    }

    /// Take `bytes` from the rate cap. Returns how long to wait before sending them
    /// (zero without a cap or with enough tokens); the tokens are taken either way, so
    /// the caller must wait that long before it sends.
    pub fn take(&mut self, bytes: usize) -> Duration {
        let Some(b) = self.bucket.as_mut() else {
            return Duration::ZERO;
        };
        let now = Instant::now();
        b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * b.rate).min(b.burst);
        b.last = now;
        b.tokens -= bytes as f64;
        if b.tokens >= 0.0 {
            Duration::ZERO
        } else {
            let wait = Duration::from_secs_f64(-b.tokens / b.rate);
            self.throttled += wait;
            wait
        }
    }

    /// Give back tokens taken for a send that did not happen.
    pub fn untake(&mut self, bytes: usize) {
        if let Some(b) = self.bucket.as_mut() {
            b.tokens = (b.tokens + bytes as f64).min(b.burst);
        }
    }

    /// The meter's metrics at `now`, with the current Spool, unacknowledged and
    /// log-backlog levels.
    pub fn metrics(&self, now: u64, spool_bytes: u64, unacked: u64, backlog: u64) -> Vec<Metric> {
        let by_signal = |f: fn(&(u64, u64)) -> u64| {
            SIGNALS
                .iter()
                .zip(&self.committed)
                .map(|(s, c)| point(self.start_ns, now, f(c) as f64, vec![attr("signal", s)]))
                .collect()
        };
        vec![
            counter("fabric.spindle.committed.bytes", "By", by_signal(|c| c.0)),
            counter("fabric.spindle.committed.records", "1", by_signal(|c| c.1)),
            counter(
                "fabric.spindle.delivered.batches",
                "1",
                vec![point(
                    self.start_ns,
                    now,
                    self.delivered_batches as f64,
                    vec![],
                )],
            ),
            counter(
                "fabric.spindle.delivered.bytes",
                "By",
                vec![point(
                    self.start_ns,
                    now,
                    self.delivered_bytes as f64,
                    vec![],
                )],
            ),
            counter(
                "fabric.spindle.throttled.seconds",
                "s",
                vec![point(
                    self.start_ns,
                    now,
                    self.throttled.as_secs_f64(),
                    vec![],
                )],
            ),
            gauge("fabric.spindle.spool.bytes", "By", now, spool_bytes as f64),
            gauge("fabric.spindle.unacked.batches", "1", now, unacked as f64),
            gauge(
                "fabric.spindle.log.backlog.bytes",
                "By",
                now,
                backlog as f64,
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cap_delays_by_the_deficit_and_never_below_one_batch() {
        let mut m = Meter::new(1, Some(100_000), 1 << 20);
        assert_eq!(
            m.take(1 << 20),
            Duration::ZERO,
            "a full burst is available at start"
        );
        let wait = m.take(50_000);
        assert!((0.49..=0.51).contains(&wait.as_secs_f64()), "{wait:?}");
        m.untake(50_000);
        let mut free = Meter::new(1, None, 1 << 20);
        assert_eq!(free.take(usize::MAX / 2), Duration::ZERO);
    }

    #[test]
    fn counters_accumulate_by_signal() {
        let mut m = Meter::new(5, None, 1 << 20);
        m.record_commit(Committed {
            logs: (100, 2),
            metrics: (40, 3),
            traces: (0, 0),
        });
        m.record_commit(Committed {
            logs: (10, 1),
            ..Default::default()
        });
        m.record_delivery(150);
        assert_eq!((m.committed_bytes(), m.delivered_bytes()), (150, 150));
        let metrics = m.metrics(9, 1, 2, 3);
        assert_eq!(metrics.len(), 8);
        let Some(metric::Data::Sum(sum)) = &metrics[0].data else {
            panic!()
        };
        assert_eq!(sum.data_points[0].start_time_unix_nano, 5);
        assert_eq!(
            sum.data_points[0].value,
            Some(number_data_point::Value::AsDouble(110.0))
        );
    }
}
