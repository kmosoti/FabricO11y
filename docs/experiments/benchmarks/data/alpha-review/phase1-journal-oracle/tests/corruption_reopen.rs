use fabric_o11y::alpha::journal::{Batch, Cursor, Journal};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use prost::Message;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
const MAX_BYTES: u64 = 64 * 1024;

struct OwnedRun {
    root: PathBuf,
    keep: bool,
}

impl OwnedRun {
    fn new() -> Result<Self, String> {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("data")
            .join(format!("corruption-{}-{id}", std::process::id()));
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
    let mut candidates = BTreeSet::new();
    for pattern in [(encoded_len as u32).to_le_bytes().to_vec(), (encoded_len as u64).to_le_bytes().to_vec()] {
        if pattern.len() <= end.saturating_sub(start) {
            for pos in start..=end - pattern.len() {
                if bytes[pos..pos + pattern.len()] == pattern {
                    candidates.insert(pos);
                }
            }
        }
    }
    if candidates.len() != 1 {
        return Err(format!(
            "missing external seam: expected one observed little-endian encoded-batch length in header, found {}",
            candidates.len()
        ));
    }
    Ok(*candidates.iter().next().unwrap())
}

#[derive(Clone)]
struct Case {
    label: &'static str,
    offset: usize,
    content_byte: bool,
}

fn probe(run: &mut OwnedRun) -> Result<usize, String> {
    let start_time = Instant::now();
    let source = run.root.join("source");
    fs::create_dir(&source).map_err(|e| format!("create source: {e}"))?;
    let mut journal = Journal::open(&source, MAX_BYTES).map_err(|e| format!("source open: {e}"))?;
    let before = files(&source)?;
    let first = journal.append(fixture("oracle-first-body"))
        .map_err(|e| format!("first append: {e}"))?;
    let after_first = files(&source)?;
    let second = journal.append(fixture("oracle-second-body"))
        .map_err(|e| format!("second append: {e}"))?;
    let after_second = files(&source)?;
    let expected_next = journal.next_sequence();
    assert_eq!(second.sequence, first.sequence + 1);
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
    let clean_source = Journal::open(&source, MAX_BYTES).map_err(|e| format!("clean source reopen: {e}"))?;
    if clean_source.next_sequence() != expected_next {
        return Err(format!("clean source next_sequence={}, expected={expected_next}", clean_source.next_sequence()));
    }
    drop(clean_source);
    let clean_copy = run.root.join("clean-copy");
    copy_tree(&golden, &clean_copy)?;
    let clean = Journal::open(&clean_copy, MAX_BYTES).map_err(|e| format!("clean copied reopen: {e}"))?;
    if clean.next_sequence() != expected_next {
        return Err(format!("clean copy next_sequence={}, expected={expected_next}", clean.next_sequence()));
    }
    drop(clean);
    fs::remove_dir_all(&clean_copy).map_err(|e| format!("clean copy cleanup: {e}"))?;

    let frames = [
        ("first", b0.len(), b1.len(), first.encode_to_vec(), b"oracle-first-body".as_slice()),
        ("second", b1.len(), b2.len(), second.encode_to_vec(), b"oracle-second-body".as_slice()),
    ];
    let mut cases = Vec::new();
    let mut missing_seams = Vec::new();
    for (name, begin, end, encoded, body) in frames {
        let encoded_at = unique_position(b2, &encoded, begin, end)?;
        let encoded_end = encoded_at + encoded.len();
        let body_at = unique_position(b2, body, encoded_at, encoded_end)?;
        cases.push(Case { label: if name == "first" { "first-payload" } else { "second-payload" }, offset: body_at + 7, content_byte: true });
        cases.push(Case { label: if name == "first" { "first-payload-end" } else { "second-payload-end" }, offset: body_at + body.len() - 2, content_byte: true });
        match length_position(b2, begin, encoded_at, encoded.len()) {
            Ok(offset) => cases.push(Case { label: if name == "first" { "first-header-length" } else { "second-header-length" }, offset, content_byte: false }),
            Err(e) => missing_seams.push(format!("{name}: {e}")),
        }
        if encoded_end < end {
            cases.push(Case { label: if name == "first" { "first-trailer-start" } else { "second-trailer-start" }, offset: encoded_end, content_byte: false });
            cases.push(Case { label: if name == "first" { "first-trailer-end" } else { "second-trailer-end" }, offset: end - 1, content_byte: false });
        } else {
            missing_seams.push(format!("{name}: missing external seam: no observed trailing bytes"));
        }
    }
    cases.sort_by_key(|case| case.offset);
    cases.dedup_by_key(|case| case.offset);
    if cases.len() > 128 {
        return Err(format!("case count {} exceeds 128", cases.len()));
    }

    let manifest_path = run.root.join("mutations.tsv");
    let mut manifest = File::create(&manifest_path).map_err(|e| format!("manifest create: {e}"))?;
    writeln!(manifest, "case\tlabel\tfile\tsource_size\tsource_sha256\toffset\tbefore\tafter")
        .map_err(|e| format!("manifest header: {e}"))?;
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
        let flipped = original ^ 0x01;
        if fs::read(&case_file).map_err(|e| format!("pre-mutation read: {e}"))? != *b2 {
            return Err("copy differs from exact golden journal before mutation".into());
        }
        writeln!(manifest, "{index}\t{}\t{}\t{source_size}\t{source_hash}\t{}\t{original:02x}\t{flipped:02x}", case.label, relative.display(), case.offset)
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
            Err(_) => {} // Explicit rejection is allowed.
            Ok(reopened) => {
                let actual = reopened.next_sequence();
                drop(reopened);
                if actual != expected_next {
                    return Err(format!(
                        "COUNTEREXAMPLE case={index} label={} offset={} original={original:02x} flipped={flipped:02x}: open succeeded with next_sequence={actual}, expected={expected_next}; files={}",
                        case.label, case.offset, case_dir.display()
                    ));
                }
                if case.content_byte {
                    let after_open = fs::read(&case_file).map_err(|e| format!("post-open read: {e}"))?;
                    if after_open.get(case.offset) != Some(&original) {
                        return Err(format!(
                            "COUNTEREXAMPLE case={index} label={} offset={} original={original:02x} flipped={flipped:02x}: open accepted changed OTLP body byte without restoring exact committed bytes; files={}",
                            case.label, case.offset, case_dir.display()
                        ));
                    }
                }
            }
        }
        fs::remove_dir_all(&case_dir).map_err(|e| format!("case cleanup: {e}"))?;
    }
    if !missing_seams.is_empty() {
        return Err(format!("{} selected cases ran; {}", cases.len(), missing_seams.join("; ")));
    }
    fs::remove_dir_all(&source).map_err(|e| format!("redundant source cleanup: {e}"))?;
    Ok(cases.len())
}

#[test]
fn single_bit_corruption_never_silently_loses_acknowledged_batch() {
    let mut run = OwnedRun::new().unwrap();
    match probe(&mut run) {
        Ok(cases) => {
            run.keep = true;
            println!(
                "clean positive reopens and {cases} selected one-bit mutations passed; evidence={}",
                run.root.display()
            );
        }
        Err(finding) => {
            run.keep = true;
            panic!("{finding}; retained={}", run.root.display());
        }
    }
}
