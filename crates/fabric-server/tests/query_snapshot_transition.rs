//! Origin: source audit found whole-Manifest metadata used for a straddling
//! page snapshot. Real storage lifecycle, public History API; no HTTP/TLS claim.
use fabric_frame::{envelope::Batch, frame::FrameLog};
use fabric_server::{
    query::{History, Plan, Query, QueryError},
    sealer::{self, Retention},
    segment,
    store::{CommitMode, Entry, Group, Store},
};
use opentelemetry_proto::tonic::{
    collector::logs::v1::ExportLogsServiceRequest,
    common::v1::{AnyValue, any_value},
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
};
use prost::Message;
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
const LIMIT: u64 = 2 * 1024 * 1024;
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root =
            PathBuf::from(std::env::var("FABRIC_SCRATCH_ROOT").expect("contained mounted scratch"))
                .join(format!("query-snapshot-transition-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("origin.json"), json!({"origin":"straddling Segment page metadata source audit", "times":[100,200,900],"expected_old_groups":[1,2],"expected_new_groups":[1,2,3]}).to_string()).unwrap();
        Self(root)
    }
    fn save(&self, name: &str, value: &Value) {
        fs::write(self.0.join(format!("{name}.json")), value.to_string()).unwrap();
    }
    fn bound(&self) {
        fn bytes(path: &Path) -> u64 {
            fs::read_dir(path)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    assert!(!entry.file_type().unwrap().is_symlink());
                    if entry.file_type().unwrap().is_dir() {
                        bytes(&entry.path())
                    } else {
                        entry.metadata().unwrap().len()
                    }
                })
                .sum()
        }
        assert!(bytes(&self.0) <= LIMIT, "fixture/evidence exceeds2MiB");
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if std::thread::panicking() || std::env::var_os("FABRIC_QUERY_SNAPSHOT_EVIDENCE").is_some()
        {
            eprintln!("query snapshot fixture retained at {}", self.0.display());
        } else {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn group(sequence: u64, time: u64) -> Group {
    let logs = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: (0..2)
                    .map(|index| LogRecord {
                        observed_time_unix_nano: time,
                        body: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(format!(
                                "known-{sequence}-{index}-λ"
                            ))),
                        }),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    let batch = Batch {
        version: 1,
        node_id: vec![7; 16],
        generation: 1,
        sequence,
        logs,
        ..Default::default()
    }
    .encode_to_vec();
    Group {
        group_sequence: sequence,
        entries: vec![Entry {
            label: "known-node".into(),
            batch,
            received_unix_nano: time,
        }],
    }
}
fn b64(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                T[((n >> (18 - 6 * i)) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}
fn ledger(groups: &[Group]) -> String {
    groups
        .iter()
        .flat_map(|g| &g.entries)
        .map(|e| {
            json!({"label":e.label,"received_ns":e.received_unix_nano,"bytes":b64(&e.batch)})
                .to_string()
                + "\n"
        })
        .collect()
}
fn oracle(s: &Scratch, name: &str, groups: &[Group], q: &Value, pages: &[Value]) -> bool {
    let root = s.0.join(name);
    fs::create_dir(&root).unwrap();
    fs::write(root.join("records.jsonl"), ledger(groups)).unwrap();
    fs::write(root.join("query.json"), q.to_string()).unwrap();
    fs::write(root.join("pages.json"), json!(pages).to_string()).unwrap();
    let mut child = Command::new("python3")
        .arg("-B")
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/qualification/query_oracle.py"),
        )
        .arg("--records")
        .arg(root.join("records.jsonl"))
        .arg("--query")
        .arg(root.join("query.json"))
        .arg("--answer")
        .arg(root.join("pages.json"))
        .stdout(File::create(root.join("stdout.json")).unwrap())
        .stderr(File::create(root.join("stderr.txt")).unwrap())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= until {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("oracle timeout; evidence retained");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let verdict: Value =
        serde_json::from_slice(&fs::read(root.join("stdout.json")).unwrap()).unwrap();
    fs::write(
        root.join("exit.json"),
        json!({"exit":status.code(),"passed":verdict["passed"]}).to_string(),
    )
    .unwrap();
    assert!(
        verdict["passed"].is_boolean(),
        "oracle must emit an actual verdict"
    );
    assert_eq!(
        status.success(),
        verdict["passed"] == true,
        "oracle exit/verdict mismatch"
    );
    s.bound();
    status.success()
}
fn chain(history: &History, mut q: Value, newest: u64, first: Option<Value>) -> Vec<Value> {
    let mut pages = Vec::new();
    if let Some(first) = first {
        q["page"] = first["next_page"].clone();
        pages.push(first);
    }
    for _ in 0..8 {
        let query: Query = serde_json::from_value(q.clone()).unwrap();
        let page = history.run(&query, newest).unwrap();
        let next = page["next_page"].clone();
        pages.push(page);
        if next.is_null() {
            return pages;
        }
        q["page"] = next;
    }
    panic!("tiny chain exceeded8 pages");
}
fn pass(state: &Path, max_bytes: u64) {
    let (intake, thread) = Store::open(state, LIMIT, CommitMode::INDIVIDUAL)
        .unwrap()
        .spawn_joinable()
        .unwrap();
    let result = sealer::pass(
        state,
        &intake,
        Retention {
            max_age_s: u64::MAX,
            max_bytes,
        },
        1,
    );
    drop(intake);
    thread.join().unwrap();
    result.unwrap();
}
#[test]
fn straddling_publication_preserves_old_page_envelope_and_retention_expires_it() {
    let s = Scratch::new();
    let state = s.0.join("state");
    fs::create_dir_all(state.join("journal")).unwrap();
    let groups = [group(1, 100), group(2, 200), group(3, 900)];
    fs::write(s.0.join("producer-before-storage.jsonl"), ledger(&groups)).unwrap();
    let mut log = FrameLog::open(
        &state.join("journal"),
        LIMIT,
        segment::MAX_GROUP_PAYLOAD,
        |_, _| Ok(()),
    )
    .unwrap();
    for group in &groups[..2] {
        log.append(&group.encode_to_vec()).unwrap();
    }
    let q = json!({"kind":"logs","from_ns":0,"to_ns":1000,"limit":1});
    let histories = [
        History::with_plan(&state, Plan::Scan),
        History::with_plan(&state, Plan::Walk),
    ];
    let first: Vec<_> = histories
        .iter()
        .map(|h| {
            h.run(&serde_json::from_value(q.clone()).unwrap(), 2)
                .unwrap()
        })
        .collect();
    s.save("first-pages", &json!(first));
    let mut outcomes = Vec::new();
    for (i, h) in histories.iter().enumerate() {
        let pages = chain(h, q.clone(), 2, Some(first[i].clone()));
        outcomes.push((
            format!("tail-{i}"),
            oracle(&s, &format!("tail-{i}"), &groups[..2], &q, &pages),
        ));
    }
    log.append(&groups[2].encode_to_vec()).unwrap();
    for (i, h) in histories.iter().enumerate() {
        let pages = chain(h, q.clone(), 3, Some(first[i].clone()));
        outcomes.push((
            format!("append-{i}"),
            oracle(&s, &format!("append-{i}"), &groups[..2], &q, &pages),
        ));
    }
    log.rotate(1).unwrap();
    drop(log);
    pass(&state, u64::MAX);
    let manifests = segment::list(&state).unwrap();
    assert_eq!(manifests.len(), 1);
    assert_eq!(
        (manifests[0].1.first_group, manifests[0].1.last_group),
        (1, 3)
    );
    assert!(!fs::read_dir(state.join("journal")).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("sealed-")
    }));
    s.save("published-manifest", &json!(manifests[0].1));
    for (i, h) in histories.iter().enumerate() {
        for (stage, reader) in [
            ("published", h),
            (
                "restarted",
                &History::with_plan(&state, if i == 0 { Plan::Scan } else { Plan::Walk }),
            ),
        ] {
            let pages = chain(reader, q.clone(), 3, Some(first[i].clone()));
            s.save(
                &format!("{stage}-trace-{i}"),
                &json!({"expected_old_receive_max":200,"expected_old_freshness":200,"pages":pages}),
            );
            outcomes.push((
                format!("{stage}-{i}"),
                oracle(&s, &format!("{stage}-{i}"), &groups[..2], &q, &pages),
            ));
        }
        let pages = chain(h, q.clone(), 3, None);
        outcomes.push((
            format!("fresh-{i}"),
            oracle(&s, &format!("fresh-{i}"), &groups, &q, &pages),
        ));
        let mut omitted = pages.clone();
        omitted[0]["rows"].as_array_mut().unwrap().clear();
        let rejected = !oracle(&s, &format!("missing-control-{i}"), &groups, &q, &omitted);
        outcomes.push((format!("missing-control-{i}"), rejected));
    }
    pass(&state, 0);
    assert!(segment::list(&state).unwrap().is_empty());
    for (i, h) in histories.iter().enumerate() {
        let mut continuation = q.clone();
        continuation["page"] = first[i]["next_page"].clone();
        let result = h.run(&serde_json::from_value(continuation).unwrap(), 3);
        let gone = matches!(result, Err(QueryError::Gone));
        s.save(
            &format!("retention-{i}"),
            &json!({"gone":gone,"result":format!("{result:?}")}),
        );
        outcomes.push((format!("retention-{i}"), gone));
    }
    s.save("outcomes-before-assertion", &json!(outcomes));
    s.bound();
    assert!(
        outcomes.iter().all(|(_, passed)| *passed),
        "snapshot transition violated exact rows/envelope or lifecycle control: {outcomes:?}"
    );
}
