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
fn partial_successful_raw_read_is_counted_even_when_later_read_fails() {
    let dir = std::env::temp_dir().join(format!("disk-gpt-partial-{}", std::process::id()));
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
        payload: Payload::Log {
            body: "x".repeat(1024),
        },
    };
    let p = disk::publish(vec![row], NonZeroUsize::new(1).unwrap(), 910, &snap_dir).unwrap();
    let snap = DiskSnapshot::open(&snap_dir, p.clone()).unwrap();
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
    let answer = snap.query(&binding, &[true]).unwrap();
    eprintln!(
        "raw_files={} raw_bytes={} unavailable={:?}",
        answer.reads.raw_files, answer.reads.raw_bytes, answer.reads.unavailable
    );
    assert_eq!(answer.reads.raw_files, 2);
    assert_eq!(answer.reads.raw_bytes, 64);
    assert_eq!(answer.reads.unavailable, vec![0]);
    fs::remove_dir_all(dir).unwrap();
}
