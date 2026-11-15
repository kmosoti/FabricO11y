use fabric_core::{
    HotMetricRing, MetricPoint, QueryDomain, ROLLUP_BUCKET_MS, UnifiedQuerySpec,
    align_rollup_bucket_ms, build_unified_sql, roundtrip_series_block,
};

fn sample_point(timestamp_ms: i64, value: f64, pid: Option<u32>, labels: &str) -> MetricPoint {
    MetricPoint {
        timestamp_ms,
        name: "fabric.test.metric".into(),
        value,
        host: "fedora-blade".into(),
        slice: "fabrico11y.slice".into(),
        unit: "fabric-daemon.service".into(),
        pid,
        labels_json: labels.into(),
    }
}

#[test]
fn gorilla_block_roundtrips_out_of_order_and_nan_bits() {
    let labels = r#"{"series":"gorilla"}"#;
    let inputs = vec![
        sample_point(1_700_000_010_000, 1.0, Some(10), labels),
        sample_point(-5_000, f64::from_bits(0x7ff8_0000_0000_0001), None, labels),
        sample_point(
            1_700_000_000_000,
            f64::from_bits(0xfff8_0000_0000_0042),
            Some(20),
            labels,
        ),
        sample_point(0, -0.0_f64, Some(0), labels),
        sample_point(0, 0.0_f64, None, labels),
        sample_point(i64::MAX / 4, f64::MAX, Some(u32::MAX), labels),
        sample_point(i64::MIN / 4, f64::MIN_POSITIVE, Some(1), labels),
    ];

    let decoded = roundtrip_series_block(&inputs);
    assert_eq!(decoded.len(), inputs.len());
    for (orig, got) in inputs.iter().zip(decoded.iter()) {
        assert_eq!(got.timestamp_ms, orig.timestamp_ms);
        assert_eq!(got.value.to_bits(), orig.value.to_bits());
        assert_eq!(got.name, orig.name);
        assert_eq!(got.host, orig.host);
        assert_eq!(got.slice, orig.slice);
        assert_eq!(got.unit, orig.unit);
        assert_eq!(got.pid, orig.pid);
        assert_eq!(got.labels_json, orig.labels_json);
    }
}

#[test]
fn rollup_alignment_is_euclidean_for_negative_and_positive_timestamps() {
    let samples = [
        i64::MIN / 2,
        -ROLLUP_BUCKET_MS - 1,
        -ROLLUP_BUCKET_MS,
        -ROLLUP_BUCKET_MS + 1,
        -1,
        0,
        1,
        ROLLUP_BUCKET_MS - 1,
        ROLLUP_BUCKET_MS,
        ROLLUP_BUCKET_MS + 1,
        1_700_000_123_456,
    ];

    let mut prev = None;
    for ts in samples {
        let aligned = align_rollup_bucket_ms(ts);
        assert_eq!(aligned.rem_euclid(ROLLUP_BUCKET_MS), 0);
        assert!(aligned <= ts);
        assert!(ts - aligned < ROLLUP_BUCKET_MS);
        assert_eq!(align_rollup_bucket_ms(aligned), aligned);
        if let Some(p) = prev {
            assert!(p <= aligned);
        }
        prev = Some(aligned);
    }
}

#[test]
fn hot_metric_ring_headroom_matches_verified_kernel() {
    for budget in [0usize, 4096, 8192, 16384] {
        let mut ring = HotMetricRing::with_capacity_bytes(budget);
        let init_stats = ring.stats();
        assert!(init_stats.used_bytes <= init_stats.budget_bytes);
        assert_eq!(
            ring.can_buffer_unflushed(),
            admission_kernel::telemetry::ring_can_buffer(
                ring.unflushed_bytes() as u64,
                init_stats.budget_bytes as u64,
            )
        );

        for idx in 0..48i64 {
            let batch = vec![sample_point(
                1_700_000_000_000 + idx * 1000,
                idx as f64 * 1.25,
                Some(idx as u32),
                r#"{"probe":"ring"}"#,
            )];
            ring.push_batch(&batch);
            let stats = ring.stats();
            assert!(stats.used_bytes <= stats.budget_bytes);
            assert_eq!(
                ring.can_buffer_unflushed(),
                admission_kernel::telemetry::ring_can_buffer(
                    ring.unflushed_bytes() as u64,
                    stats.budget_bytes as u64,
                )
            );
        }

        let drained = ring.drain_unflushed();
        ring.restore_unflushed(drained);
        let after = ring.stats();
        assert!(after.used_bytes <= after.budget_bytes);
        assert_eq!(
            ring.can_buffer_unflushed(),
            admission_kernel::telemetry::ring_can_buffer(
                ring.unflushed_bytes() as u64,
                after.budget_bytes as u64,
            )
        );
    }
}

#[test]
fn unified_query_spec_operator_precedence_and_numeric_literals() {
    let spec = UnifiedQuerySpec {
        domain: QueryDomain::Metrics,
        since_ms: Some(1_700_000_000_000),
        where_clauses: vec![
            "value>=12.5".into(),
            "value<=99.5".into(),
            "pid!=42".into(),
            "value>10".into(),
            "pid<1000".into(),
            "unit=a'b.service".into(),
            "host=42".into(),
        ],
        group_by: vec!["unit".into()],
        agg_specs: vec!["count".into(), "avg:value".into()],
        limit: 50,
    };

    let sql = build_unified_sql(&spec, Some("unit ASC")).expect("valid SQL");
    assert!(sql.contains("value >= 12.5"));
    assert!(sql.contains("value <= 99.5"));
    assert!(sql.contains("pid != 42"));
    assert!(sql.contains("value > 10"));
    assert!(sql.contains("pid < 1000"));
    assert!(sql.contains("unit = 'a''b.service'"));
    assert!(sql.contains("host = '42'"));
    assert!(sql.ends_with("LIMIT 50"));

    for bad_clause in ["bad;drop=1", "missing_operator"] {
        let bad = UnifiedQuerySpec {
            domain: QueryDomain::Metrics,
            since_ms: None,
            where_clauses: vec![bad_clause.into()],
            group_by: Vec::new(),
            agg_specs: Vec::new(),
            limit: 10,
        };
        assert!(build_unified_sql(&bad, None).is_err());
    }
}
