#![no_main]
use fabric_core::{HotMetricRing, MetricPoint, align_rollup_bucket_ms, roundtrip_series_block};
use libfuzzer_sys::fuzz_target;

fn read_u64(data: &[u8], cursor: &mut usize) -> u64 {
    let mut buf = [0u8; 8];
    for slot in &mut buf {
        if *cursor < data.len() {
            *slot = data[*cursor];
            *cursor += 1;
        }
    }
    u64::from_le_bytes(buf)
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 || data.len() > 65_536 {
        return;
    }

    let budget_bytes = 4096usize + (usize::from(data[0]) << 9);
    let count = usize::from(data[1] % 48).saturating_add(1);
    let shared_label = format!("k={}", data[2] & 0x0f);
    let mut cursor = 3usize;

    let mut homogeneous = Vec::with_capacity(count);
    let mut mixed = Vec::with_capacity(count);

    for idx in 0..count {
        if cursor >= data.len() {
            break;
        }
        let ts = read_u64(data, &mut cursor) as i64;
        let val = f64::from_bits(read_u64(data, &mut cursor));
        let tag = data.get(cursor).copied().unwrap_or(idx as u8);
        cursor = cursor.saturating_add(1);

        let aligned = align_rollup_bucket_ms(ts);
        assert_eq!(align_rollup_bucket_ms(aligned), aligned);

        let base_pt = MetricPoint {
            timestamp_ms: ts,
            name: "fuzz.metric".into(),
            value: val,
            host: "fuzz-host".into(),
            slice: "fabrico11y.slice".into(),
            unit: "fuzz.service".into(),
            pid: if tag & 1 == 0 {
                Some(u32::from(tag))
            } else {
                None
            },
            labels_json: format!(r#"{{{shared_label:?}:"v"}}"#),
        };
        homogeneous.push(base_pt.clone());

        let mut variant = base_pt;
        if tag & 2 != 0 {
            variant.name = format!("fuzz.metric.{}", tag & 0x07);
            variant.labels_json = format!(r#"{{"shard":{}}}"#, tag & 0x0f);
        }
        mixed.push(variant);
    }

    let decoded = roundtrip_series_block(&homogeneous);
    assert_eq!(decoded.len(), homogeneous.len());
    for (orig, got) in homogeneous.iter().zip(decoded.iter()) {
        assert_eq!(orig.timestamp_ms, got.timestamp_ms);
        assert_eq!(orig.value.to_bits(), got.value.to_bits());
        assert_eq!(orig.labels_json, got.labels_json);
        assert_eq!(orig.pid, got.pid);
    }

    let mut ring = HotMetricRing::with_capacity_bytes(budget_bytes);
    ring.push_batch(&mixed);
    let stats = ring.stats();
    assert!(stats.used_bytes <= stats.budget_bytes);

    let drained = ring.drain_unflushed();
    ring.restore_unflushed(drained);
    let stats_after = ring.stats();
    assert!(stats_after.used_bytes <= stats_after.budget_bytes);
    let _ = ring.can_buffer_unflushed();
});
