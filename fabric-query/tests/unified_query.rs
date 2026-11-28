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
    olap.write_metrics(&[MetricPoint {
        timestamp_ms: now,
        name: "large".into(),
        value: 1e100,
        ..Default::default()
    }])
    .unwrap();
    olap.write_logs(&[LogRecord {
        timestamp_ms: now,
        message: "007's status is true".into(),
        ..Default::default()
    }])
    .unwrap();
    olap.write_fs_events(&[
        FsEventRecord {
            timestamp_ms: now,
            path: "/data/work/file".into(),
            delta_bytes: -4096,
            owner_pid: Some(42),
            ..Default::default()
        },
        FsEventRecord {
            timestamp_ms: now,
            path: "/min".into(),
            delta_bytes: i64::MIN,
            ..Default::default()
        },
        FsEventRecord {
            timestamp_ms: now,
            path: "/min-plus-one".into(),
            delta_bytes: i64::MIN + 1,
            ..Default::default()
        },
    ])
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

    // Adjacent values around f64's precision limit and the UInt64 maximum must
    // remain distinguishable for equality and ordering in DataFusion.
    let boundary_snapshot = HostSnapshot {
        units: [
            ("below", 9_007_199_254_740_992),
            ("above", 9_007_199_254_740_993),
            ("max-1", u64::MAX - 1),
            ("max", u64::MAX),
        ]
        .into_iter()
        .map(|(name, cpu_usage_usec)| UnitRecord {
            name: name.into(),
            cpu_usage_usec,
            ..Default::default()
        })
        .collect(),
        ..Default::default()
    };
    for (predicate, expected) in [
        ("cpu_usage_usec=9007199254740993", vec!["above"]),
        (
            "cpu_usage_usec>9007199254740992",
            vec!["above", "max-1", "max"],
        ),
        ("cpu_usage_usec=18446744073709551614", vec!["max-1"]),
        (
            "cpu_usage_usec<18446744073709551615",
            vec!["below", "above", "max-1"],
        ),
    ] {
        let result = execute_unified_query(
            dir.path(),
            &UnifiedQuerySpec {
                domain: QueryDomain::Units,
                where_clauses: vec![predicate.into()],
                ..Default::default()
            },
            &[],
            Some(&boundary_snapshot),
        )
        .await
        .unwrap();
        let mut actual: Vec<_> = result
            .rows
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect();
        actual.sort_unstable();
        let mut expected = expected;
        expected.sort_unstable();
        assert_eq!(actual, expected, "{predicate}");
    }

    for (predicate, expected) in [
        ("delta_bytes=-9223372036854775808", "/min"),
        ("delta_bytes=-9223372036854775807", "/min-plus-one"),
    ] {
        let result = execute_unified_query(
            dir.path(),
            &UnifiedQuerySpec {
                domain: QueryDomain::Fs,
                where_clauses: vec![predicate.into()],
                ..Default::default()
            },
            &[],
            Some(&snapshot),
        )
        .await
        .unwrap();
        assert_eq!(result.row_count, 1, "{predicate}");
        assert_eq!(result.rows[0]["path"].as_str(), Some(expected));
    }

    let result = execute_unified_query(
        dir.path(),
        &UnifiedQuerySpec {
            domain: QueryDomain::Metrics,
            where_clauses: vec!["value=1e100".into()],
            ..Default::default()
        },
        &[],
        None,
    )
    .await
    .unwrap();
    assert_eq!(result.row_count, 1);

    for (domain, predicate) in [
        (QueryDomain::Metrics, "value=NaN"),
        (QueryDomain::Metrics, "value=inf"),
        (QueryDomain::Metrics, "value=1e999"),
        (QueryDomain::Fs, "delta_bytes=-9223372036854775809"),
        (QueryDomain::Fs, "delta_bytes=9223372036854775808"),
        (QueryDomain::Fs, "size_bytes=-1"),
        (QueryDomain::Logs, "owner_pid=42"),
    ] {
        assert!(
            execute_unified_query(
                dir.path(),
                &UnifiedQuerySpec {
                    domain,
                    where_clauses: vec![predicate.into()],
                    ..Default::default()
                },
                &[],
                Some(&snapshot)
            )
            .await
            .is_err(),
            "accepted {predicate}"
        );
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

fn partition_metric(timestamp_ms: i64) -> MetricPoint {
    MetricPoint {
        timestamp_ms,
        name: "partition.evidence".into(),
        value: 1.0,
        host: "fixture".into(),
        slice: "fixture".into(),
        unit: "fixture".into(),
        pid: None,
        labels_json: "{}".into(),
    }
}

#[tokio::test]
async fn out_of_range_metric_is_queryable_from_fallback_partition() {
    let dir = non_tmp_dir();
    let catalog = CatalogStore::open(dir.path()).unwrap();
    let olap = OlapParquetStore::new(catalog.layout().clone());
    let paths = olap.write_metrics(&[partition_metric(i64::MIN)]).unwrap();
    assert_eq!(paths.len(), 1);
    let partition = paths[0]
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let partition_day =
        chrono::NaiveDate::parse_from_str(partition.strip_prefix("dt=").unwrap(), "%Y-%m-%d")
            .unwrap();
    let cutoff = chrono::DateTime::<Utc>::from_timestamp_millis(0)
        .unwrap()
        .date_naive();
    assert!(
        partition_day > cutoff,
        "fixture requires fallback partition after cutoff"
    );
    let query = UnifiedQuerySpec {
        domain: QueryDomain::Metrics,
        where_clauses: vec!["timestamp_ms<0".into()],
        ..Default::default()
    };
    let result = execute_unified_query(dir.path(), &query, &[], None)
        .await
        .unwrap();
    assert_eq!(result.row_count, 1);
    assert_eq!(result.rows[0]["timestamp_ms"].as_i64(), Some(i64::MIN));
    let array = result.batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<arrow::array::Int64Array>()
        .unwrap();
    assert_eq!(array.value(0), i64::MIN);
}

#[tokio::test]
async fn metric_since_hint_preserves_typed_rows_across_midnight() {
    let dir = non_tmp_dir();
    let catalog = CatalogStore::open(dir.path()).unwrap();
    let olap = OlapParquetStore::new(catalog.layout().clone());
    // 2024-01-03T00:00:00Z; include the immediately preceding millisecond.
    let midnight = 1_704_240_000_000_i64;
    let samples = [
        midnight - 86_400_000,
        midnight - 1,
        midnight,
        midnight + 86_400_000,
    ];
    let points: Vec<_> = samples.into_iter().map(partition_metric).collect();
    assert_eq!(olap.write_metrics(&points).unwrap().len(), 3);
    let full = olap
        .snapshot_pinned_parquet_files_bounded("metrics", None, 64, 1024 * 1024)
        .unwrap();
    let selected = olap
        .snapshot_pinned_parquet_files_bounded("metrics", Some(midnight), 64, 1024 * 1024)
        .unwrap();
    assert_eq!(full.len(), 3);
    assert_eq!(selected.len(), 2);
    drop(full);
    drop(selected);
    let hinted = UnifiedQuerySpec {
        domain: QueryDomain::Metrics,
        since_ms: Some(midnight),
        ..Default::default()
    };
    let textual = UnifiedQuerySpec {
        domain: QueryDomain::Metrics,
        where_clauses: vec![format!("timestamp_ms>={midnight}")],
        ..Default::default()
    };
    let left = execute_unified_query(dir.path(), &hinted, &[], None)
        .await
        .unwrap();
    let right = execute_unified_query(dir.path(), &textual, &[], None)
        .await
        .unwrap();
    assert_eq!(left.schema, right.schema);
    assert_eq!(left.row_count, 2);
    assert_eq!(left.rows, right.rows);
    let left_batch = arrow::compute::concat_batches(&left.schema, &left.batches).unwrap();
    let right_batch = arrow::compute::concat_batches(&right.schema, &right.batches).unwrap();
    assert_eq!(left_batch, right_batch);
    assert_eq!(
        left.rows
            .iter()
            .map(|r| r["timestamp_ms"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![midnight + 86_400_000, midnight]
    );
}
