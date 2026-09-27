//! Process-level oracle for CLI_API.md, registered before the research CLI exists.
//! The fixture is caller-authored version-1 JSON, never the demo generator output.
//! Open questions in the contracts: the exact E1 digest preimage encoding, the JSON
//! encoding of a MatchedRow digest, and the number of buffer retries for a given
//! capacity/batch. This oracle uses the public E1 hash function, accepts digest
//! bytes as either a JSON byte array or lowercase hex, and checks no retry count.
use fabric_o11y::{
    Attribute, Event, EventId, EventTime, ObservedTime, Payload, ResourceId, Scalar, SourceId,
    TenantId,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{
    ffi::OsStr,
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
use storage_probe::coverage::{self, SealedSnapshot};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "fabric-research-cli-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
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
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn run(args: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fabric-research"))
        .args(args)
        .output()
        .unwrap()
}
fn ok(args: &[&OsStr]) -> Value {
    let output = run(args);
    assert!(
        output.status.success(),
        "command failed: {:?}; stderr: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("one JSON success report")
}
fn fails(args: &[&OsStr], code: i32) {
    let output = run(args);
    assert_eq!(
        output.status.code(),
        Some(code),
        "args: {:?}; stderr: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stderr.is_empty());
}
fn s(value: &str) -> &OsStr {
    OsStr::new(value)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn file_hash(path: &Path) -> String {
    hex(&Sha256::digest(fs::read(path).unwrap()))
}
fn assert_digest(value: &Value, expected: &[u8; 32]) {
    if let Some(array) = value.as_array() {
        let bytes: Vec<u8> = array
            .iter()
            .map(|x| u8::try_from(x.as_u64().unwrap()).unwrap())
            .collect();
        assert_eq!(bytes.as_slice(), expected);
    } else {
        assert_eq!(value.as_str().unwrap(), hex(expected));
    }
}
fn assert_checkpoint(report: &Value, path: &Path, history: u64) -> String {
    let digest = report["checkpoint_sha256"].as_str().unwrap();
    assert_eq!(digest.len(), 64);
    assert!(
        digest
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );
    assert_eq!(digest, file_hash(path));
    assert_eq!(report["checkpoint"], path.to_str().unwrap());
    assert_eq!(report["history_pages"], history);
    digest.to_owned()
}
fn event(position: usize) -> Event {
    let bodies = [
        "ordinary",
        "rare\u{2003}match",
        "ordinary",
        "ordinary",
        "rare\tmatch",
        "ordinary",
        "ordinary",
        "ordinary",
        "ordinary",
        "rare match",
        "ordinary",
        "ordinary",
    ];
    Event {
        // Duplicate identities at distinct positions must retain both positions.
        id: EventId(if position == 9 {
            101
        } else {
            position as u64 + 100
        }),
        tenant: TenantId(if position == 10 { 8 } else { 7 }),
        source: SourceId(91),
        resource: ResourceId(92),
        event_time: EventTime(position as i64 - 6),
        observed_time: ObservedTime(position as i64 + 200),
        attributes: vec![
            Attribute {
                key: "flag".into(),
                value: Scalar::Bool(position % 2 == 0),
            },
            Attribute {
                key: "flag".into(),
                value: Scalar::I64(-7),
            },
        ],
        payload: if position == 2 {
            Payload::Gauge {
                name: "temperature".into(),
                value: -0.0,
                unit: "C".into(),
            }
        } else {
            Payload::Log {
                body: bodies[position].into(),
            }
        },
    }
}
fn fixture_rows() -> Vec<Event> {
    (0..12).map(event).collect()
}
fn wire_event(row: &Event) -> Value {
    let attrs: Vec<Value> = row
        .attributes
        .iter()
        .map(|attribute| {
            let scalar = match &attribute.value {
                Scalar::Bool(x) => json!({"kind":"bool","value":x}),
                Scalar::I64(x) => json!({"kind":"i64","value":x}),
                Scalar::U64(x) => json!({"kind":"u64","value":x}),
                Scalar::String(x) => json!({"kind":"string","value":x}),
                Scalar::F64(x) => {
                    json!({"kind":"f64_bits","value":format!("{:016x}", x.to_bits())})
                }
            };
            json!({"key": attribute.key, "value": scalar})
        })
        .collect();
    let payload = match &row.payload {
        Payload::Log { body } => json!({"kind":"log","body":body}),
        Payload::Gauge { name, value, unit } => json!({
            "kind":"gauge", "name":name,
            "value_bits":format!("{:016x}", value.to_bits()), "unit":unit
        }),
    };
    json!({
        "id":row.id.0, "tenant":row.tenant.0, "source":row.source.0,
        "resource":row.resource.0, "event_time":row.event_time.0,
        "observed_time":row.observed_time.0, "attributes":attrs, "payload":payload
    })
}
fn write_input(path: &Path, rows: &[Event]) {
    let events: Vec<Value> = rows.iter().map(wire_event).collect();
    fs::write(
        path,
        serde_json::to_vec(&json!({"version":1,"events":events})).unwrap(),
    )
    .unwrap();
}
fn query_json(path: &Path, token: &str) {
    fs::write(
        path,
        serde_json::to_vec(&json!({
            "start_ns":-6, "end_ns":5, "tenant":7, "token":token
        }))
        .unwrap(),
    )
    .unwrap();
}
fn expected_rows(rows: &[Event], block_rows: usize) -> (Vec<usize>, Vec<(usize, usize, [u8; 32])>) {
    let selected: Vec<(usize, &Event)> = rows.iter().enumerate().filter(|(_, row)| {
        (-6..=5).contains(&row.event_time.0)
            && row.tenant.0 == 7
            && matches!(&row.payload, Payload::Log { body } if body.split_whitespace().any(|word| word == "rare"))
    }).collect();
    let positions = selected.iter().map(|(position, _)| *position).collect();
    let expected = selected
        .into_iter()
        .map(|(position, row)| {
            (
                position / block_rows,
                position % block_rows,
                coverage::rows_digest(std::slice::from_ref(row)),
            )
        })
        .collect();
    (positions, expected)
}
fn assert_rows(report: &Value, expected: &[(usize, usize, [u8; 32])]) {
    let actual = report["rows"].as_array().unwrap();
    assert_eq!(actual.len(), expected.len());
    for (row, (block, offset, digest)) in actual.iter().zip(expected) {
        assert_eq!(row["block"], *block);
        assert_eq!(row["offset"], *offset);
        assert_digest(&row["digest"], digest);
    }
}

#[test]
fn process_lifecycle_preserves_roots_and_restarts_from_partial_history() {
    let scratch = Scratch::new();
    let source = scratch.path("external.json");
    let wrong = scratch.path("wrong.json");
    let log = scratch.path("events.fol2");
    let snap = scratch.path("snapshot");
    let trusted = scratch.path("trusted.json");
    let q = scratch.path("query.json");
    let changed_q = scratch.path("changed-query.json");
    let cp1 = scratch.path("page1.json");
    let cp2 = scratch.path("page2.json");
    let cp3 = scratch.path("page3.json");
    let cp4 = scratch.path("page4.json");
    let rows = fixture_rows();
    write_input(&source, &rows);
    query_json(&q, "rare");
    query_json(&changed_q, "ordinary");

    let ingest = ok(&[
        s("ingest"),
        source.as_os_str(),
        log.as_os_str(),
        s("1"),
        s("2"),
    ]);
    assert_eq!(ingest["input_events"], 12);
    assert_eq!(ingest["already_committed"], 0);
    assert_eq!(ingest["appended"], 12);
    assert_eq!(ingest["capacity"], 1);
    assert_eq!(ingest["batch"], 2);
    // The contract reports retries, but does not require a particular count.
    assert!(ingest["buffer_retries"].as_u64().is_some());
    assert_eq!(ingest["peak_buffer_events"], 1);
    assert_eq!(
        ingest["log_bytes"].as_u64().unwrap(),
        fs::metadata(&log).unwrap().len()
    );
    let committed = fs::read(&log).unwrap();
    let retry = ok(&[
        s("ingest"),
        source.as_os_str(),
        log.as_os_str(),
        s("1"),
        s("2"),
    ]);
    assert_eq!(retry["already_committed"], 12);
    assert_eq!(retry["appended"], 0);
    assert_eq!(fs::read(&log).unwrap(), committed);
    let mut wrong_rows = fixture_rows();
    wrong_rows[3].payload = Payload::Log {
        body: "changed".into(),
    };
    write_input(&wrong, &wrong_rows);
    fails(
        &[
            s("ingest"),
            wrong.as_os_str(),
            log.as_os_str(),
            s("1"),
            s("2"),
        ],
        1,
    );
    assert_eq!(fs::read(&log).unwrap(), committed);

    let published = ok(&[
        s("publish"),
        log.as_os_str(),
        snap.as_os_str(),
        trusted.as_os_str(),
        s("4"),
        s("901"),
    ]);
    assert_eq!(published["snapshot"], snap.to_str().unwrap());
    assert_eq!(published["publication"], trusted.to_str().unwrap());
    let trusted_bytes = fs::read(&trusted).unwrap();
    let anchor =
        SealedSnapshot::new(fixture_rows(), NonZeroUsize::new(4).unwrap(), 901, None).unwrap();
    let publication: Value = serde_json::from_slice(&trusted_bytes).unwrap();
    assert_eq!(publication["block_rows"], 4);
    assert_eq!(publication["anchor"]["snapshot_id"], 901);
    assert_eq!(publication["anchor"]["block_count"], 3);
    assert_eq!(publication["anchor"]["row_count"], 12);
    assert_digest(&publication["anchor"]["root"], &anchor.anchor().root);

    let first = ok(&[
        s("query"),
        snap.as_os_str(),
        trusted.as_os_str(),
        q.as_os_str(),
        s("101"),
        cp1.as_os_str(),
    ]);
    assert_eq!(first["complete"], false);
    assert_eq!(first["unavailable"], json!([1]));
    assert_eq!(first["positions"], json!([1, 9]));
    let digest1 = assert_checkpoint(&first, &cp1, 1);
    let first_bytes = fs::read(&cp1).unwrap();
    assert!(first["reads"]["raw_files"].as_u64().unwrap() <= 2);
    fails(
        &[
            s("verify"),
            source.as_os_str(),
            trusted.as_os_str(),
            q.as_os_str(),
            cp1.as_os_str(),
            s(&digest1),
        ],
        1,
    );
    fails(
        &[
            s("resume"),
            snap.as_os_str(),
            trusted.as_os_str(),
            changed_q.as_os_str(),
            cp1.as_os_str(),
            s(&digest1),
            s("all"),
            cp2.as_os_str(),
        ],
        1,
    );
    assert!(!cp2.exists());
    let wrong_digest = format!(
        "{}{}",
        if digest1.starts_with('0') { '1' } else { '0' },
        &digest1[1..]
    );
    fails(
        &[
            s("resume"),
            snap.as_os_str(),
            trusted.as_os_str(),
            q.as_os_str(),
            cp1.as_os_str(),
            s(&wrong_digest),
            s("all"),
            cp2.as_os_str(),
        ],
        1,
    );
    assert!(!cp2.exists());

    let block = snap.join("block-1.json");
    let cold_dir = snap.join("cold");
    fs::create_dir(&cold_dir).unwrap();
    let cold = cold_dir.join("block-1.json");
    fs::rename(&block, &cold).unwrap();
    let second = ok(&[
        s("resume"),
        snap.as_os_str(),
        trusted.as_os_str(),
        q.as_os_str(),
        cp1.as_os_str(),
        s(&digest1),
        s("all"),
        cp2.as_os_str(),
    ]);
    assert_eq!(second["complete"], true);
    assert_eq!(second["unavailable"], json!([]));
    let (positions, expected) = expected_rows(&rows, 4);
    assert_eq!(positions, vec![1, 4, 9]);
    assert_eq!(second["positions"], json!(positions));
    assert_rows(&second, &expected);
    let digest2 = assert_checkpoint(&second, &cp2, 2);
    assert_eq!(fs::read(&cp1).unwrap(), first_bytes);
    let verification = ok(&[
        s("verify"),
        source.as_os_str(),
        trusted.as_os_str(),
        q.as_os_str(),
        cp2.as_os_str(),
        s(&digest2),
    ]);
    assert_eq!(verification["verified"], true);
    assert_eq!(verification["matched"], 3);
    assert_eq!(verification["input_events"], 12);

    let cold_bytes = fs::read(&cold).unwrap();
    fs::remove_file(&cold).unwrap();
    let missing = ok(&[
        s("query"),
        snap.as_os_str(),
        trusted.as_os_str(),
        q.as_os_str(),
        s("all"),
        cp3.as_os_str(),
    ]);
    assert_eq!(missing["complete"], false);
    assert_eq!(missing["unavailable"], json!([1]));
    let digest3 = assert_checkpoint(&missing, &cp3, 1);
    fs::write(&cold, cold_bytes).unwrap();
    let restored = ok(&[
        s("resume"),
        snap.as_os_str(),
        trusted.as_os_str(),
        q.as_os_str(),
        cp3.as_os_str(),
        s(&digest3),
        s("all"),
        cp4.as_os_str(),
    ]);
    assert_eq!(restored["complete"], true);
    assert_rows(&restored, &expected);
    assert_checkpoint(&restored, &cp4, 2);

    let tampered = scratch.path("tampered.json");
    let mut bytes = first_bytes.clone();
    bytes[0] ^= 1;
    fs::write(&tampered, bytes).unwrap();
    let no_cp = scratch.path("no-checkpoint.json");
    fails(
        &[
            s("resume"),
            snap.as_os_str(),
            trusted.as_os_str(),
            q.as_os_str(),
            tampered.as_os_str(),
            s(&digest1),
            s("all"),
            no_cp.as_os_str(),
        ],
        1,
    );
    assert!(!no_cp.exists());
    assert_eq!(fs::read(&trusted).unwrap(), trusted_bytes);
}

#[test]
fn malformed_inputs_missing_log_and_successor_reject_old_checkpoint() {
    let scratch = Scratch::new();
    let source = scratch.path("source.json");
    let malformed = scratch.path("malformed.json");
    let log = scratch.path("events.fol2");
    let snapshot = scratch.path("snapshot");
    let trusted = scratch.path("trusted.json");
    let q = scratch.path("query.json");
    let checkpoint = scratch.path("checkpoint.json");
    let rows = fixture_rows();
    write_input(&source, &rows);
    query_json(&q, "rare");
    fs::write(&malformed, b"{\"version\":1,\"events\":[] ,\"extra\":true}").unwrap();
    fails(
        &[
            s("ingest"),
            malformed.as_os_str(),
            log.as_os_str(),
            s("1"),
            s("1"),
        ],
        1,
    );
    assert!(!log.exists());
    fails(
        &[
            s("publish"),
            log.as_os_str(),
            snapshot.as_os_str(),
            trusted.as_os_str(),
            s("4"),
            s("1"),
        ],
        1,
    );
    assert!(!snapshot.exists());
    assert!(!trusted.exists());
    ok(&[
        s("ingest"),
        source.as_os_str(),
        log.as_os_str(),
        s("1"),
        s("1"),
    ]);
    ok(&[
        s("publish"),
        log.as_os_str(),
        snapshot.as_os_str(),
        trusted.as_os_str(),
        s("4"),
        s("901"),
    ]);
    let original = ok(&[
        s("query"),
        snapshot.as_os_str(),
        trusted.as_os_str(),
        q.as_os_str(),
        s("101"),
        checkpoint.as_os_str(),
    ]);
    let digest = assert_checkpoint(&original, &checkpoint, 1);
    let checkpoint_bytes = fs::read(&checkpoint).unwrap();
    let original_trust = fs::read(&trusted).unwrap();

    let late_source = scratch.path("late.json");
    let mut successor_rows = fixture_rows();
    successor_rows.push(Event {
        id: EventId(777),
        tenant: TenantId(7),
        source: SourceId(91),
        resource: ResourceId(92),
        event_time: EventTime(0),
        observed_time: ObservedTime(999),
        attributes: vec![],
        payload: Payload::Log {
            body: "rare late".into(),
        },
    });
    write_input(&late_source, &successor_rows);
    let appended = ok(&[
        s("ingest"),
        late_source.as_os_str(),
        log.as_os_str(),
        s("1"),
        s("1"),
    ]);
    assert_eq!(appended["already_committed"], 12);
    assert_eq!(appended["appended"], 1);
    let successor = scratch.path("successor");
    let successor_trust = scratch.path("successor-trusted.json");
    ok(&[
        s("publish"),
        log.as_os_str(),
        successor.as_os_str(),
        successor_trust.as_os_str(),
        s("4"),
        s("902"),
    ]);
    let rejected = scratch.path("rejected.json");
    fails(
        &[
            s("resume"),
            successor.as_os_str(),
            successor_trust.as_os_str(),
            q.as_os_str(),
            checkpoint.as_os_str(),
            s(&digest),
            s("all"),
            rejected.as_os_str(),
        ],
        1,
    );
    assert!(!rejected.exists());
    assert_eq!(fs::read(&checkpoint).unwrap(), checkpoint_bytes);
    assert_eq!(fs::read(&trusted).unwrap(), original_trust);
    fails(
        &[
            s("verify"),
            late_source.as_os_str(),
            trusted.as_os_str(),
            q.as_os_str(),
            checkpoint.as_os_str(),
            s(&digest),
        ],
        1,
    );
}

#[test]
fn generate_is_a_fresh_convenience_source() {
    let scratch = Scratch::new();
    let path = scratch.path("generated.json");
    ok(&[s("generate"), s("17"), s("4"), path.as_os_str()]);
    let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["events"].as_array().unwrap().len(), 4);
    assert_eq!(value["events"][0]["payload"]["kind"], "log");
    assert_eq!(
        value["events"][0]["payload"]["body"],
        "common request0 rare"
    );
    assert_eq!(value["events"][1]["payload"]["kind"], "gauge");
    assert_eq!(value["events"][2]["payload"]["body"], "common request2");
    let before = fs::read(&path).unwrap();
    fails(&[s("generate"), s("17"), s("4"), path.as_os_str()], 1);
    assert_eq!(fs::read(&path).unwrap(), before);
    fails(&[s("generate"), s("17"), s("4")], 2);
}
