use fabric_o11y::{
    Event, EventId, EventTime, ObservedTime, Payload, ResourceId, SourceId, TenantId,
};
use std::{fs, num::NonZeroUsize};
use storage_probe::{
    Query,
    disk::{self, DiskSnapshot},
    resume::{self, Binding},
};

#[test]
#[ignore = "requires the Linux preload fault shim; run tools/bench/check_disk_io.py"]
fn partial_primary_manifest_read_is_counted_on_valid_fallback() {
    let dir = std::env::temp_dir().join(format!("disk-gpt-metadata-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let snap_dir = dir.join("snapshot");
    let row = Event {
        id: EventId(1),
        tenant: TenantId(1),
        source: SourceId(1),
        resource: ResourceId(1),
        event_time: EventTime(1),
        observed_time: ObservedTime(1),
        attributes: vec![],
        payload: Payload::Log { body: "row".into() },
    };
    let p = disk::publish(vec![row], NonZeroUsize::new(1).unwrap(), 911, &snap_dir).unwrap();
    let manifest = fs::read(snap_dir.join("manifest.json")).unwrap();
    fs::write(snap_dir.join("manifest.rebuilt.json"), &manifest).unwrap();
    unsafe {
        std::env::set_var("INJECT_PARTIAL_METADATA", "1");
    }
    let snap = DiskSnapshot::open(&snap_dir, p.clone()).unwrap();
    unsafe {
        std::env::remove_var("INJECT_PARTIAL_METADATA");
    }
    let binding = Binding {
        anchor: p.anchor,
        query: Query {
            start_ns: i64::MIN,
            end_ns: i64::MAX,
            tenant: None,
            token: None,
        },
        tokenizer_version: resume::TOKENIZER_VERSION,
        order_version: resume::ORDER_VERSION,
    };
    let answer = snap.query(&binding, &[false]).unwrap();
    eprintln!(
        "manifest_size={} metadata_bytes={} raw_files={}",
        manifest.len(),
        answer.reads.metadata_bytes,
        answer.reads.raw_files
    );
    assert_eq!(answer.reads.metadata_bytes, manifest.len() as u64 + 64);
    assert_eq!(answer.reads.raw_files, 0);
    fs::remove_dir_all(dir).unwrap();
}
