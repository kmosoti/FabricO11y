use fabric::observation::{
    Data, Identity, Level, MetricType, ObservationV3, Outcome, REGISTRY_V3, StoredObservation,
    VERSION_V3,
};
use fabric::run::RunContext;
use fabric_collect::spindle_bridge::{IngestEnvelope, observation_to_telemetry};
use fabric_query::unified::{QueryDomain, UnifiedQuerySpec, execute_unified_query};
use fabric_store::{OlapParquetStore, validated_storage_layout};

#[tokio::test]
async fn projected_run_context_survives_storage_and_query() {
    let dir = tempfile::Builder::new()
        .prefix("spindle-correlation-")
        .tempdir()
        .unwrap();
    let store = OlapParquetStore::new(validated_storage_layout(dir.path()).unwrap());
    let run = "11111111-2222-4333-8444-555555555555";
    let other = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let mut metrics = Vec::new();
    let mut logs = Vec::new();
    let mut traces = Vec::new();
    for run_id in [run, other] {
        let signals = [
            Data::Metric {
                name: "latency".into(),
                unit: "ms".into(),
                value: 1.0,
                metric_type: MetricType::Gauge,
                reset: false,
            },
            Data::Log {
                level: Level::Info,
                message: "log".into(),
            },
            Data::Event {
                name: "event".into(),
                message: "event".into(),
            },
            Data::Gap {
                source: "source".into(),
                reason: "gap".into(),
            },
            Data::ServiceRequest {
                service: "worker".into(),
                operation: "work".into(),
                duration_ns: 1000,
                status: Outcome::Ok,
                trace_id: None,
            },
        ];
        for (i, data) in signals.into_iter().enumerate() {
            let obs = StoredObservation::V3(ObservationV3 {
                version: VERSION_V3,
                registry: REGISTRY_V3.into(),
                identity: Identity {
                    producer: "worker".into(),
                    generation: "g".into(),
                    sequence: i as u64,
                },
                timestamp_ns: 1_700_000_000_000_000_000,
                resource: "worker.service".into(),
                context: RunContext {
                    run_id: run_id.into(),
                },
                data,
            });
            match observation_to_telemetry(&obs, "node") {
                IngestEnvelope::Metric(row) => metrics.push(row),
                IngestEnvelope::Log(row) => logs.push(row),
                IngestEnvelope::Trace(row) => traces.push(row),
            }
        }
    }
    store.write_metrics(&metrics).unwrap();
    store.write_logs(&logs).unwrap();
    store.write_traces(&traces).unwrap();
    for (domain, column, expected) in [
        (QueryDomain::Metrics, "labels_json", 1),
        (QueryDomain::Logs, "attributes_json", 3),
        (QueryDomain::Traces, "attributes_json", 1),
    ] {
        // The unified query offers substring narrowing, not a structured JSON selector.
        // Parse the returned metadata for the exact key/value before joining signals.
        let result = execute_unified_query(
            dir.path(),
            &UnifiedQuerySpec {
                domain,
                where_clauses: vec![format!("{column}~{run}")],
                ..Default::default()
            },
            &[],
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.row_count, expected, "{domain:?}");
        for row in result.rows {
            let metadata: serde_json::Value =
                serde_json::from_str(row[column].as_str().unwrap()).unwrap();
            assert_eq!(metadata["context_run_id"], run);
        }
    }
}

#[tokio::test]
async fn projected_v4_parent_survives_wire_parquet_and_query() {
    use fabric_collect::spindle_bridge::parse_ingest_line;
    use serde_json::json;

    const TRACE: &str = "11111111111111112222222222222222";
    const OTHER_TRACE: &str = "33333333333333334444444444444444";
    const CHILD: &str = "0000000000000002";
    const PARENT: &str = "0000000000000001";
    let dir = tempfile::Builder::new()
        .prefix("spindle-parent-trace-")
        .tempdir()
        .unwrap();
    let store = OlapParquetStore::new(validated_storage_layout(dir.path()).unwrap());

    let span = |producer: &str, trace: &str, id: &str, parent: Option<&str>| {
        let raw = json!({
            "version": 4,
            "registry": "fabrico11y/4",
            "context": {"run_id": "11111111-2222-4333-8444-555555555555"},
            "identity": {"producer": producer, "generation": "g", "sequence": 1},
            "timestamp_ns": 1_700_000_000_000_000_000u64,
            "resource": "worker.service",
            "data": {
                "kind": "service_request",
                "service": "worker",
                "operation": "work",
                "duration_ns": 1000,
                "status": "ok",
                "trace_id": null
            },
            "span": {"trace_id": trace, "span_id": id, "parent_span_id": parent}
        })
        .to_string();
        let IngestEnvelope::Trace(row) = parse_ingest_line(&raw, "node").unwrap() else {
            panic!("a v4 span must project into a trace");
        };
        row
    };
    let query = UnifiedQuerySpec {
        domain: QueryDomain::Traces,
        where_clauses: vec![format!("trace_id={TRACE}")],
        ..Default::default()
    };
    store
        .write_traces(&[
            span("child", TRACE, CHILD, Some(PARENT)),
            span("other-trace", OTHER_TRACE, PARENT, None),
        ])
        .unwrap();
    let prefix = execute_unified_query(dir.path(), &query, &[], None)
        .await
        .unwrap();
    assert_eq!(prefix.row_count, 1);
    assert_eq!(prefix.rows[0]["span_id"], CHILD);
    assert_eq!(prefix.rows[0]["parent_span_id"], PARENT);

    let legacy = json!({
        "version": 3,
        "registry": "fabrico11y/3",
        "context": {"run_id": "11111111-2222-4333-8444-555555555555"},
        "identity": {"producer": "legacy", "generation": "g", "sequence": 1},
        "timestamp_ns": 1_700_000_000_000_000_000u64,
        "resource": "worker.service",
        "data": {
            "kind": "service_request",
            "service": "worker",
            "operation": "legacy",
            "duration_ns": 1000,
            "status": "ok",
            "trace_id": TRACE
        }
    });
    let IngestEnvelope::Trace(legacy) = parse_ingest_line(&legacy.to_string(), "node").unwrap()
    else {
        panic!("a legacy service request must project into a trace");
    };
    store
        .write_traces(&[
            legacy,
            span("parent", TRACE, PARENT, None),
            span("duplicate-id", TRACE, PARENT, None),
        ])
        .unwrap();
    let rows = execute_unified_query(dir.path(), &query, &[], None)
        .await
        .unwrap();
    assert_eq!(rows.row_count, 4);
    let mut parent_producers = Vec::new();
    let mut legacy_rows = 0;
    let mut child_rows = 0;
    for row in rows.rows {
        let meta: serde_json::Value =
            serde_json::from_str(row["attributes_json"].as_str().unwrap()).unwrap();
        match row["span_id"].as_str().unwrap() {
            PARENT => {
                assert_eq!(meta["version"], 4);
                assert!(row["parent_span_id"].is_null());
                parent_producers.push(meta["producer"].as_str().unwrap().to_owned());
            }
            CHILD => {
                assert_eq!(meta["version"], 4);
                assert_eq!(row["parent_span_id"], PARENT);
                child_rows += 1;
            }
            "g:1" => {
                assert_eq!(meta["version"], 3);
                assert!(row["parent_span_id"].is_null());
                legacy_rows += 1;
            }
            other => panic!("unexpected span {other}"),
        }
    }
    parent_producers.sort_unstable();
    assert_eq!(parent_producers, ["duplicate-id", "parent"]);
    assert_eq!((legacy_rows, child_rows), (1, 1));
}
