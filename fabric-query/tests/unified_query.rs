use chrono::Utc;
use fabric_core::{HostSnapshot, LogRecord, MetricPoint, UnitRecord};
use fabric_query::unified::{
    QueryDomain, UnifiedQuerySpec, execute_unified_query, parse_duration_ms,
};
use fabric_store::{CatalogStore, OlapParquetStore};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn non_tmp_dir() -> TempDir {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let _ = fs::create_dir_all(&base);
    tempfile::tempdir_in(base).unwrap()
}

#[tokio::test]
async fn unified_query_supports_metrics_logs_units_and_aggregations() {
    let dir = non_tmp_dir();
    let catalog = CatalogStore::open(dir.path()).unwrap();
    let olap = OlapParquetStore::new(catalog.layout().clone());
    let now = Utc::now().timestamp_millis();

    olap.write_metrics(&[
        MetricPoint {
            timestamp_ms: now - 5_000,
            name: "cgroup.slice.memory_current_bytes".into(),
            value: 100.0,
            host: "fedora-blade".into(),
            slice: "agent.slice".into(),
            unit: "u1".into(),
            pid: Some(11),
            labels_json: "{}".into(),
        },
        MetricPoint {
            timestamp_ms: now - 1_000,
            name: "cgroup.slice.memory_current_bytes".into(),
            value: 300.0,
            host: "fedora-blade".into(),
            slice: "agent.slice".into(),
            unit: "u2".into(),
            pid: Some(12),
            labels_json: "{}".into(),
        },
    ])
    .unwrap();

    olap.write_logs(&[LogRecord {
        timestamp_ms: now - 2_000,
        level: "warn".into(),
        source: "systemd".into(),
        unit: "orphan.scope".into(),
        message: "missing --collect".into(),
        attributes_json: "{}".into(),
    }])
    .unwrap();

    let agg_res = execute_unified_query(
        dir.path(),
        &UnifiedQuerySpec {
            domain: QueryDomain::Metrics,
            since_ms: Some(now - parse_duration_ms("1h").unwrap()),
            where_clauses: vec!["slice=agent.slice".into()],
            group_by: vec!["slice".into()],
            agg_specs: vec!["sum:value".into(), "count".into()],
            limit: 50,
        },
        &[],
        None,
    )
    .await
    .unwrap();
    assert_eq!(agg_res.row_count, 1);
    assert_eq!(agg_res.rows[0]["sum_value"].as_f64(), Some(400.0));
    assert_eq!(agg_res.rows[0]["count"].as_i64(), Some(2));

    let log_res = execute_unified_query(
        dir.path(),
        &UnifiedQuerySpec {
            domain: QueryDomain::Logs,
            where_clauses: vec!["message~--collect".into()],
            ..Default::default()
        },
        &[],
        None,
    )
    .await
    .unwrap();
    assert_eq!(log_res.row_count, 1);

    let snap = HostSnapshot {
        timestamp_unix_ms: now,
        hostname: "fedora-blade".into(),
        units: vec![UnitRecord {
            name: "fabric-synth-load-test.scope".into(),
            manager: "user".into(),
            slice: "agent.slice".into(),
            active_state: "active".into(),
            sub_state: "running".into(),
            transient: true,
            collect_mode_enabled: true,
            memory_current_bytes: 512 * 1024 * 1024,
            populated: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    let units_res = execute_unified_query(
        dir.path(),
        &UnifiedQuerySpec {
            domain: QueryDomain::Units,
            where_clauses: vec!["slice=agent.slice".into()],
            ..Default::default()
        },
        &[],
        Some(&snap),
    )
    .await
    .unwrap();
    assert_eq!(units_res.row_count, 1);
    assert_eq!(
        units_res.rows[0]["name"].as_str(),
        Some("fabric-synth-load-test.scope")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_symlinked_spill_dir_into_tmp_and_preserves_distinct_spindle_sequences() {
    use std::os::unix::fs::symlink;

    let dir = non_tmp_dir();
    let catalog = CatalogStore::open(dir.path()).unwrap();
    let olap = OlapParquetStore::new(catalog.layout().clone());
    let now = Utc::now().timestamp_millis();

    // Two Spindle observations in the same millisecond with identical value but distinct sequences.
    olap.write_metrics(&[MetricPoint {
        timestamp_ms: now - 500,
        name: "spindle.counter".into(),
        value: 42.0,
        host: "fedora-blade".into(),
        slice: "spindle".into(),
        unit: "spindle".into(),
        pid: None,
        labels_json: r#"{"producer":1,"generation":1,"sequence":1}"#.into(),
    }])
    .unwrap();
    let live = vec![MetricPoint {
        timestamp_ms: now - 500,
        name: "spindle.counter".into(),
        value: 42.0,
        host: "fedora-blade".into(),
        slice: "spindle".into(),
        unit: "spindle".into(),
        pid: None,
        labels_json: r#"{"producer":1,"generation":1,"sequence":2}"#.into(),
    }];
    let distinct = execute_unified_query(
        dir.path(),
        &UnifiedQuerySpec {
            domain: QueryDomain::Metrics,
            since_ms: Some(now - 60_000),
            agg_specs: vec!["sum:value".into(), "count".into()],
            limit: 10,
            ..Default::default()
        },
        &live,
        None,
    )
    .await
    .unwrap();
    assert_eq!(distinct.rows[0]["count"].as_i64(), Some(2));
    assert_eq!(distinct.rows[0]["sum_value"].as_f64(), Some(84.0));

    // Pre-existing spill symlink pointing into /tmp must be rejected.
    fs::remove_dir(catalog.layout().spill_dir()).unwrap();
    symlink("/tmp/fabrico11y-evil-spill", catalog.layout().spill_dir()).unwrap();
    let Err(err) = execute_unified_query(
        dir.path(),
        &UnifiedQuerySpec {
            domain: QueryDomain::Metrics,
            ..Default::default()
        },
        &[],
        None,
    )
    .await
    else {
        panic!("spill symlink into /tmp must fail");
    };
    assert!(
        err.contains("refusing") || err.contains("/tmp"),
        "unexpected error: {err}"
    );
}
