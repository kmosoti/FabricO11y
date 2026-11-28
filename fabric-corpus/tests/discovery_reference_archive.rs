//! Opt-in synthetic archive/replay test. Scripted choices are not learning claims.
use fabric_corpus::{
    Corpus, Kind, RevisionBody,
    as_of::AsOfView,
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
    let mut texts = Vec::new();
    for i in 0..2 {
        let expected = parse_hash(&env::var(format!("FABRIC_REFERENCE_HASH_{i}")).unwrap());
        let view = read_verified(&dir.join(format!("{i}.bin")), expected);
        texts.push(archived_text(&view).to_owned());
    }
    assert_eq!(texts, ["room", "full"]);
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
    for mode in ["fixed", "aware"] {
        let mut command = Command::new(env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg("archive_worker")
            .arg("--ignored")
            .arg("--nocapture")
            .env("FABRIC_REFERENCE_DIR", &dir.0)
            .env("FABRIC_REFERENCE_MODE", mode)
            .env("FABRIC_REFERENCE_HASH_0", hex(&snapshots[0].1))
            .env("FABRIC_REFERENCE_HASH_1", hex(&snapshots[1].1));
        let (status, stdout, stderr) = bounded_worker(command);
        assert!(
            status.success(),
            "worker {mode}: {}",
            String::from_utf8_lossy(&stderr)
        );
        let stdout = String::from_utf8(stdout).unwrap();
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
            "mode={mode} attempts={ATTEMPTS} accepted={accepted} refused={} archive_snapshots=2",
            ATTEMPTS - accepted
        );
    }
    assert_eq!(results, [0, 2]);
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
