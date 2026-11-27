use chrono::Utc;
use fabric_core::{
    AgentSessionRecord, FsEventRecord, HostSnapshot, LogRecord, MetricPoint, UnitRecord,
};
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
            host: "workstation-01".into(),
            slice: "agent.slice".into(),
            unit: "u1".into(),
            pid: Some(11),
            labels_json: "{}".into(),
        },
        MetricPoint {
            timestamp_ms: now - 1_000,
            name: "cgroup.slice.memory_current_bytes".into(),
            value: 300.0,
            host: "workstation-01".into(),
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
        hostname: "workstation-01".into(),
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

#[tokio::test]
async fn unified_predicates_use_the_selected_domains_field_types() {
    let dir = non_tmp_dir();
    let catalog = CatalogStore::open(dir.path()).unwrap();
    let olap = OlapParquetStore::new(catalog.layout().clone());
    let now = Utc::now().timestamp_millis();
    olap.write_logs(&[LogRecord {
        timestamp_ms: now,
        message: "007's status is true".into(),
        ..Default::default()
    }])
    .unwrap();
    olap.write_fs_events(&[FsEventRecord {
        timestamp_ms: now,
        path: "/data/work/file".into(),
        delta_bytes: -4096,
        owner_pid: Some(42),
        ..Default::default()
    }])
    .unwrap();
    let snapshot = HostSnapshot {
        units: vec![
            UnitRecord {
                name: "007".into(),
                transient: true,
                main_pid: Some(42),
                cpu_usage_usec: u64::MAX,
                populated: true,
                ..Default::default()
            },
            UnitRecord {
                name: "inactive.service".into(),
                main_pid: Some(43),
                cpu_usage_usec: u64::MAX - 1,
                ..Default::default()
            },
        ],
        agents: vec![AgentSessionRecord {
            session_id: "agent-fixture".into(),
            pids: vec![42, 43],
            write_bytes: 9_007_199_254_740_993,
            ..Default::default()
        }],
        ..Default::default()
    };

    for (domain, predicates, field, expected) in [
        (
            QueryDomain::Units,
            vec![
                "transient=TRUE",
                "populated!=false",
                "MAIN_PID=42",
                "name=007",
            ],
            "name",
            "007",
        ),
        (
            QueryDomain::Units,
            vec![
                "transient=false",
                "main_pid>42",
                "collect_mode_enabled=false",
            ],
            "name",
            "inactive.service",
        ),
        (
            QueryDomain::Units,
            vec!["cpu_usage_usec=18446744073709551615"],
            "name",
            "007",
        ),
        (
            QueryDomain::Logs,
            vec!["message=007's status is true"],
            "message",
            "007's status is true",
        ),
        (
            QueryDomain::Fs,
            vec!["delta_bytes=-4096", "owner_pid=42"],
            "path",
            "/data/work/file",
        ),
        (
            QueryDomain::Agents,
            vec!["pid_count=2", "write_bytes=9007199254740993"],
            "session_id",
            "agent-fixture",
        ),
    ] {
        let result = execute_unified_query(
            dir.path(),
            &UnifiedQuerySpec {
                domain,
                where_clauses: predicates.into_iter().map(str::to_owned).collect(),
                ..Default::default()
            },
            &[],
            Some(&snapshot),
        )
        .await
        .unwrap();
        assert_eq!(result.row_count, 1);
        assert_eq!(result.rows[0][field].as_str(), Some(expected));
    }

    for (predicate, expected) in [
        ("transient=maybe", "invalid Boolean value"),
        ("populated<true", "invalid for Boolean"),
        ("main_pid=-1", "invalid UInt32 value"),
        ("main_pid=42.5", "invalid UInt32 value"),
        (
            "cpu_usage_usec=18446744073709551616",
            "invalid UInt64 value",
        ),
        ("owner_pid=42", "unknown units field"),
    ] {
        let Err(error) = execute_unified_query(
            dir.path(),
            &UnifiedQuerySpec {
                domain: QueryDomain::Units,
                where_clauses: vec![predicate.into()],
                ..Default::default()
            },
            &[],
            Some(&snapshot),
        )
        .await
        else {
            panic!("accepted {predicate}");
        };
        assert!(error.contains(expected), "{predicate}: {error}");
    }
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
        host: "workstation-01".into(),
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
        host: "workstation-01".into(),
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
