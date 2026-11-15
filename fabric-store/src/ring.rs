//! Bounded `<= 32 MiB` RAM ring buffer using Gorilla delta-of-delta timestamps
//! and IEEE-754 XOR bit encoding for hot time-series telemetry.

use fabric_core::MetricPoint;
use std::collections::{BTreeMap, VecDeque};

/// Default memory budget for the in-RAM hot time-series ring (`32 MiB`).
pub const DEFAULT_RING_BUDGET_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct SeriesKey {
    name: String,
    host: String,
    slice: String,
    unit: String,
}

impl SeriesKey {
    fn from_point(pt: &MetricPoint) -> Self {
        Self {
            name: pt.name.clone(),
            host: pt.host.clone(),
            slice: pt.slice.clone(),
            unit: pt.unit.clone(),
        }
    }

    fn heap_bytes(&self) -> usize {
        self.name.len() + self.host.len() + self.slice.len() + self.unit.len() + 64
    }
}

/// Lossless delta-of-delta timestamp + XOR value compressed block for one series.
#[derive(Debug, Clone)]
struct CompressedSeriesBlock {
    key: SeriesKey,
    min_ts: i64,
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
            min_ts: pt.timestamp_ms,
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
        let delta = pt.timestamp_ms.saturating_sub(self.prev_ts);
        let dod = delta.saturating_sub(self.prev_delta);
        encode_zigzag_varint(dod, &mut self.ts_payload);
        self.prev_ts = pt.timestamp_ms;
        self.prev_delta = delta;
        self.min_ts = self.min_ts.min(pt.timestamp_ms);
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
            labels_json: "{}".into(),
        });

        let mut ts_cursor = 0usize;
        let mut val_cursor = 0usize;
        let mut prev_ts = self.first_ts;
        let mut prev_delta = 0i64;
        let mut prev_bits = self.first_val_bits;

        for idx in 1..self.count {
            let dod = decode_zigzag_varint(&self.ts_payload, &mut ts_cursor);
            let delta = prev_delta.saturating_add(dod);
            let ts = prev_ts.saturating_add(delta);
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
                labels_json: "{}".into(),
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
    series_order: VecDeque<(SeriesKey, usize)>,
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
            let block_idx = blocks.len();
            blocks.push(block);
            self.used_bytes = self.used_bytes.saturating_add(added);
            self.series_order.push_back((key, block_idx));
        }

        let pt_bytes = std::mem::size_of::<MetricPoint>()
            + pt.name.len()
            + pt.host.len()
            + pt.slice.len()
            + pt.unit.len()
            + pt.labels_json.len();
        self.unflushed.push_back(pt.clone());
        self.unflushed_bytes = self.unflushed_bytes.saturating_add(pt_bytes);

        self.enforce_budget();
    }

    fn enforce_budget(&mut self) {
        // Keep unflushed queue under half of total ring budget at all times.
        let unflushed_cap = (self.budget_bytes / 2).max(2048);
        while self.unflushed_bytes > unflushed_cap {
            let Some(dropped) = self.unflushed.pop_front() else {
                self.unflushed_bytes = 0;
                break;
            };
            let b = std::mem::size_of::<MetricPoint>()
                + dropped.name.len()
                + dropped.host.len()
                + dropped.slice.len()
                + dropped.unit.len()
                + dropped.labels_json.len();
            self.unflushed_bytes = self.unflushed_bytes.saturating_sub(b);
        }

        while self.used_bytes + self.unflushed_bytes > self.budget_bytes {
            let Some((key, _)) = self.series_order.pop_front() else {
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

fn encode_zigzag_varint(val: i64, out: &mut Vec<u8>) {
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

fn decode_zigzag_varint(buf: &[u8], cursor: &mut usize) -> i64 {
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
}
