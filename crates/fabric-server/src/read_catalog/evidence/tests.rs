use super::*;
use arrow_array::{
    BinaryArray, FixedSizeBinaryArray, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use fabric_frame::envelope::Batch;
use parquet::arrow::ArrowWriter;
use prost::Message;
use sha2::{Digest, Sha256};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(std::env::var("FABRIC_SCRATCH_ROOT").unwrap()).join(format!(
            "scoped-evidence-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        Self { root }
    }
    fn raw(
        &self,
        name: &str,
        records: &[(u64, String, Vec<u8>)],
        bad_digest: bool,
    ) -> (PathBuf, segment::Manifest) {
        let dir = self.root.join(name);
        std::fs::create_dir(&dir).unwrap();
        let digest: Vec<[u8; 32]> = records
            .iter()
            .map(|(_, _, v)| {
                if bad_digest {
                    [0; 32]
                } else {
                    Sha256::digest(v).into()
                }
            })
            .collect();
        let batch = RecordBatch::try_new(
            segment::batches_schema(),
            vec![
                Arc::new(UInt64Array::from_iter_values(records.iter().map(|r| r.0))),
                Arc::new(StringArray::from_iter_values(
                    records.iter().map(|r| r.1.as_str()),
                )),
                Arc::new(Int64Array::from_iter_values(
                    records.iter().map(|r| r.0 as i64 * 100),
                )),
                Arc::new(FixedSizeBinaryArray::try_from_iter(digest.iter()).unwrap()),
                Arc::new(BinaryArray::from_iter_values(
                    records.iter().map(|r| r.2.as_slice()),
                )),
            ],
        )
        .unwrap();
        let path = dir.join("batches.parquet");
        let mut writer =
            ArrowWriter::try_new(std::fs::File::create(&path).unwrap(), batch.schema(), None)
                .unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        let manifest = segment::Manifest {
            version: 1,
            journal_label: 1,
            first_group: 1,
            last_group: 3,
            records: records.len() as u64,
            received_min_ns: 100,
            received_max_ns: 300,
            freshness: BTreeMap::new(),
            files: BTreeMap::from([(
                "batches.parquet".into(),
                segment::FileEntry {
                    sha256: "fixture digest identity".into(),
                    bytes: path.metadata().unwrap().len(),
                    rows: records.len() as u64,
                },
            )]),
        };
        (dir, manifest)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }
}
fn record(logs: Vec<u8>) -> Vec<u8> {
    Batch {
        version: 1,
        node_id: vec![1; 16],
        generation: 1,
        sequence: 1,
        logs,
        ..Default::default()
    }
    .encode_to_vec()
}
fn full() -> Snapshot {
    Snapshot {
        oldest_group: 1,
        newest_group: 3,
    }
}

#[test]
fn invalid_excluded_record_never_poisoned_scope_or_populates_cache() {
    let f = Fixture::new();
    let (dir, m) = f.raw(
        "typed",
        &[
            (1, "allowed".into(), record(vec![])),
            (2, "excluded".into(), record(vec![0xff])),
        ],
        false,
    );
    let mut cache = Cache::default();
    let allowed = HashSet::from(["allowed".into()]);
    assert_eq!(
        cache
            .select(&dir, &m, full(), segment::Table::Logs, &allowed, &None)
            .unwrap(),
        selected_empty()
    );
    assert!(cache.entries.is_empty());
    let excluded = HashSet::from(["excluded".into()]);
    assert!(
        cache
            .select(&dir, &m, full(), segment::Table::Logs, &excluded, &None)
            .is_err()
    );
    // The exact scanner validates excluded raw digests before visitor filtering.
    let (dir, m) = f.raw("digest", &[(1, "excluded".into(), record(vec![]))], true);
    assert!(
        cache
            .select(&dir, &m, full(), segment::Table::Logs, &allowed, &None)
            .is_err()
    );
}

#[test]
fn cached_identity_changes_and_partial_snapshots_force_exact_scans() {
    let f = Fixture::new();
    let (dir, mut m) = f.raw("data", &[(1, "n".into(), record(vec![]))], false);
    let mut cache = Cache::default();
    let nodes = HashSet::from(["n".into()]);
    for _ in 0..2 {
        cache
            .select(&dir, &m, full(), segment::Table::Logs, &nodes, &None)
            .unwrap();
    }
    assert_eq!(cache.scans, 1);
    cache
        .select(
            &dir,
            &m,
            Snapshot {
                oldest_group: 1,
                newest_group: 2,
            },
            segment::Table::Logs,
            &nodes,
            &None,
        )
        .unwrap();
    assert_eq!(cache.scans, 2);
    m.files.get_mut("batches.parquet").unwrap().sha256.push('x');
    cache
        .select(&dir, &m, full(), segment::Table::Logs, &nodes, &None)
        .unwrap();
    assert_eq!(cache.scans, 3);
    let path = dir.join("batches.parquet");
    std::fs::copy(&path, dir.join("replacement")).unwrap();
    std::fs::rename(dir.join("replacement"), &path).unwrap();
    cache
        .select(&dir, &m, full(), segment::Table::Logs, &nodes, &None)
        .unwrap();
    assert_eq!(cache.scans, 4);
    std::fs::write(&path, b"corrupt").unwrap();
    assert!(
        cache
            .select(&dir, &m, full(), segment::Table::Logs, &nodes, &None)
            .is_err()
    );
    assert!(cache.entries.is_empty());
    std::fs::remove_file(&path).unwrap();
    assert!(
        cache
            .select(&dir, &m, full(), segment::Table::Logs, &nodes, &None)
            .is_err()
    );
}

#[test]
fn metadata_outside_manifest_snapshot_is_not_cached() {
    let f = Fixture::new();
    let (dir, m) = f.raw("outside", &[(4, "n".into(), record(vec![]))], false);
    let mut cache = Cache::default();
    let nodes = HashSet::from(["n".into()]);
    assert_eq!(
        cache
            .select(&dir, &m, full(), segment::Table::Logs, &nodes, &None)
            .unwrap(),
        selected_empty()
    );
    assert!(cache.entries.is_empty());
}

#[test]
fn oversized_descriptor_uses_exact_scan_without_cache_admission() {
    let f = Fixture::new();
    let (dir, mut m) = f.raw(
        "large-descriptor",
        &[(1, "n".into(), record(vec![]))],
        false,
    );
    m.files.get_mut("batches.parquet").unwrap().sha256 = "x".repeat(RESIDENT_BYTES + 1);
    let mut cache = Cache::default();
    assert_eq!(
        cache
            .select(
                &dir,
                &m,
                full(),
                segment::Table::Logs,
                &HashSet::from(["n".into()]),
                &None
            )
            .unwrap(),
        selected_empty()
    );
    assert!(cache.entries.is_empty());
    assert_eq!(cache.charged, 0);
}

#[test]
fn staging_overflow_falls_back_and_resident_eviction_remains_bounded() {
    let f = Fixture::new();
    let records: Vec<_> = (0..2500)
        .map(|i| (1, format!("node-{i:04}"), record(vec![])))
        .collect();
    let (dir, m) = f.raw("too-many", &records, false);
    let nodes = HashSet::from(["node-0000".into()]);
    let mut cache = Cache::default();
    assert_eq!(
        cache
            .select(&dir, &m, full(), segment::Table::Logs, &nodes, &None)
            .unwrap(),
        selected_empty()
    );
    assert!(cache.entries.is_empty());
    let (dir, m) = f.raw("bounded", &records[..1800], false);
    for i in 0..12 {
        let linked = f.root.join(format!("segment-{i}"));
        std::fs::create_dir(&linked).unwrap();
        std::fs::hard_link(dir.join("batches.parquet"), linked.join("batches.parquet")).unwrap();
        cache
            .select(&linked, &m, full(), segment::Table::Logs, &nodes, &None)
            .unwrap();
        assert!(cache.charged <= RESIDENT_BYTES);
        assert!(cache.entries.len() <= SEGMENTS);
        assert!(
            cache
                .entries
                .iter()
                .all(|entry| entry.charge <= BUILD_BYTES)
        );
    }
    assert!(
        cache.entries.len() < 12,
        "real bounded inputs must have evicted a previous Segment"
    );
    let before = cache.scans;
    cache
        .select(
            &f.root.join("segment-0"),
            &m,
            full(),
            segment::Table::Logs,
            &nodes,
            &None,
        )
        .unwrap();
    assert_eq!(cache.scans, before + 1);
}
