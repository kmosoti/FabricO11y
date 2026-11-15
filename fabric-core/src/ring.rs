//! Sans-I/O `<= 32 MiB` bounded RAM ring buffer using Gorilla delta-of-delta
//! timestamps (`wrapping` modular arithmetic) and IEEE-754 XOR bit encoding, plus
//! Euclidean 5-minute rollup bucket alignment.

use crate::MetricPoint;
use std::collections::{BTreeMap, VecDeque};

/// Default memory budget for the in-RAM hot time-series ring (`32 MiB`).
pub const DEFAULT_RING_BUDGET_BYTES: usize = 32 * 1024 * 1024;

/// 5-minute rollup bucket width in milliseconds.
pub const ROLLUP_BUCKET_MS: i64 = 300_000;

/// Aligns a timestamp in milliseconds to its floor 5-minute bucket via Euclidean division.
#[must_use]
pub fn align_rollup_bucket_ms(timestamp_ms: i64) -> i64 {
    timestamp_ms
        .div_euclid(ROLLUP_BUCKET_MS)
        .saturating_mul(ROLLUP_BUCKET_MS)
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct SeriesKey {
    name: String,
    host: String,
    slice: String,
    unit: String,
    labels_json: String,
}

impl SeriesKey {
    fn from_point(pt: &MetricPoint) -> Self {
        Self {
            name: pt.name.clone(),
            host: pt.host.clone(),
            slice: pt.slice.clone(),
            unit: pt.unit.clone(),
            labels_json: pt.labels_json.clone(),
        }
    }

    fn heap_bytes(&self) -> usize {
        self.name.len()
            + self.host.len()
            + self.slice.len()
            + self.unit.len()
            + self.labels_json.len()
            + 64
    }
}

fn metric_point_bytes(pt: &MetricPoint) -> usize {
    std::mem::size_of::<MetricPoint>()
        + pt.name.len()
        + pt.host.len()
        + pt.slice.len()
        + pt.unit.len()
        + pt.labels_json.len()
}

/// Lossless delta-of-delta timestamp + XOR value compressed block for one series.
#[derive(Debug, Clone)]
struct CompressedSeriesBlock {
    key: SeriesKey,
    max_ts: i64,
    count: usize,
    first_ts: i64,
    prev_ts: i64,
    prev_delta: i64,
    first_val_bits: u64,
    prev_val_bits: u64,
    /// Varint ZigZag delta-of-delta timestamps (after first sample).
    ts_payload: Vec<u8>,
    /// Compact XOR payload: header byte (`0` = identical to previous `f64`, `1..=8` = non-zero bytes of XOR diff).
    val_payload: Vec<u8>,
    pids: Vec<Option<u32>>,
}

impl CompressedSeriesBlock {
    fn new(key: SeriesKey, pt: &MetricPoint) -> Self {
        Self {
            key,
            max_ts: pt.timestamp_ms,
            count: 1,
            first_ts: pt.timestamp_ms,
            prev_ts: pt.timestamp_ms,
            prev_delta: 0,
            first_val_bits: pt.value.to_bits(),
            prev_val_bits: pt.value.to_bits(),
            ts_payload: Vec::new(),
            val_payload: Vec::new(),
            pids: vec![pt.pid],
        }
    }

    fn approx_bytes(&self) -> usize {
        self.key.heap_bytes()
            + self.ts_payload.len()
            + self.val_payload.len()
            + self.pids.len() * std::mem::size_of::<Option<u32>>()
            + std::mem::size_of::<Self>()
    }

    fn push(&mut self, pt: &MetricPoint) -> usize {
        let before = self.approx_bytes();
        // Modular two's-complement arithmetic makes delta-of-delta encoding exact
        // across arbitrary (including out-of-order and boundary) i64 timestamps.
        let delta = pt.timestamp_ms.wrapping_sub(self.prev_ts);
        let dod = delta.wrapping_sub(self.prev_delta);
        encode_zigzag_i64(dod, &mut self.ts_payload);
        self.prev_ts = pt.timestamp_ms;
        self.prev_delta = delta;
        self.max_ts = self.max_ts.max(pt.timestamp_ms);

        let bits = pt.value.to_bits();
        let xor = self.prev_val_bits ^ bits;
        if xor == 0 {
            self.val_payload.push(0);
        } else {
            let raw = xor.to_le_bytes();
            let used = raw.iter().rposition(|&b| b != 0).map_or(1, |pos| pos + 1);
            self.val_payload.push(used as u8);
            self.val_payload.extend_from_slice(&raw[..used]);
        }
        self.prev_val_bits = bits;
        self.pids.push(pt.pid);
        self.count += 1;
        self.approx_bytes().saturating_sub(before)
    }

    fn decode(&self) -> Vec<MetricPoint> {
        let mut out = Vec::with_capacity(self.count);
        out.push(MetricPoint {
            timestamp_ms: self.first_ts,
            name: self.key.name.clone(),
            value: f64::from_bits(self.first_val_bits),
            host: self.key.host.clone(),
            slice: self.key.slice.clone(),
            unit: self.key.unit.clone(),
            pid: self.pids.first().copied().flatten(),
            labels_json: self.key.labels_json.clone(),
        });

        let mut ts_cursor = 0usize;
        let mut val_cursor = 0usize;
        let mut prev_ts = self.first_ts;
        let mut prev_delta = 0i64;
        let mut prev_bits = self.first_val_bits;

        for idx in 1..self.count {
            let dod = decode_zigzag_i64(&self.ts_payload, &mut ts_cursor);
            let delta = prev_delta.wrapping_add(dod);
            let ts = prev_ts.wrapping_add(delta);
            prev_ts = ts;
            prev_delta = delta;

            let tag = *self.val_payload.get(val_cursor).unwrap_or(&0) as usize;
            val_cursor += 1;
            let bits = if tag == 0 {
                prev_bits
            } else {
                let mut raw = [0u8; 8];
                let copy_len = tag
                    .min(8)
                    .min(self.val_payload.len().saturating_sub(val_cursor));
                raw[..copy_len]
                    .copy_from_slice(&self.val_payload[val_cursor..val_cursor + copy_len]);
                val_cursor += copy_len;
                prev_bits ^ u64::from_le_bytes(raw)
            };
            prev_bits = bits;

            out.push(MetricPoint {
                timestamp_ms: ts,
                name: self.key.name.clone(),
                value: f64::from_bits(bits),
                host: self.key.host.clone(),
                slice: self.key.slice.clone(),
                unit: self.key.unit.clone(),
                pid: self.pids.get(idx).copied().flatten(),
                labels_json: self.key.labels_json.clone(),
            });
        }

        out
    }
}

/// Summary statistics for `HotMetricRing`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RingStats {
    pub budget_bytes: usize,
    pub used_bytes: usize,
    pub block_count: usize,
    pub sample_count: usize,
    pub evicted_blocks: u64,
}

/// Fixed-capacity RAM metric ring buffer capped strictly under `budget_bytes`.
#[derive(Debug)]
pub struct HotMetricRing {
    budget_bytes: usize,
    used_bytes: usize,
    evicted_blocks: u64,
    series_order: VecDeque<SeriesKey>,
    series: BTreeMap<SeriesKey, Vec<CompressedSeriesBlock>>,
    unflushed: VecDeque<MetricPoint>,
    unflushed_bytes: usize,
}

impl Default for HotMetricRing {
    fn default() -> Self {
        Self::with_capacity_bytes(DEFAULT_RING_BUDGET_BYTES)
    }
}

impl HotMetricRing {
    /// Creates a hot metric ring bounded by `budget_bytes` (default `32 MiB`).
    #[must_use]
    pub fn with_capacity_bytes(budget_bytes: usize) -> Self {
        Self {
            budget_bytes: budget_bytes.max(4096),
            used_bytes: 0,
            evicted_blocks: 0,
            series_order: VecDeque::new(),
            series: BTreeMap::new(),
            unflushed: VecDeque::new(),
            unflushed_bytes: 0,
        }
    }

    /// Inserts a batch of metric points, maintaining Gorilla block compression and eviction.
    pub fn push_batch(&mut self, points: &[MetricPoint]) {
        for pt in points {
            self.push_one(pt);
        }
    }

    fn push_one(&mut self, pt: &MetricPoint) {
        let key = SeriesKey::from_point(pt);
        let blocks = self.series.entry(key.clone()).or_default();
        if let Some(last) = blocks.last_mut()
            && last.count < 256
        {
            let added = last.push(pt);
            self.used_bytes = self.used_bytes.saturating_add(added);
        } else {
            let block = CompressedSeriesBlock::new(key.clone(), pt);
            let added = block.approx_bytes();
            blocks.push(block);
            self.used_bytes = self.used_bytes.saturating_add(added);
            self.series_order.push_back(key);
        }

        let pt_bytes = metric_point_bytes(pt);
        self.unflushed.push_back(pt.clone());
        self.unflushed_bytes = self.unflushed_bytes.saturating_add(pt_bytes);

        self.enforce_budget();
    }

    /// Current estimated byte size of queued unflushed metric samples.
    #[must_use]
    pub fn unflushed_bytes(&self) -> usize {
        self.unflushed_bytes
    }

    /// Returns `true` while the unflushed queue maintains 25% headroom below its soft cap.
    #[must_use]
    pub fn can_buffer_unflushed(&self) -> bool {
        admission_kernel::telemetry::ring_can_buffer(
            self.unflushed_bytes as u64,
            self.budget_bytes as u64,
        )
    }

    fn enforce_budget(&mut self) {
        // Keep unflushed queue under half of total ring budget at all times.
        let unflushed_cap = (self.budget_bytes / 2).max(2048);
        while self.unflushed_bytes > unflushed_cap {
            let Some(dropped) = self.unflushed.pop_front() else {
                self.unflushed_bytes = 0;
                break;
            };
            self.unflushed_bytes = self
                .unflushed_bytes
                .saturating_sub(metric_point_bytes(&dropped));
        }

        while self.used_bytes + self.unflushed_bytes > self.budget_bytes {
            let Some(key) = self.series_order.pop_front() else {
                break;
            };
            if let Some(blocks) = self.series.get_mut(&key)
                && !blocks.is_empty()
            {
                let removed = blocks.remove(0);
                self.used_bytes = self.used_bytes.saturating_sub(removed.approx_bytes());
                self.evicted_blocks = self.evicted_blocks.saturating_add(1);
                if blocks.is_empty() {
                    let _ = self.series.remove(&key);
                }
            }
        }
    }

    /// Decodes metric points with `timestamp_ms >= since_ms`.
    #[must_use]
    pub fn query_since(&self, since_ms: i64) -> Vec<MetricPoint> {
        let mut out = Vec::new();
        for blocks in self.series.values() {
            for block in blocks {
                if block.max_ts < since_ms {
                    continue;
                }
                for pt in block.decode() {
                    if pt.timestamp_ms >= since_ms {
                        out.push(pt);
                    }
                }
            }
        }
        out.sort_by_key(|p| p.timestamp_ms);
        out
    }

    /// Drains all unflushed metric points for periodic Parquet flushing.
    pub fn drain_unflushed(&mut self) -> Vec<MetricPoint> {
        self.unflushed_bytes = 0;
        self.unflushed.drain(..).collect()
    }

    /// Re-queues drained metric points at the front if a Parquet flush fails.
    pub fn restore_unflushed(&mut self, points: Vec<MetricPoint>) {
        for pt in points.into_iter().rev() {
            self.unflushed_bytes = self.unflushed_bytes.saturating_add(metric_point_bytes(&pt));
            self.unflushed.push_front(pt);
        }
        self.enforce_budget();
    }

    #[must_use]
    pub fn stats(&self) -> RingStats {
        let mut block_count = 0;
        let mut sample_count = 0;
        for blocks in self.series.values() {
            block_count += blocks.len();
            for b in blocks {
                sample_count += b.count;
            }
        }
        RingStats {
            budget_bytes: self.budget_bytes,
            used_bytes: self.used_bytes + self.unflushed_bytes,
            block_count,
            sample_count,
            evicted_blocks: self.evicted_blocks,
        }
    }
}

/// Encodes and immediately decodes one homogeneous `CompressedSeriesBlock` for verification.
#[must_use]
pub fn roundtrip_series_block(points: &[MetricPoint]) -> Vec<MetricPoint> {
    let Some(first) = points.first() else {
        return Vec::new();
    };
    let mut block = CompressedSeriesBlock::new(SeriesKey::from_point(first), first);
    for pt in &points[1..] {
        let _ = block.push(pt);
    }
    block.decode()
}

/// Encodes a signed `i64` into ZigZag LEB128 varint bytes.
pub fn encode_zigzag_i64(val: i64, out: &mut Vec<u8>) {
    let mut zigzag = ((val << 1) ^ (val >> 63)) as u64;
    loop {
        let byte = (zigzag & 0x7f) as u8;
        zigzag >>= 7;
        if zigzag == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

/// Decodes a signed `i64` from ZigZag LEB128 varint bytes starting at `cursor`.
pub fn decode_zigzag_i64(buf: &[u8], cursor: &mut usize) -> i64 {
    let mut result = 0u64;
    let mut shift = 0u32;
    while *cursor < buf.len() {
        let byte = buf[*cursor];
        *cursor += 1;
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift >= 64 {
            break;
        }
    }
    ((result >> 1) as i64) ^ (-((result & 1) as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gorilla_compression_roundtrips_and_respects_budget() {
        let mut ring = HotMetricRing::with_capacity_bytes(16 * 1024);
        let mut batch = Vec::new();
        for i in 0..400 {
            batch.push(MetricPoint {
                timestamp_ms: 1_700_000_000_000 + i * 1000,
                name: "host.mem.available_bytes".into(),
                value: (1024.0 * 1024.0) + (i as f64) * 12.5,
                host: "fedora-blade".into(),
                slice: "agent.slice".into(),
                unit: format!("unit-{}.scope", i % 5),
                pid: Some((i as u32) + 100),
                labels_json: "{}".into(),
            });
        }
        ring.push_batch(&batch);
        let stats = ring.stats();
        assert!(stats.used_bytes <= 16 * 1024, "used={}", stats.used_bytes);
        let recent = ring.query_since(1_700_000_350_000);
        assert!(!recent.is_empty());
        for pt in recent {
            assert!(pt.timestamp_ms >= 1_700_000_350_000);
        }
    }

    #[test]
    fn gorilla_extreme_out_of_order_timestamps_and_floats_roundtrip_bit_exact() {
        let samples = [
            (0i64, -0.0f64),
            (i64::MAX, f64::MAX),
            (-5, f64::MIN),
            (i64::MIN, f64::EPSILON),
            (42, f64::NAN),
            (-1, 0.0),
            (i64::MAX, f64::NEG_INFINITY),
            (i64::MIN + 1, f64::INFINITY),
        ];
        let input: Vec<MetricPoint> = samples
            .iter()
            .enumerate()
            .map(|(idx, &(ts, val))| MetricPoint {
                timestamp_ms: ts,
                name: "extreme".into(),
                value: val,
                host: "h".into(),
                slice: "s".into(),
                unit: "u".into(),
                pid: Some(idx as u32),
                labels_json: "{}".into(),
            })
            .collect();
        let decoded = roundtrip_series_block(&input);
        assert_eq!(decoded.len(), input.len());
        for (orig, rec) in input.iter().zip(decoded.iter()) {
            assert_eq!(orig.timestamp_ms, rec.timestamp_ms);
            assert_eq!(orig.value.to_bits(), rec.value.to_bits());
            assert_eq!(orig.pid, rec.pid);
        }
    }

    #[test]
    fn rollup_bucket_alignment_is_euclidean_and_idempotent() {
        for ts in [
            i64::MIN / 2,
            -300_001,
            -300_000,
            -1,
            0,
            1,
            299_999,
            300_000,
            1_700_000_123_456,
        ] {
            let bucket = align_rollup_bucket_ms(ts);
            assert!(bucket <= ts);
            assert!(bucket + ROLLUP_BUCKET_MS > ts);
            assert_eq!(align_rollup_bucket_ms(bucket), bucket);
        }
    }

    #[test]
    fn ring_preserves_labels_json_and_restores_unflushed_in_order() {
        let mut ring = HotMetricRing::with_capacity_bytes(8 * 1024);
        assert!(ring.can_buffer_unflushed());
        let points = vec![
            MetricPoint {
                timestamp_ms: 100,
                name: "custom.lat".into(),
                value: 1.25,
                host: "fedora-blade".into(),
                slice: "agent.slice".into(),
                unit: "spindle.service".into(),
                pid: Some(42),
                labels_json: r#"{"route":"/v1/ingest","ok":true}"#.into(),
            },
            MetricPoint {
                timestamp_ms: 200,
                name: "custom.lat".into(),
                value: 2.5,
                host: "fedora-blade".into(),
                slice: "agent.slice".into(),
                unit: "spindle.service".into(),
                pid: Some(42),
                labels_json: r#"{"route":"/v1/query","ok":false}"#.into(),
            },
        ];
        ring.push_batch(&points);

        let decoded = ring.query_since(0);
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].labels_json, points[0].labels_json);
        assert_eq!(decoded[1].labels_json, points[1].labels_json);

        let drained = ring.drain_unflushed();
        assert_eq!(drained.len(), 2);
        ring.push_batch(&[MetricPoint {
            timestamp_ms: 300,
            name: "custom.lat".into(),
            value: 3.75,
            host: "fedora-blade".into(),
            slice: "agent.slice".into(),
            unit: "spindle.service".into(),
            pid: Some(42),
            labels_json: r#"{"route":"/v1/flush"}"#.into(),
        }]);
        ring.restore_unflushed(drained);
        let restored = ring.drain_unflushed();
        assert_eq!(
            restored.iter().map(|p| p.timestamp_ms).collect::<Vec<_>>(),
            vec![100, 200, 300]
        );
    }
}
