#![cfg(feature = "append-attribution")]

use fabric_o11y::log::{EventLog, MAX_RECORD_BYTES};
use fabric_o11y::{
    Event, EventId, EventTime, ObservedTime, Payload, ResourceId, SourceId, TenantId,
};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "fabric-o11y-attribution-{}-{nonce}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn log_path(&self) -> PathBuf {
        self.0.join("events.log")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn event(id: u64, body: String) -> Event {
    Event {
        id: EventId(id),
        tenant: TenantId(1),
        source: SourceId(2),
        resource: ResourceId(3),
        event_time: EventTime(4),
        observed_time: ObservedTime(5),
        attributes: vec![],
        payload: Payload::Log { body },
    }
}

#[test]
fn samples_only_successful_appends_and_recovers_after_rejected_encoding() {
    let temp = TempDir::new();
    let path = temp.log_path();
    let mut log = EventLog::open(&path).unwrap();
    assert!(log.last_append_phases().is_none());

    let first = event(1, "first".into());
    let outer_start = Instant::now();
    log.append(&first).unwrap();
    let outer_ns = outer_start.elapsed().as_nanos();
    let phases = log.last_append_phases().expect("successful sample");
    let inner_ns = phases.encode_ns
        + phases.data_write_ns
        + phases.data_sync_ns
        + phases.marker_write_ns
        + phases.marker_sync_ns;
    assert!(inner_ns <= outer_ns);
    let committed_bytes = fs::read(&path).unwrap();

    let oversized = event(2, "x".repeat(MAX_RECORD_BYTES + 1));
    assert!(log.append(&oversized).is_err());
    assert!(log.last_append_phases().is_none());
    assert_eq!(fs::read(&path).unwrap(), committed_bytes);

    let second = event(3, "second".into());
    log.append(&second).unwrap();
    assert!(log.last_append_phases().is_some());
    let mut replayed = Vec::new();
    assert_eq!(
        log.replay(|stored| {
            replayed.push(stored);
            Ok(())
        })
        .unwrap(),
        2
    );
    assert_eq!(replayed, vec![first, second]);
}
