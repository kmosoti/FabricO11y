//! Opt-in synthetic archive/replay test. Scripted choices are not learning claims.
use fabric_corpus::{
    Corpus, Kind, RevisionBody,
    as_of::AsOfView,
    differential::Applicability,
    handoff::{Continuation, Declaration, Handoff, Marker},
    portable::{Availability, AvailabilityClaim, PortableView},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const ATTEMPTS: usize = 4;
const MAX_ARCHIVE: usize = 64 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Window {
    accepted: u8,
    confirmed: u8,
    slots: u8,
}
fn verifier(w: Window) -> bool {
    let a = u16::from(w.accepted);
    let c = u16::from(w.confirmed);
    let s = u16::from(w.slots);
    c <= a && a - c <= s && a + 1 - c <= s
}
// Independent explicit table for the two fixture windows.
fn oracle(w: Window) -> Option<bool> {
    match (w.accepted, w.confirmed, w.slots) {
        (0, 0, 1) => Some(true),
        (1, 0, 1) => Some(false),
        _ => None,
    }
}
// The verifier binds the worker's reported state to the independently imported indexed record.
fn row_valid(fields: &[u8], n: usize, mode: &str, expected: &[Window; 2]) -> bool {
    if fields.len() != 6 || usize::from(fields[0]) != n {
        return false;
    }
    let index = if mode == "fixed" { 1 } else { n % 2 };
    if usize::from(fields[1]) != index {
        return false;
    }
    let reported = Window {
        accepted: fields[2],
        confirmed: fields[3],
        slots: fields[4],
    };
    reported == expected[index] && oracle(reported) == Some(fields[5] == 1) && fields[5] <= 1
}
fn bounded_worker(mut cmd: Command) -> (ExitStatus, Vec<u8>, Vec<u8>) {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let read = |stream: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            stream.take(4097).read_to_end(&mut bytes).unwrap();
            bytes
        })
    };
    let out = read(Box::new(stdout));
    let err = read(Box::new(stderr));
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            child.wait().unwrap();
            panic!("worker timed out and was reaped");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let out = out.join().unwrap();
    let err = err.join().unwrap();
    assert!(
        out.len() <= 4096 && err.len() <= 4096,
        "worker output exceeded cap"
    );
    (status, out, err)
}
#[test]
fn rejects_wrong_index_or_unmatched_window() {
    let expected = [
        Window {
            accepted: 0,
            confirmed: 0,
            slots: 1,
        },
        Window {
            accepted: 1,
            confirmed: 0,
            slots: 1,
        },
    ];
    assert!(row_valid(&[0, 0, 0, 0, 1, 1], 0, "aware", &expected));
    assert!(!row_valid(&[0, 1, 0, 0, 1, 1], 0, "aware", &expected));
    assert!(!row_valid(&[0, 0, 1, 0, 1, 0], 0, "aware", &expected));
    assert!(!row_valid(&[1, 1, 1, 0, 1, 0], 0, "aware", &expected));
}
fn records(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix("record "))
        .collect()
}
#[test]
fn framing_preserves_first_record_after_libtest_prefix() {
    let transcript = "test archive_worker ... \nrecord 0 1 1 0 1 0\nrecord 1 1 1 0 1 0\nrecord 2 1 1 0 1 0\nrecord 3 1 1 0 1 0\nok\n";
    let rows = records(transcript);
    assert_eq!(rows.len(), ATTEMPTS);
    assert_eq!(rows[0], "0 1 1 0 1 0");
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|v| format!("{v:02x}")).collect()
}
fn hash(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}
fn parse_hash(s: &str) -> [u8; 32] {
    assert_eq!(s.len(), 64);
    let mut h = [0; 32];
    for (i, slot) in h.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
    }
    h
}
fn archive_snapshot(text: &str) -> (Vec<u8>, [u8; 32]) {
    let mut c = Corpus::default();
    let root = c
        .admit(RevisionBody {
            object_id: text.into(),
            kind: Kind::Observation,
            text: text.into(),
            parents: BTreeSet::new(),
            sources: vec![],
            extensions: BTreeMap::new(),
        })
        .unwrap();
    let view =
        AsOfView::capture(&c, BTreeSet::from([root]), BTreeSet::new(), BTreeSet::new()).unwrap();
    PortableView::new(
        view,
        BTreeMap::from([(
            root,
            AvailabilityClaim {
                archive: Availability::Exact,
                current: Availability::Exact,
            },
        )]),
    )
    .unwrap()
    .export()
    .unwrap()
}
fn changed_snapshot(bytes: &[u8], digest: [u8; 32], missing: bool) -> (Vec<u8>, [u8; 32]) {
    let original = PortableView::import(bytes, digest).unwrap();
    let root = *original.view().revisions().keys().next().unwrap();
    let view = if missing {
        AsOfView::capture(
            &Corpus::default(),
            BTreeSet::from([root]),
            BTreeSet::new(),
            BTreeSet::new(),
        )
        .unwrap()
    } else {
        original.view().clone()
    };
    let claims = BTreeMap::from([(
        root,
        AvailabilityClaim {
            archive: Availability::Exact,
            current: Availability::Missing,
        },
    )]);
    PortableView::new(view, claims).unwrap().export().unwrap()
}
fn bounded_bytes(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .unwrap()
        .take((MAX_ARCHIVE + 1) as u64)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(bytes.len() <= MAX_ARCHIVE);
    bytes
}
fn write_exact(dir: &Path, name: &str, bytes: &[u8]) {
    assert!(bytes.len() <= MAX_ARCHIVE);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(name))
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    let mut permissions = file.metadata().unwrap().permissions();
    permissions.set_readonly(true);
    file.set_permissions(permissions).unwrap();
}
fn read_verified(path: &Path, expected: [u8; 32]) -> PortableView {
    let metadata = fs::metadata(path).unwrap();
    assert!(metadata.len() <= MAX_ARCHIVE as u64);
    let bytes = fs::read(path).unwrap();
    assert!(bytes.len() <= MAX_ARCHIVE);
    PortableView::import(&bytes, expected).unwrap()
}
fn archived_text(view: &PortableView) -> &str {
    let mut revisions = view.view().revisions().values();
    let body = revisions.next().unwrap().as_ref().unwrap();
    assert!(revisions.next().is_none());
    &body.text
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct ReadRecord {
    root: String,
    text: String,
    view: String,
}
fn portable_record(view: &PortableView, expected_view: [u8; 32]) -> ReadRecord {
    let mut entries = view.view().revisions().iter();
    let (root, body) = entries.next().unwrap();
    assert!(entries.next().is_none());
    let body = body.as_ref().unwrap();
    assert_eq!(body.kind, Kind::Observation);
    assert!(body.sources.is_empty() && body.parents.is_empty() && body.extensions.is_empty());
    assert!(view.view().relations().is_empty() && view.view().groups().is_empty());
    let availability = view.availability().get(root).unwrap();
    assert_eq!(
        (availability.archive, availability.current),
        (Availability::Exact, Availability::Exact)
    );
    ReadRecord {
        root: hex(root),
        text: body.text.clone(),
        view: hex(&expected_view),
    }
}
fn jsonl_record(record: &ReadRecord) -> Vec<u8> {
    let mut out=serde_json::to_vec(&serde_json::json!({"version":1,"root":record.root,"text":record.text,"view":record.view,"kind":"observation","sources":0,"archive":"exact","current":"exact"})).unwrap();
    out.push(b'\n');
    out
}
fn read_jsonl(
    bytes: &[u8],
    expected_digest: [u8; 32],
    expected_root: &str,
    expected_view: &str,
) -> Result<ReadRecord, &'static str> {
    if bytes.len() > MAX_ARCHIVE || bytes.last() != Some(&b'\n') || hash(bytes) != expected_digest {
        return Err("bytes");
    }
    let content = std::str::from_utf8(bytes).map_err(|_| "utf8")?;
    let mut lines = content.lines();
    let line = lines.next().ok_or("missing")?;
    if lines.next().is_some() {
        return Err("duplicate");
    }
    let value: serde_json::Value = serde_json::from_str(line).map_err(|_| "json")?;
    let object = value.as_object().ok_or("shape")?;
    if object.len() != 8
        || value["version"] != 1
        || value["root"] != expected_root
        || value["view"] != expected_view
        || value["kind"] != "observation"
        || value["sources"] != 0
        || value["archive"] != "exact"
        || value["current"] != "exact"
    {
        return Err("identity");
    }
    let text = value["text"].as_str().ok_or("text")?.to_owned();
    Ok(ReadRecord {
        root: expected_root.to_owned(),
        text,
        view: expected_view.to_owned(),
    })
}
struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn archive_dir() -> OwnedDir {
    let base = PathBuf::from(env::var_os("TMPDIR").expect("disk-backed TMPDIR required"));
    let base = base.canonicalize().unwrap();
    assert!(base.starts_with("/data/tmp"));
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = base.join(format!("corpus-reference-{}-{id}", std::process::id()));
    fs::create_dir(&path).unwrap();
    OwnedDir(path)
}
#[test]
#[ignore = "subprocess worker, invoked only by the opt-in archive test"]
fn archive_worker() {
    let dir = PathBuf::from(env::var_os("FABRIC_REFERENCE_DIR").unwrap());
    let mode = env::var("FABRIC_REFERENCE_MODE").unwrap();
    assert!(mode == "fixed" || mode == "aware");
    let storage = env::var("FABRIC_REFERENCE_STORAGE").unwrap();
    assert!(storage == "portable" || storage == "jsonl");
    let mut texts = Vec::new();
    for i in 0..2 {
        let expected = parse_hash(&env::var(format!("FABRIC_REFERENCE_HASH_{i}")).unwrap());
        let root = env::var(format!("FABRIC_REFERENCE_ROOT_{i}")).unwrap();
        let record = if storage == "portable" {
            let view = read_verified(&dir.join(format!("{i}.bin")), expected);
            let record = portable_record(&view, expected);
            assert_eq!(record.root, root);
            record
        } else {
            let jsonl = bounded_bytes(&dir.join(format!("{i}.jsonl")));
            let digest = parse_hash(&env::var(format!("FABRIC_JSONL_HASH_{i}")).unwrap());
            read_jsonl(&jsonl, digest, &root, &hex(&expected)).unwrap()
        };
        texts.push(record.text);
    }
    assert_eq!(texts, ["room", "full"]);
    let original = bounded_bytes(&dir.join("0.bin"));
    let basis = parse_hash(&env::var("FABRIC_REFERENCE_HASH_0").unwrap());
    let root = parse_hash(&env::var("FABRIC_HANDOFF_ROOT").unwrap());
    let policy = parse_hash(&env::var("FABRIC_HANDOFF_POLICY").unwrap());
    let expected_handoff = parse_hash(&env::var("FABRIC_HANDOFF_DIGEST").unwrap());
    let handoff_bytes = bounded_bytes(&dir.join("handoff.bin"));
    let handoff = Handoff::import(
        &handoff_bytes,
        expected_handoff,
        &original,
        root,
        policy,
        basis,
    )
    .unwrap();
    assert_eq!(
        (
            handoff.declaration().evidence_omission,
            handoff.declaration().dissent,
            handoff.declaration().continuation
        ),
        (Marker::Omitted, Marker::Unknown, Continuation::Unresolved)
    );
    assert!(
        Handoff::import(
            &handoff_bytes,
            expected_handoff,
            &original,
            root,
            [0; 32],
            basis
        )
        .is_err()
    );
    assert!(
        Handoff::import(
            &handoff_bytes,
            expected_handoff,
            &original,
            root,
            policy,
            [0; 32]
        )
        .is_err()
    );
    assert!(
        Handoff::import(
            &handoff_bytes,
            expected_handoff,
            &original,
            [0; 32],
            policy,
            basis
        )
        .is_err()
    );
    let mut altered = handoff_bytes.clone();
    altered[0] ^= 1;
    assert!(Handoff::import(&altered, expected_handoff, &original, root, policy, basis).is_err());
    let changed = bounded_bytes(&dir.join("changed.bin"));
    let missing = bounded_bytes(&dir.join("missing.bin"));
    let changed_digest = parse_hash(&env::var("FABRIC_CHANGED_DIGEST").unwrap());
    let missing_digest = parse_hash(&env::var("FABRIC_MISSING_DIGEST").unwrap());
    assert_eq!(
        handoff.compare_current(&original, &changed, changed_digest, root, policy),
        Ok(Applicability::Stale)
    );
    assert_eq!(
        handoff.compare_current(&original, &missing, missing_digest, root, policy),
        Ok(Applicability::Unknown)
    );
    assert_eq!(handoff.bytes(), handoff_bytes);
    assert_eq!(handoff.digest(), expected_handoff);
    println!("\nHANDOFF_REPLAY_OK");

    // libtest may print its progress prefix without a terminating newline.
    println!();
    for n in 0..ATTEMPTS {
        let index = if mode == "fixed" { 1 } else { n % 2 };
        let before = if texts[index] == "room" {
            Window {
                accepted: 0,
                confirmed: 0,
                slots: 1,
            }
        } else {
            Window {
                accepted: 1,
                confirmed: 0,
                slots: 1,
            }
        };
        println!(
            "record {n} {index} {} {} {} {}",
            before.accepted,
            before.confirmed,
            before.slots,
            u8::from(verifier(before))
        );
    }
}
#[test]
#[ignore = "requires disk-backed TMPDIR and fresh subprocesses"]
fn scripted_consumers_replay_same_verified_portable_archive() {
    let dir = archive_dir();
    let snapshots = [archive_snapshot("room"), archive_snapshot("full")];
    for (i, (bytes, digest)) in snapshots.iter().enumerate() {
        assert!(bytes.len() <= MAX_ARCHIVE);
        assert_eq!(hash(bytes), *digest);
        let path = dir.0.join(format!("{i}.bin"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        let mut permissions = file.metadata().unwrap().permissions();
        permissions.set_readonly(true);
        file.set_permissions(permissions).unwrap();
    }
    let archive_records: Vec<_> = snapshots
        .iter()
        .map(|(bytes, digest)| {
            portable_record(&PortableView::import(bytes, *digest).unwrap(), *digest)
        })
        .collect();
    let jsonl: Vec<_> = archive_records.iter().map(jsonl_record).collect();
    for i in 0..2 {
        write_exact(&dir.0, &format!("{i}.jsonl"), &jsonl[i]);
        assert_eq!(
            read_jsonl(
                &jsonl[i],
                hash(&jsonl[i]),
                &archive_records[i].root,
                &archive_records[i].view
            ),
            Ok(archive_records[i].clone())
        );
    }
    let mut corrupted = jsonl[0].clone();
    corrupted[0] ^= 1;
    assert!(
        read_jsonl(
            &corrupted,
            hash(&jsonl[0]),
            &archive_records[0].root,
            &archive_records[0].view
        )
        .is_err()
    );
    assert!(
        read_jsonl(
            b"",
            hash(b""),
            &archive_records[0].root,
            &archive_records[0].view
        )
        .is_err()
    );
    let truncated = &jsonl[0][..jsonl[0].len() - 1];
    assert!(
        read_jsonl(
            truncated,
            hash(truncated),
            &archive_records[0].root,
            &archive_records[0].view
        )
        .is_err()
    );
    let duplicate = [jsonl[0].as_slice(), jsonl[0].as_slice()].concat();
    assert!(
        read_jsonl(
            &duplicate,
            hash(&duplicate),
            &archive_records[0].root,
            &archive_records[0].view
        )
        .is_err()
    );
    assert!(
        read_jsonl(
            &jsonl[0],
            hash(&jsonl[0]),
            &archive_records[1].root,
            &archive_records[0].view
        )
        .is_err()
    );
    assert!(
        read_jsonl(
            &jsonl[0],
            hash(&jsonl[0]),
            &archive_records[0].root,
            &archive_records[1].view
        )
        .is_err()
    );
    let basis = PortableView::import(&snapshots[0].0, snapshots[0].1).unwrap();
    let root = *basis.view().revisions().keys().next().unwrap();
    let policy = [3; 32];
    let declaration = Declaration {
        basis_digest: snapshots[0].1,
        root,
        policy_id: policy,
        revisions: BTreeSet::from([root]),
        relations: BTreeSet::new(),
        citations: vec![],
        evidence_omission: Marker::Omitted,
        dissent: Marker::Unknown,
        continuation: Continuation::Unresolved,
    };
    let handoff =
        Handoff::create(declaration, &snapshots[0].0, root, policy, snapshots[0].1).unwrap();
    let changed = changed_snapshot(&snapshots[0].0, snapshots[0].1, false);
    let missing = changed_snapshot(&snapshots[0].0, snapshots[0].1, true);
    write_exact(&dir.0, "handoff.bin", handoff.bytes());
    write_exact(&dir.0, "changed.bin", &changed.0);
    write_exact(&dir.0, "missing.bin", &missing.0);
    let expected = std::array::from_fn(|i| {
        let view = PortableView::import(&snapshots[i].0, snapshots[i].1).unwrap();
        match archived_text(&view) {
            "room" => Window {
                accepted: 0,
                confirmed: 0,
                slots: 1,
            },
            "full" => Window {
                accepted: 1,
                confirmed: 0,
                slots: 1,
            },
            _ => panic!("unexpected independently imported record"),
        }
    });
    let mut results = Vec::new();
    for storage in ["portable", "jsonl"] {
        for mode in ["fixed", "aware"] {
            let mut command = Command::new(env::current_exe().unwrap());
            command
                .arg("--exact")
                .arg("archive_worker")
                .arg("--ignored")
                .arg("--nocapture")
                .env("FABRIC_REFERENCE_DIR", &dir.0)
                .env("FABRIC_REFERENCE_MODE", mode)
                .env("FABRIC_REFERENCE_STORAGE", storage)
                .env("FABRIC_REFERENCE_ROOT_0", &archive_records[0].root)
                .env("FABRIC_REFERENCE_ROOT_1", &archive_records[1].root)
                .env("FABRIC_JSONL_HASH_0", hex(&hash(&jsonl[0])))
                .env("FABRIC_JSONL_HASH_1", hex(&hash(&jsonl[1])))
                .env("FABRIC_REFERENCE_HASH_0", hex(&snapshots[0].1))
                .env("FABRIC_REFERENCE_HASH_1", hex(&snapshots[1].1))
                .env("FABRIC_HANDOFF_ROOT", hex(&root))
                .env("FABRIC_HANDOFF_POLICY", hex(&policy))
                .env("FABRIC_HANDOFF_DIGEST", hex(&handoff.digest()))
                .env("FABRIC_CHANGED_DIGEST", hex(&changed.1))
                .env("FABRIC_MISSING_DIGEST", hex(&missing.1));
            let (status, stdout, stderr) = bounded_worker(command);
            assert!(
                status.success(),
                "worker {mode}: {}",
                String::from_utf8_lossy(&stderr)
            );
            let stdout = String::from_utf8(stdout).unwrap();
            assert_eq!(
                stdout
                    .lines()
                    .filter(|line| *line == "HANDOFF_REPLAY_OK")
                    .count(),
                1
            );
            let rows = records(&stdout);
            assert_eq!(rows.len(), ATTEMPTS);
            let mut accepted = 0;
            for (n, row) in rows.iter().enumerate() {
                let fields: Vec<u8> = row.split_whitespace().map(|x| x.parse().unwrap()).collect();
                assert!(
                    row_valid(&fields, n, mode, &expected),
                    "unmatched archive row {row}"
                );
                accepted += usize::from(fields[5]);
            }
            results.push(accepted);
            println!(
                "storage={storage} mode={mode} attempts={ATTEMPTS} accepted={accepted} refused={} archive_snapshots=2",
                ATTEMPTS - accepted
            );
        }
    }
    assert_eq!(results, [0, 2, 0, 2]);
    // Both consumers used identical immutable bytes and independently held digests.
    for (i, (bytes, digest)) in snapshots.iter().enumerate() {
        let stored = fs::read(dir.0.join(format!("{i}.bin"))).unwrap();
        assert_eq!(&stored, bytes);
        assert_eq!(hash(&stored), *digest);
        assert_eq!(
            read_verified(&dir.0.join(format!("{i}.bin")), *digest),
            read_verified(&dir.0.join(format!("{i}.bin")), *digest)
        );
    }
    let mut tampered = snapshots[0].0.clone();
    tampered[0] ^= 1;
    assert!(PortableView::import(&tampered, snapshots[0].1).is_err());
    assert!(PortableView::import(&snapshots[0].0, snapshots[1].1).is_err());
    assert!(verifier(Window {
        accepted: 0,
        confirmed: 0,
        slots: 1
    }));
    assert!(!verifier(Window {
        accepted: 1,
        confirmed: 0,
        slots: 1
    }));
}
