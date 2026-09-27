use fabric_o11y::alpha::journal::{Batch, Cursor, Journal};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use prost::Message;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
const MAX_BYTES: u64 = 2 * 1024 * 1024;
const ENCODED_CAP: u32 = 1024 * 1024;

struct OwnedRun {
    root: PathBuf,
    keep: bool,
}

impl OwnedRun {
    fn new() -> Result<Self, String> {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("data")
            .join(format!("length-matrix-{}-{id}", std::process::id()));
        fs::create_dir(&root).map_err(|e| format!("create {}: {e}", root.display()))?;
        Ok(Self { root, keep: false })
    }
}

impl Drop for OwnedRun {
    fn drop(&mut self) {
        if !self.keep {
            fs::remove_dir_all(&self.root).expect("remove only owned probe directory");
        }
    }
}

fn fixture(body: &str) -> Batch {
    let metrics = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![Metric {
                    name: "oracle.gauge".into(),
                    unit: "1".into(),
                    data: Some(metric::Data::Gauge(Gauge {
                        data_points: vec![NumberDataPoint {
                            time_unix_nano: 2_000,
                            value: Some(number_data_point::Value::AsInt(17)),
                            ..Default::default()
                        }],
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    let logs = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: vec![LogRecord {
                    time_unix_nano: 3_000,
                    observed_time_unix_nano: 4_000,
                    body: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(body.into())),
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    Batch {
        version: 1,
        node_id: vec![9; 16],
        generation: 99,
        sequence: 99,
        metrics,
        logs,
        cursors: vec![Cursor {
            path: "/oracle/source.log".into(),
            device: 10,
            inode: 20,
            offset: 30,
            skipping_oversize: false,
            prefix_len: 0,
            prefix_crc: 0,
        }],
        collection_gaps: vec![],
    }
}

fn files(dir: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, String> {
    fn visit(root: &Path, here: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) -> Result<(), String> {
        for entry in fs::read_dir(here).map_err(|e| format!("list {}: {e}", here.display()))? {
            let entry = entry.map_err(|e| format!("directory entry: {e}"))?;
            let path = entry.path();
            let kind = fs::symlink_metadata(&path)
                .map_err(|e| format!("metadata {}: {e}", path.display()))?
                .file_type();
            if kind.is_symlink() {
                return Err(format!("unexpected symlink in owned journal: {}", path.display()));
            }
            if kind.is_dir() {
                visit(root, &path, out)?;
            } else if kind.is_file() {
                let relative = path.strip_prefix(root).unwrap().to_path_buf();
                out.insert(relative, fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?);
            } else {
                return Err(format!("unexpected file type: {}", path.display()));
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    visit(dir, dir, &mut out)?;
    Ok(out)
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir(to).map_err(|e| format!("create copy {}: {e}", to.display()))?;
    for (relative, bytes) in files(from)? {
        let target = to.join(relative);
        fs::create_dir_all(target.parent().unwrap()).map_err(|e| format!("copy parent: {e}"))?;
        fs::write(&target, bytes).map_err(|e| format!("copy {}: {e}", target.display()))?;
    }
    Ok(())
}

fn sha256(path: &Path) -> Result<String, String> {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| format!("sha256sum launch: {e}"))?;
    if !output.status.success() {
        return Err(format!("sha256sum exit {}", output.status));
    }
    let text = String::from_utf8(output.stdout).map_err(|e| format!("sha256sum UTF-8: {e}"))?;
    let hash = text.split_whitespace().next().unwrap_or("");
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("sha256sum output has no 64-digit hash".into());
    }
    Ok(hash.into())
}

fn unique_position(haystack: &[u8], needle: &[u8], start: usize, end: usize) -> Result<usize, String> {
    let candidates: Vec<_> = (start..=end.saturating_sub(needle.len()))
        .filter(|&pos| haystack.get(pos..pos + needle.len()) == Some(needle))
        .collect();
    if candidates.len() != 1 {
        return Err(format!(
            "missing external seam: expected one exact encoding in observed append range, found {}",
            candidates.len()
        ));
    }
    Ok(candidates[0])
}

fn length_position(bytes: &[u8], start: usize, end: usize, encoded_len: usize) -> Result<usize, String> {
    let pattern = (encoded_len as u32).to_le_bytes();
    let mut candidates = Vec::new();
    if pattern.len() <= end.saturating_sub(start) {
        for pos in start..=end - pattern.len() {
            if bytes[pos..pos + pattern.len()] == pattern {
                candidates.push(pos);
            }
        }
    }
    if candidates.len() != 1 {
        return Err(format!(
            "missing external seam: expected one observed four-byte little-endian encoded-batch length in header, found {}",
            candidates.len()
        ));
    }
    Ok(candidates[0])
}

#[derive(Clone)]
struct Case {
    label: String,
    offset: usize,
    bit: u8,
    declared_len_after: Option<u32>,
    remaining_from_payload: Option<usize>,
}

fn replay_exact(journal: &mut Journal, expected: &[Batch], expected_next: u64, expected_identity: ([u8; 16], u64)) -> Result<(), String> {
    let identity = journal.identity();
    if identity != expected_identity {
        return Err(format!("identity changed: actual={identity:?}, expected={expected_identity:?}"));
    }
    let next = journal.next_sequence();
    if next != expected_next {
        return Err(format!("next_sequence={next}, expected={expected_next}"));
    }
    let mut visited = Vec::new();
    let reported = journal
        .replay(|batch| -> io::Result<()> {
            visited.push(batch);
            Ok(())
        })
        .map_err(|e| format!("replay after successful open rejected: {e}"))?;
    if reported != expected.len() || visited.len() != expected.len() {
        return Err(format!(
            "replay count={} visited={}, expected={}",
            reported, visited.len(), expected.len()
        ));
    }
    if visited.as_slice() != expected {
        let mismatch = visited.iter().zip(expected).position(|(a, b)| a != b).unwrap();
        return Err(format!("replay batch {mismatch} differs from exact acknowledged Batch"));
    }
    Ok(())
}

fn probe(run: &mut OwnedRun) -> Result<(usize, usize, usize), String> {
    let start_time = Instant::now();
    let source = run.root.join("source");
    fs::create_dir(&source).map_err(|e| format!("create source: {e}"))?;
    let mut journal = Journal::open(&source, MAX_BYTES).map_err(|e| format!("source open: {e}"))?;
    let before = files(&source)?;
    let first = journal.append(&fixture("oracle-first-body"))
        .map_err(|e| format!("first append: {e}"))?;
    let after_first = files(&source)?;
    let second = journal.append(&fixture("oracle-second-body"))
        .map_err(|e| format!("second append: {e}"))?;
    let after_second = files(&source)?;
    let expected_next = journal.next_sequence();
    let expected_identity = journal.identity();
    let expected = vec![first.clone(), second.clone()];
    if second.sequence != first.sequence + 1 {
        return Err(format!("clean source sequence gap: first={}, second={}", first.sequence, second.sequence));
    }
    drop(journal);
    let closed = files(&source)?;

    let candidates: Vec<_> = after_second
        .iter()
        .filter_map(|(path, second_bytes)| {
            let first_bytes = after_first.get(path)?;
            let initial_bytes = before.get(path).map(Vec::as_slice).unwrap_or(&[]);
            (initial_bytes.len() < first_bytes.len()
                && first_bytes.len() < second_bytes.len()
                && first_bytes.starts_with(initial_bytes)
                && second_bytes.starts_with(first_bytes))
            .then(|| path.clone())
        })
        .collect();
    if candidates.len() != 1 {
        return Err(format!(
            "missing external seam: {} files grew append-only at both appends",
            candidates.len()
        ));
    }
    let relative = &candidates[0];
    let b0 = before.get(relative).map(Vec::as_slice).unwrap_or(&[]);
    let b1 = &after_first[relative];
    let b2 = &after_second[relative];
    if closed.get(relative) != Some(b2) {
        return Err("missing external seam: append-bearing bytes changed on clean close".into());
    }
    let golden = run.root.join("golden");
    copy_tree(&source, &golden)?;
    let target = golden.join(relative);
    let source_size = fs::metadata(&target).map_err(|e| format!("golden metadata: {e}"))?.len();
    let source_hash = sha256(&target)?;

    // Clean positive reopen, both on the original and on an unmutated copy.
    let mut clean_source = Journal::open(&source, MAX_BYTES).map_err(|e| format!("clean source reopen: {e}"))?;
    replay_exact(&mut clean_source, &expected, expected_next, expected_identity)
        .map_err(|e| format!("clean source: {e}"))?;
    drop(clean_source);
    let clean_copy = run.root.join("clean-copy");
    copy_tree(&golden, &clean_copy)?;
    let mut clean = Journal::open(&clean_copy, MAX_BYTES).map_err(|e| format!("clean copied reopen: {e}"))?;
    replay_exact(&mut clean, &expected, expected_next, expected_identity)
        .map_err(|e| format!("clean copy: {e}"))?;
    drop(clean);
    fs::remove_dir_all(&clean_copy).map_err(|e| format!("clean copy cleanup: {e}"))?;

    let frames = [
        ("first", b0.len(), b1.len(), first.encode_to_vec(), b"oracle-first-body".as_slice()),
        ("second", b1.len(), b2.len(), second.encode_to_vec(), b"oracle-second-body".as_slice()),
    ];
    let mut cases = Vec::new();
    let mut oversized_remaining_but_below_cap = false;
    for (name, begin, end, encoded, body) in frames {
        let encoded_at = unique_position(b2, &encoded, begin, end)?;
        let encoded_end = encoded_at + encoded.len();
        let body_at = unique_position(b2, body, encoded_at, encoded_end)?;
        let length_at = length_position(b2, begin, encoded_at, encoded.len())?;
        let remaining_from_payload = b2.len() - encoded_at;
        for byte_index in 0..4 {
            for bit in 0..8 {
                let mut raw = [0u8; 4];
                raw.copy_from_slice(&b2[length_at..length_at + 4]);
                raw[byte_index] ^= 1u8 << bit;
                let declared = u32::from_le_bytes(raw);
                if declared as usize > remaining_from_payload && declared <= ENCODED_CAP {
                    oversized_remaining_but_below_cap = true;
                }
                cases.push(Case {
                    label: format!("{name}-length-byte-{byte_index}-bit-{bit}"),
                    offset: length_at + byte_index,
                    bit,
                    declared_len_after: Some(declared),
                    remaining_from_payload: Some(remaining_from_payload),
                });
            }
        }
        cases.push(Case { label: format!("{name}-payload"), offset: body_at + 7, bit: 0, declared_len_after: None, remaining_from_payload: None });
        cases.push(Case { label: format!("{name}-payload-end"), offset: body_at + body.len() - 2, bit: 0, declared_len_after: None, remaining_from_payload: None });
        if encoded_end < end {
            cases.push(Case { label: format!("{name}-trailer-start"), offset: encoded_end, bit: 0, declared_len_after: None, remaining_from_payload: None });
            cases.push(Case { label: format!("{name}-trailer-end"), offset: end - 1, bit: 0, declared_len_after: None, remaining_from_payload: None });
        } else {
            return Err(format!("{name}: missing external seam: no observed trailing bytes"));
        }
    }
    if !oversized_remaining_but_below_cap {
        return Err("missing discriminating boundary: no one-bit length mutation exceeds remaining file while staying within 1 MiB cap".into());
    }
    cases.sort_by_key(|case| (case.offset, case.bit));
    if cases.len() != 72 {
        return Err(format!("expected 64 length and 8 control cases, found {}", cases.len()));
    }

    let manifest_path = run.root.join("mutations.tsv");
    let mut manifest = File::create(&manifest_path).map_err(|e| format!("manifest create: {e}"))?;
    writeln!(manifest, "case\tlabel\tfile\tsource_size\tsource_sha256\toffset\tbit\tbefore\tafter\tdeclared_len_after\tremaining_file_from_payload")
        .map_err(|e| format!("manifest header: {e}"))?;
    let mut rejected = 0;
    let mut accepted = 0;
    for (index, case) in cases.iter().enumerate() {
        if start_time.elapsed() >= Duration::from_secs(110) {
            return Err(format!("incomplete: internal 110 s guard before case {index}"));
        }
        let case_dir = run.root.join(format!("case-{index:03}"));
        copy_tree(&golden, &case_dir)?;
        let live_bytes: u64 = files(&source)?.values().map(|v| v.len() as u64).sum::<u64>()
            + files(&golden)?.values().map(|v| v.len() as u64).sum::<u64>()
            + files(&case_dir)?.values().map(|v| v.len() as u64).sum::<u64>()
            + fs::metadata(&manifest_path).map_err(|e| format!("manifest metadata: {e}"))?.len();
        if live_bytes > 9 * 1024 * 1024 {
            return Err(format!("incomplete: live data {} exceeds 9 MiB guard", live_bytes));
        }
        let case_file = case_dir.join(relative);
        let original = b2[case.offset];
        let flipped = original ^ (1u8 << case.bit);
        if fs::read(&case_file).map_err(|e| format!("pre-mutation read: {e}"))? != *b2 {
            return Err("copy differs from exact golden journal before mutation".into());
        }
        let declared = case.declared_len_after.map_or(String::new(), |x| x.to_string());
        let remaining = case.remaining_from_payload.map_or(String::new(), |x| x.to_string());
        writeln!(manifest, "{index}\t{}\t{}\t{source_size}\t{source_hash}\t{}\t{}\t{original:02x}\t{flipped:02x}\t{declared}\t{remaining}", case.label, relative.display(), case.offset, case.bit)
            .map_err(|e| format!("manifest case: {e}"))?;
        manifest.sync_all().map_err(|e| format!("manifest sync: {e}"))?;
        let mut file = OpenOptions::new().write(true).open(&case_file).map_err(|e| format!("mutant open: {e}"))?;
        file.seek(SeekFrom::Start(case.offset as u64)).map_err(|e| format!("mutant seek: {e}"))?;
        file.write_all(&[flipped]).map_err(|e| format!("mutant byte write: {e}"))?;
        file.sync_all().map_err(|e| format!("mutant sync: {e}"))?;
        drop(file);
        let mutant = fs::read(&case_file).map_err(|e| format!("mutant read: {e}"))?;
        if mutant.len() != b2.len() || mutant.iter().zip(b2).filter(|(x, y)| x != y).count() != 1 {
            return Err(format!("case {index}: mutation changed other bytes"));
        }

        match Journal::open(&case_dir, MAX_BYTES) {
            Err(_) => rejected += 1, // Explicit corruption/recovery-required rejection is allowed.
            Ok(mut reopened) => {
                if let Err(reason) = replay_exact(&mut reopened, &expected, expected_next, expected_identity) {
                    return Err(format!(
                        "COUNTEREXAMPLE case={index} label={} offset={} bit={} before={original:02x} after={flipped:02x} declared_after={declared} remaining={remaining}: open accepted, {reason}; files={}",
                        case.label, case.offset, case.bit, case_dir.display()
                    ));
                }
                accepted += 1;
            }
        }
        fs::remove_dir_all(&case_dir).map_err(|e| format!("case cleanup: {e}"))?;
    }
    fs::remove_dir_all(&source).map_err(|e| format!("redundant source cleanup: {e}"))?;
    Ok((cases.len(), rejected, accepted))
}

#[test]
fn each_bit_of_each_observed_length_byte_preserves_acknowledged_replay_or_rejects() {
    let mut run = OwnedRun::new().unwrap();
    match probe(&mut run) {
        Ok((cases, rejected, accepted)) => {
            run.keep = true;
            println!(
                "clean exact replays and {cases} selected one-bit mutations passed (rejected={rejected}, accepted_exact={accepted}); evidence={}",
                run.root.display()
            );
        }
        Err(finding) => {
            run.keep = true;
            panic!("{finding}; retained={}", run.root.display());
        }
    }
}
