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
