use fabric_o11y::{
    Event, EventId, EventTime, ObservedTime, Payload, ResourceId, SourceId, TenantId,
};
use std::{
    fs,
    num::NonZeroUsize,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use storage_probe::{
    Query,
    coverage::CoverageStatus,
    disk::{self, DiskSnapshot},
    resume::{self, Accumulator, Binding},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "disk-gpt-probe-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn row(id: u64) -> Event {
    Event {
        id: EventId(id),
        tenant: TenantId(3),
        source: SourceId(4),
        resource: ResourceId(5),
        event_time: EventTime(id as i64),
        observed_time: ObservedTime(id as i64),
        attributes: vec![],
        payload: Payload::Log {
            body: format!("row-{id}"),
        },
    }
}
fn bind(p: &disk::Publication) -> Binding {
    Binding {
        anchor: p.anchor.clone(),
        query: Query {
            start_ns: i64::MIN,
            end_ns: i64::MAX,
            tenant: None,
            token: None,
        },
        tokenizer_version: resume::TOKENIZER_VERSION,
        order_version: resume::ORDER_VERSION,
    }
}

#[test]
fn ambiguous_rebuild_fails_closed_until_bad_hot_copy_is_removed() {
    let s = Scratch::new();
    let dir = s.0.join("snapshot");
    let p = disk::publish(
        vec![row(1), row(2), row(3)],
        NonZeroUsize::new(2).unwrap(),
        901,
        &dir,
    )
    .unwrap();
    fs::create_dir(dir.join("cold")).unwrap();
    let original = fs::read(dir.join("block-0.json")).unwrap();
    fs::write(dir.join("cold/block-0.json"), &original).unwrap();
    let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
    changed["events"][0]["id"] = serde_json::json!(999);
    fs::write(
        dir.join("block-0.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    fs::remove_file(dir.join("manifest.json")).unwrap();
    // Whole-root authority cannot identify the good member of conflicting copies.
    assert!(disk::rebuild_manifest(&dir, &p).is_err());
    assert!(!dir.join("manifest.rebuilt.json").exists());
    fs::remove_file(dir.join("block-0.json")).unwrap();
    let path =
        disk::rebuild_manifest(&dir, &p).expect("rebuild from the restored unambiguous source");
    assert_eq!(path, dir.join("manifest.rebuilt.json"));
    let snap = DiskSnapshot::open(&dir, p.clone()).unwrap();
    let answer = snap.query(&bind(&p), &[true, true]).unwrap();
    assert_eq!(
        Accumulator::new(bind(&p), answer.page).unwrap().status(),
        CoverageStatus::Complete
    );
}

#[cfg(unix)]
#[test]
fn publish_rejects_preexisting_dangling_symlink_name() {
    use std::os::unix::fs::symlink;
    let s = Scratch::new();
    let final_dir = s.0.join("snapshot");
    symlink("missing-destination", &final_dir).unwrap();
    let before = fs::read_link(&final_dir).unwrap();
    let result = disk::publish(vec![row(1)], NonZeroUsize::new(1).unwrap(), 902, &final_dir);
    assert!(
        result.is_err(),
        "publisher accepted a non-fresh final name: {result:?}"
    );
    assert_eq!(fs::read_link(&final_dir).unwrap(), before);
}

#[test]
fn invalid_hot_falls_back_to_valid_cold_and_counts_both_copies() {
    let s = Scratch::new();
    let dir = s.0.join("snapshot");
    let p = disk::publish(vec![row(1)], NonZeroUsize::new(1).unwrap(), 903, &dir).unwrap();
    fs::create_dir(dir.join("cold")).unwrap();
    let original = fs::read(dir.join("block-0.json")).unwrap();
    fs::write(dir.join("cold/block-0.json"), &original).unwrap();
    fs::write(dir.join("block-0.json"), b"oops").unwrap();
    let snap = DiskSnapshot::open(&dir, p.clone()).unwrap();
    let answer = snap.query(&bind(&p), &[true]).unwrap();
    assert_eq!(answer.reads.raw_files, 2);
    assert_eq!(answer.reads.raw_bytes, original.len() as u64 + 4);
    assert!(answer.reads.unavailable.is_empty());
    assert_eq!(
        Accumulator::new(bind(&p), answer.page).unwrap().status(),
        CoverageStatus::Complete
    );
}

#[test]
fn invalid_manifest_fallback_counts_both_metadata_files() {
    let s = Scratch::new();
    let dir = s.0.join("snapshot");
    let p = disk::publish(vec![row(1)], NonZeroUsize::new(1).unwrap(), 904, &dir).unwrap();
    let original = fs::read(dir.join("manifest.json")).unwrap();
    fs::write(dir.join("manifest.rebuilt.json"), &original).unwrap();
    fs::write(dir.join("manifest.json"), b"bad").unwrap();
    let snap = DiskSnapshot::open(&dir, p.clone()).unwrap();
    let answer = snap.query(&bind(&p), &[false]).unwrap();
    assert_eq!(answer.reads.metadata_bytes, original.len() as u64 + 3);
    assert_eq!(answer.reads.raw_files, 0);
    assert_eq!(
        Accumulator::new(bind(&p), answer.page).unwrap().status(),
        CoverageStatus::Incomplete {
            unavailable: vec![0]
        }
    );
}

#[test]
fn codec_rejects_duplicate_variant_tags() {
    let original = disk::encode_events(&[row(1)]).unwrap();
    let wire = String::from_utf8(original).unwrap();
    let duplicate_payload =
        wire.replace("\"kind\":\"log\",", "\"kind\":\"log\",\"kind\":\"gauge\",");
    assert_ne!(duplicate_payload, wire);
    assert!(disk::decode_events(duplicate_payload.as_bytes()).is_err());
    let scalar = wire.replace("\"attributes\":[]", "\"attributes\":[{\"key\":\"x\",\"value\":{\"kind\":\"i64\",\"kind\":\"u64\",\"value\":1}}]");
    assert_ne!(scalar, wire);
    assert!(disk::decode_events(scalar.as_bytes()).is_err());
}

#[test]
fn false_availability_and_failed_reads_both_remain_unavailable() {
    let s = Scratch::new();
    let dir = s.0.join("snapshot");
    let p = disk::publish(
        vec![row(1), row(2), row(3)],
        NonZeroUsize::new(1).unwrap(),
        920,
        &dir,
    )
    .unwrap();
    fs::remove_file(dir.join("block-1.json")).unwrap();
    let opened = DiskSnapshot::open(&dir, p.clone()).unwrap();
    let result = opened.query(&bind(&p), &[false, true, true]).unwrap();
    assert_eq!(result.reads.unavailable, vec![0, 1]);
    assert_eq!(result.reads.raw_files, 3);
    assert_eq!(
        Accumulator::new(bind(&p), result.page).unwrap().status(),
        CoverageStatus::Incomplete {
            unavailable: vec![0, 1]
        }
    );
}
