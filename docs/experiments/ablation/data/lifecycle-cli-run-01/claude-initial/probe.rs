//! Independent adversarial probes for the S2/E3 lifecycle CLI and S5 collect
//! adapter, written against tools/storage-probe/CLI_API.md and
//! COLLECT_API.md. Invokes the real compiled binary as a subprocess. Read-
//! only against both repos; writes only under this scratch package's own
//! temp directories. resume_e3 is never run.

use fabric_o11y::{Event, EventId, EventTime, ObservedTime, Payload, ResourceId, Scalar, SourceId, TenantId};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
use storage_probe::disk;

const BINARY: &str = "/tmp/fabric-cli-b/tools/storage-probe/target/release/fabric-research";

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "fabric-cli-review-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(BINARY).args(args).output().expect("failed to spawn fabric-research")
}

fn log(id: u64, tenant: u64, time: i64, body: &str) -> Event {
    Event {
        id: EventId(id),
        tenant: TenantId(tenant),
        source: SourceId(id ^ 5),
        resource: ResourceId(id ^ 9),
        event_time: EventTime(time),
        observed_time: ObservedTime(time),
        attributes: vec![],
        payload: Payload::Log { body: body.into() },
    }
}

fn write_input(scratch: &Scratch, name: &str, rows: &[Event]) -> PathBuf {
    let path = scratch.path(name);
    let bytes = disk::encode_events(rows).unwrap();
    fs::write(&path, bytes).unwrap();
    path
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

// === A) Confirm the parse_query repair: duplicate JSON keys rejected =======
// The Value-based "exactly 4 keys" check alone cannot see a duplicate key
// (serde_json::Value collapses repeats to the last occurrence, so the
// resulting object still has exactly 4 unique keys). Only re-parsing the
// ORIGINAL BYTES into the strongly-typed Query struct -- whose derived
// Deserialize rejects duplicate fields -- catches this.

#[test]
fn duplicate_query_json_key_is_rejected_not_silently_collapsed() {
    let scratch = Scratch::new();
    let rows = vec![log(1, 1, 0, "a")];
    let input = write_input(&scratch, "input.fol2json", &rows);
    let log_path = scratch.path("events.log");
    let ingest = run(&["ingest", input.to_str().unwrap(), log_path.to_str().unwrap(), "8", "8"]);
    assert!(ingest.status.success(), "setup ingest failed: {}", stderr(&ingest));
    let snapshot = scratch.path("snapshot");
    let trusted = scratch.path("publication.json");
    let publish = run(&[
        "publish", log_path.to_str().unwrap(), snapshot.to_str().unwrap(),
        trusted.to_str().unwrap(), "4", "1",
    ]);
    assert!(publish.status.success(), "setup publish failed: {}", stderr(&publish));

    // A genuinely duplicate key, unique-key count still 4, with the LAST
    // occurrence of start_ns holding a different value than the first.
    let query_path = scratch.path("dup-query.json");
    fs::write(
        &query_path,
        br#"{"start_ns":0,"end_ns":0,"tenant":null,"token":null,"start_ns":9999}"#,
    ).unwrap();
    let checkpoint = scratch.path("dup-checkpoint.json");
    let out = run(&[
        "query", snapshot.to_str().unwrap(), trusted.to_str().unwrap(),
        query_path.to_str().unwrap(), "all", checkpoint.to_str().unwrap(),
    ]);
    assert!(
        !out.status.success(),
        "a duplicate-keyed QUERY_JSON must be rejected, not silently resolved \
         to the last occurrence: stdout={:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !checkpoint.exists(),
        "a rejected duplicate-keyed query must not write a checkpoint before failing"
    );

    // Control: the SAME JSON with the duplicate key removed (using the
    // second value, 9999) must be structurally valid JSON accepted normally
    // (sanity check that the harness/query itself is not simply broken).
    let control_path = scratch.path("control-query.json");
    fs::write(&control_path, br#"{"start_ns":9999,"end_ns":9999,"tenant":null,"token":null}"#).unwrap();
    let control_checkpoint = scratch.path("control-checkpoint.json");
    let out = run(&[
        "query", snapshot.to_str().unwrap(), trusted.to_str().unwrap(),
        control_path.to_str().unwrap(), "all", control_checkpoint.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "control query unexpectedly failed: {}", stderr(&out));
}

// === B) Whole-operation-before-output: an invalid query writes no checkpoint

#[test]
fn failed_query_writes_no_checkpoint_and_no_stdout_json() {
    let scratch = Scratch::new();
    let rows = vec![log(1, 1, 0, "a"), log(2, 1, 1, "b")];
    let input = write_input(&scratch, "input.fol2json", &rows);
    let log_path = scratch.path("events.log");
    assert!(run(&["ingest", input.to_str().unwrap(), log_path.to_str().unwrap(), "8", "8"]).status.success());
    let snapshot = scratch.path("snapshot");
    let trusted = scratch.path("publication.json");
    assert!(run(&[
        "publish", log_path.to_str().unwrap(), snapshot.to_str().unwrap(),
        trusted.to_str().unwrap(), "1", "1",
    ]).status.success());

    let query_path = scratch.path("query.json");
    fs::write(&query_path, br#"{"start_ns":0,"end_ns":0,"tenant":null,"token":null}"#).unwrap();
    let checkpoint = scratch.path("checkpoint.json");
    // Wrong availability length (2 rows -> 2 blocks with block_rows=1, but
    // supply only 1 bit) must fail entirely before any write.
    let out = run(&[
        "query", snapshot.to_str().unwrap(), trusted.to_str().unwrap(),
        query_path.to_str().unwrap(), "0", checkpoint.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "a failed command must emit no stdout JSON");
    assert!(!checkpoint.exists(), "a failed query must not create the checkpoint file");
}

// === C) capacity=1 ingest and an exact, byte-identical retry ===============

#[test]
fn capacity_one_ingest_retries_correctly_and_identical_retry_appends_zero() {
    let scratch = Scratch::new();
    let rows: Vec<Event> = (0..6).map(|i| log(i, 1, i as i64, "x")).collect();
    let input = write_input(&scratch, "input.fol2json", &rows);
    let log_path = scratch.path("events.log");

    let first = run(&["ingest", input.to_str().unwrap(), log_path.to_str().unwrap(), "1", "1"]);
    assert!(first.status.success(), "capacity=1 ingest failed: {}", stderr(&first));
    let report: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(report["input_events"], 6);
    assert_eq!(report["already_committed"], 0);
    assert_eq!(report["appended"], 6);
    assert_eq!(report["capacity"], 1);
    assert_eq!(report["batch"], 1);
    // With capacity 1, the buffer can never hold more than 1 event at once.
    assert_eq!(report["peak_buffer_events"], 1);

    let bytes_after_first = fs::read(&log_path).unwrap();

    let second = run(&["ingest", input.to_str().unwrap(), log_path.to_str().unwrap(), "1", "1"]);
    assert!(second.status.success(), "identical retry failed: {}", stderr(&second));
    let report2: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(report2["already_committed"], 6, "retry must recognize all 6 rows as already committed");
    assert_eq!(report2["appended"], 0, "an identical retry must append zero new rows");
    assert_eq!(report2["input_events"], 6);

    let bytes_after_second = fs::read(&log_path).unwrap();
    assert_eq!(bytes_after_first, bytes_after_second, "an identical retry must preserve every log byte");
}

// === D) Duplicate EventIds / identical records retained at distinct positions

#[test]
fn duplicate_identical_events_are_retained_not_collapsed_during_ingest() {
    let scratch = Scratch::new();
    let rows = vec![log(7, 1, 0, "dup"), log(7, 1, 0, "dup"), log(7, 1, 0, "dup")];
    let input = write_input(&scratch, "input.fol2json", &rows);
    let log_path = scratch.path("events.log");
    let out = run(&["ingest", input.to_str().unwrap(), log_path.to_str().unwrap(), "1", "1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["appended"], 3, "three identical rows must all be appended, not deduplicated");

    let snapshot = scratch.path("snapshot");
    let trusted = scratch.path("publication.json");
    assert!(run(&[
        "publish", log_path.to_str().unwrap(), snapshot.to_str().unwrap(),
        trusted.to_str().unwrap(), "3", "1",
    ]).status.success());
    let query_path = scratch.path("query.json");
    fs::write(&query_path, br#"{"start_ns":0,"end_ns":0,"tenant":null,"token":"dup"}"#).unwrap();
    let checkpoint = scratch.path("checkpoint.json");
    let out = run(&[
        "query", snapshot.to_str().unwrap(), trusted.to_str().unwrap(),
        query_path.to_str().unwrap(), "all", checkpoint.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["positions"].as_array().unwrap().len(), 3, "all three duplicate rows must remain distinct positions");
}

// === E) resume: mismatched query and tampered digest both fail cleanly =====

#[test]
fn resume_rejects_mismatched_query_and_tampered_digest_without_new_checkpoint() {
    let scratch = Scratch::new();
    let rows = vec![log(1, 1, 0, "alpha"), log(2, 1, 1, "beta")];
    let input = write_input(&scratch, "input.fol2json", &rows);
    let log_path = scratch.path("events.log");
    assert!(run(&["ingest", input.to_str().unwrap(), log_path.to_str().unwrap(), "8", "8"]).status.success());
    let snapshot = scratch.path("snapshot");
    let trusted = scratch.path("publication.json");
    assert!(run(&[
        "publish", log_path.to_str().unwrap(), snapshot.to_str().unwrap(),
        trusted.to_str().unwrap(), "1", "1",
    ]).status.success());

    let query_path = scratch.path("query.json");
    fs::write(&query_path, br#"{"start_ns":0,"end_ns":1,"tenant":null,"token":"alpha"}"#).unwrap();
    let checkpoint = scratch.path("checkpoint.json");
    // Deliberately no blocks available (2 rows, block_rows=1 -> 2 blocks) so
    // the checkpoint is Incomplete and a resume is meaningful.
    let out = run(&[
        "query", snapshot.to_str().unwrap(), trusted.to_str().unwrap(),
        query_path.to_str().unwrap(), "none", checkpoint.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["complete"], false);
    let digest = report["checkpoint_sha256"].as_str().unwrap().to_owned();

    // E1: resume with a DIFFERENT query (different token) but the SAME
    // checkpoint/digest must fail -- the checkpoint was built for "alpha".
    let wrong_query_path = scratch.path("wrong-query.json");
    fs::write(&wrong_query_path, br#"{"start_ns":0,"end_ns":1,"tenant":null,"token":"beta"}"#).unwrap();
    let resumed1 = scratch.path("resumed1.json");
    let out = run(&[
        "resume", snapshot.to_str().unwrap(), trusted.to_str().unwrap(),
        wrong_query_path.to_str().unwrap(), checkpoint.to_str().unwrap(), &digest,
        "all", resumed1.to_str().unwrap(),
    ]);
    assert!(!out.status.success(), "resume with a mismatched query must fail");
    assert!(!resumed1.exists(), "a rejected resume must not write a new checkpoint");

    // E2: resume with the CORRECT query but a bit-flipped digest must fail.
    let mut tampered_digest = digest.clone().into_bytes();
    let last = tampered_digest.len() - 1;
    tampered_digest[last] = if tampered_digest[last] == b'0' { b'1' } else { b'0' };
    let tampered_digest = String::from_utf8(tampered_digest).unwrap();
    let resumed2 = scratch.path("resumed2.json");
    let out = run(&[
        "resume", snapshot.to_str().unwrap(), trusted.to_str().unwrap(),
        query_path.to_str().unwrap(), checkpoint.to_str().unwrap(), &tampered_digest,
        "all", resumed2.to_str().unwrap(),
    ]);
    assert!(!out.status.success(), "resume with a tampered checkpoint digest must fail");
    assert!(!resumed2.exists(), "a rejected resume must not write a new checkpoint");

    // Control: the correct query + correct digest must succeed and complete.
    let resumed_ok = scratch.path("resumed-ok.json");
    let out = run(&[
        "resume", snapshot.to_str().unwrap(), trusted.to_str().unwrap(),
        query_path.to_str().unwrap(), checkpoint.to_str().unwrap(), &digest,
        "all", resumed_ok.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "control resume unexpectedly failed: {}", stderr(&out));
    assert!(resumed_ok.exists());
}

// === F) adapt-otlp: null/unknown/numeric-string/overflow rules =============

fn otlp_request(body: &str) -> String {
    format!(
        r#"{{"resourceLogs":[{{"resource":{{}},"scopeLogs":[{{"logRecords":[{{"body":{{"stringValue":{body:?}}}}}]}}]}}]}}"#
    )
}

fn config_json(first_event_id: u64) -> String {
    format!(r#"{{"tenant":1,"source":2,"resource":3,"first_event_id":{first_event_id}}}"#)
}

#[test]
fn adapt_otlp_rejects_explicit_null_at_object_and_scalar_positions() {
    let scratch = Scratch::new();
    let config_path = scratch.path("config.json");
    fs::write(&config_path, config_json(0)).unwrap();

    // 1) Explicit null for an optional OBJECT-typed field ("resource").
    let req_path = scratch.path("null-resource.json");
    fs::write(
        &req_path,
        br#"{"resourceLogs":[{"resource":null,"scopeLogs":[{"logRecords":[]}]}]}"#,
    ).unwrap();
    let output = scratch.path("out1.fol2json");
    let out = run(&["adapt-otlp", req_path.to_str().unwrap(), config_path.to_str().unwrap(), output.to_str().unwrap()]);
    assert!(!out.status.success(), "explicit null resource must be rejected");
    assert!(!output.exists());

    // 2) Explicit null for an optional STRING-typed field ("schemaUrl").
    let req_path = scratch.path("null-schema.json");
    fs::write(
        &req_path,
        br#"{"resourceLogs":[{"resource":{},"schemaUrl":null,"scopeLogs":[{"logRecords":[]}]}]}"#,
    ).unwrap();
    let output = scratch.path("out2.fol2json");
    let out = run(&["adapt-otlp", req_path.to_str().unwrap(), config_path.to_str().unwrap(), output.to_str().unwrap()]);
    assert!(!out.status.success(), "explicit null schemaUrl must be rejected");
    assert!(!output.exists());
}

#[test]
fn adapt_otlp_rejects_unknown_fields_and_numeric_string_severity() {
    let scratch = Scratch::new();
    let config_path = scratch.path("config.json");
    fs::write(&config_path, config_json(0)).unwrap();

    // Unknown top-level request field.
    let req_path = scratch.path("unknown.json");
    fs::write(&req_path, br#"{"resourceLogs":[],"extra":1}"#).unwrap();
    let output = scratch.path("out1.fol2json");
    let out = run(&["adapt-otlp", req_path.to_str().unwrap(), config_path.to_str().unwrap(), output.to_str().unwrap()]);
    assert!(!out.status.success(), "unknown request field must be rejected");

    // severityNumber as a numeric STRING must be rejected (only unsigned
    // JSON integers are accepted for severityNumber/flags/droppedCounts).
    let req_path = scratch.path("severity-string.json");
    fs::write(
        &req_path,
        br#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"severityNumber":"5"}]}]}]}"#,
    ).unwrap();
    let output = scratch.path("out2.fol2json");
    let out = run(&["adapt-otlp", req_path.to_str().unwrap(), config_path.to_str().unwrap(), output.to_str().unwrap()]);
    assert!(!out.status.success(), "severityNumber as a string must be rejected");

    // Duplicate JSON object member within a LogRecord.
    let req_path = scratch.path("dup-field.json");
    fs::write(
        &req_path,
        br#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"body":{"stringValue":"y"}}]}]}]}"#,
    ).unwrap();
    let output = scratch.path("out3.fol2json");
    let out = run(&["adapt-otlp", req_path.to_str().unwrap(), config_path.to_str().unwrap(), output.to_str().unwrap()]);
    assert!(!out.status.success(), "duplicate LogRecord.body key must be rejected");

    // Control: a genuinely valid request succeeds, proving the harness works.
    let req_path = scratch.path("valid.json");
    fs::write(&req_path, otlp_request("hello")).unwrap();
    let output = scratch.path("valid.fol2json");
    let out = run(&["adapt-otlp", req_path.to_str().unwrap(), config_path.to_str().unwrap(), output.to_str().unwrap()]);
    assert!(out.status.success(), "control request unexpectedly failed: {}", stderr(&out));
}

#[test]
fn adapt_otlp_rejects_event_id_overflow() {
    let scratch = Scratch::new();
    // first_event_id = u64::MAX, and TWO log records -> position 1 overflows.
    let config_path = scratch.path("config.json");
    fs::write(&config_path, config_json(u64::MAX)).unwrap();
    let req_path = scratch.path("two-records.json");
    fs::write(
        &req_path,
        br#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"a"}},{"body":{"stringValue":"b"}}]}]}]}"#,
    ).unwrap();
    let output = scratch.path("out.fol2json");
    let out = run(&["adapt-otlp", req_path.to_str().unwrap(), config_path.to_str().unwrap(), output.to_str().unwrap()]);
    assert!(!out.status.success(), "EventId overflow (first_event_id=u64::MAX, 2 records) must be rejected");
    assert!(!output.exists(), "a rejected adapt-otlp must not create the fresh output file");

    // Control: exactly one record at first_event_id=u64::MAX must succeed
    // (no overflow at position 0).
    let req_path = scratch.path("one-record.json");
    fs::write(
        &req_path,
        br#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"a"}}]}]}]}"#,
    ).unwrap();
    let output = scratch.path("out-ok.fol2json");
    let out = run(&["adapt-otlp", req_path.to_str().unwrap(), config_path.to_str().unwrap(), output.to_str().unwrap()]);
    assert!(out.status.success(), "control (no overflow) case unexpectedly failed: {}", stderr(&out));
}

// === G) adapt-otlp float/ordering exactness through CLI round trip =========

#[test]
fn adapt_otlp_preserves_exact_double_bits_and_attribute_order_through_cli() {
    let scratch = Scratch::new();
    let config_path = scratch.path("config.json");
    fs::write(&config_path, config_json(100)).unwrap();
    let req_path = scratch.path("floats.json");
    fs::write(
        &req_path,
        br#"{"resourceLogs":[{"resource":{"attributes":[{"key":"a","value":{"doubleValue":"NaN"}},{"key":"b","value":{"doubleValue":"-Infinity"}},{"key":"c","value":{"doubleValue":-0.0}}]},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"}}]}]}]}"#,
    ).unwrap();
    let output = scratch.path("floats.fol2json");
    let out = run(&["adapt-otlp", req_path.to_str().unwrap(), config_path.to_str().unwrap(), output.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    let bytes = fs::read(&output).unwrap();
    let decoded = disk::decode_events(&bytes).unwrap();
    assert_eq!(decoded.len(), 1);
    let attrs = &decoded[0].attributes;
    // "a" (NaN), "b" (-Infinity), "c" (-0.0) must appear first, in this exact
    // order (resource attributes come before the two presence-flag bools).
    assert_eq!(attrs[0].key, "otel.resource.attr.a");
    assert_eq!(attrs[1].key, "otel.resource.attr.b");
    assert_eq!(attrs[2].key, "otel.resource.attr.c");
    let Scalar::F64(a) = attrs[0].value else { panic!() };
    let Scalar::F64(b) = attrs[1].value else { panic!() };
    let Scalar::F64(c) = attrs[2].value else { panic!() };
    assert!(a.is_nan(), "expected NaN, got {a}");
    assert_eq!(b, f64::NEG_INFINITY);
    assert_eq!(c.to_bits(), (-0.0_f64).to_bits(), "signed zero bit pattern must survive exactly");
    assert_eq!(attrs.last().unwrap().key, "otel.log.field.observed_time_present");
}
