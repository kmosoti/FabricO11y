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
