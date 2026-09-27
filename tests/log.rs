use fabric_o11y::log::EventLog;
use fabric_o11y::{
    Attribute, Event, EventId, EventTime, ObservedTime, Payload, ResourceId, Scalar, SourceId,
    TenantId,
};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const HEADER_LEN: usize = 16;
const COMMIT_LEN: usize = 16;
const MAX_PAYLOAD_LEN: usize = 16 * 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        for _ in 0..100 {
            let path = std::env::temp_dir().join(format!(
                "fabric-o11y-log-test-{}-{nonce}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create temporary directory: {error}"),
            }
        }
        panic!("could not allocate a unique temporary directory");
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

fn log_event(id: u64) -> Event {
    Event {
        id: EventId(id),
        tenant: TenantId(u64::MAX),
        source: SourceId(0),
        resource: ResourceId(9001),
        event_time: EventTime(i64::MIN),
        observed_time: ObservedTime(i64::MAX),
        attributes: vec![
            Attribute {
                key: "enabled".into(),
                value: Scalar::Bool(true),
            },
            Attribute {
                key: "disabled".into(),
                value: Scalar::Bool(false),
            },
            Attribute {
                key: "signed".into(),
                value: Scalar::I64(i64::MIN),
            },
            Attribute {
                key: "unsigned".into(),
                value: Scalar::U64(u64::MAX),
            },
            Attribute {
                key: "fraction".into(),
                value: Scalar::F64(1.25),
            },
            Attribute {
                key: "unicode".into(),
                value: Scalar::String("snowman ☃ and nul \0".into()),
            },
        ],
        payload: Payload::Log {
            body: "a log body with unicode 🦀 and nul \0".into(),
        },
    }
}

fn gauge_event(id: u64) -> Event {
    Event {
        id: EventId(id),
        tenant: TenantId(1),
        source: SourceId(u64::MAX),
        resource: ResourceId(0),
        event_time: EventTime(-42),
        observed_time: ObservedTime(0),
        attributes: vec![],
        payload: Payload::Gauge {
            name: "request.duration".into(),
            value: -42.5,
            unit: "ms".into(),
        },
    }
}

fn replay_all(log: &mut EventLog) -> Vec<Event> {
    let mut events = Vec::new();
    let count = log.replay(|event| {
        events.push(event);
        Ok(())
    });
    assert_eq!(count.unwrap(), events.len());
    events
}

fn append_bytes(path: &Path, bytes: &[u8]) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

fn frame_for(event: &Event) -> Vec<u8> {
    let temp = TempDir::new();
    let path = temp.log_path();
    let mut log = EventLog::open(&path).unwrap();
    log.append(event).unwrap();
    drop(log);
    fs::read(path).unwrap()
}

fn frame_for_at(event: &Event, offset: u64) -> Vec<u8> {
    let mut frame = frame_for(event);
    let marker_start = frame.len() - COMMIT_LEN;
    let data_end = offset + marker_start as u64;
    frame[marker_start + 4..marker_start + 12].copy_from_slice(&data_end.to_le_bytes());
    let marker_crc = crc32fast::hash(&frame[marker_start..marker_start + 12]);
    frame[marker_start + 12..].copy_from_slice(&marker_crc.to_le_bytes());
    frame
}

#[test]
fn reopens_and_streams_all_domain_variants_in_order() {
    let temp = TempDir::new();
    let path = temp.log_path();
    let expected = vec![log_event(1), gauge_event(2), log_event(3)];

    let mut log = EventLog::open(&path).unwrap();
    for event in &expected {
        log.append(event).unwrap();
    }
    assert_eq!(replay_all(&mut log), expected);
    drop(log);

    let mut reopened = EventLog::open(&path).unwrap();
    assert_eq!(replay_all(&mut reopened), expected);
    reopened.append(&gauge_event(4)).unwrap();
    drop(reopened);

    let mut reopened = EventLog::open(&path).unwrap();
    let mut expected = expected;
    expected.push(gauge_event(4));
    assert_eq!(replay_all(&mut reopened), expected);
}

#[test]
fn floating_point_bits_survive_reopen() {
    let temp = TempDir::new();
    let path = temp.log_path();
    let scalar_bits = 0x7ff8_0000_0000_0123_u64;
    let mut event = gauge_event(44);
    event.attributes.push(Attribute {
        key: "nan".into(),
        value: Scalar::F64(f64::from_bits(scalar_bits)),
    });
    if let Payload::Gauge { value, .. } = &mut event.payload {
        *value = -0.0;
    }

    let mut log = EventLog::open(&path).unwrap();
    log.append(&event).unwrap();
    drop(log);
    let mut reopened = EventLog::open(&path).unwrap();
    let mut stored = replay_all(&mut reopened);
    let stored = stored.pop().unwrap();
    assert_eq!(stored.attributes.len(), 1);
    let Scalar::F64(scalar) = &stored.attributes[0].value else {
        panic!("expected floating-point attribute");
    };
    assert_eq!(scalar.to_bits(), scalar_bits);
    let Payload::Gauge { value, .. } = stored.payload else {
        panic!("expected gauge payload");
    };
    assert_eq!(value.to_bits(), (-0.0_f64).to_bits());
}

#[test]
fn empty_log_replays_zero_records() {
    let temp = TempDir::new();
    let mut log = EventLog::open(temp.log_path()).unwrap();
    assert!(replay_all(&mut log).is_empty());
}

#[test]
fn frame_header_has_specified_layout_and_bounded_length() {
    let frame = frame_for(&log_event(10));
    assert!(frame.len() > HEADER_LEN);
    assert_eq!(&frame[..4], b"FOL2");
    let length = u32::from_le_bytes(frame[4..8].try_into().unwrap());
    let header_crc = u32::from_le_bytes(frame[8..12].try_into().unwrap());
    assert_eq!(header_crc, crc32fast::hash(&frame[..8]));
    assert!(length as usize <= MAX_PAYLOAD_LEN);
    let marker_start = HEADER_LEN + length as usize;
    assert_eq!(frame.len(), marker_start + COMMIT_LEN);
    assert_eq!(&frame[marker_start..marker_start + 4], b"FOC2");
    let marked_end = u64::from_le_bytes(
        frame[marker_start + 4..marker_start + 12]
            .try_into()
            .unwrap(),
    );
    assert_eq!(marked_end, marker_start as u64);
    let marker_crc = u32::from_le_bytes(frame[marker_start + 12..].try_into().unwrap());
    assert_eq!(
        marker_crc,
        crc32fast::hash(&frame[marker_start..marker_start + 12])
    );
}

#[test]
fn partial_header_is_removed_without_changing_valid_prefix() {
    let temp = TempDir::new();
    let path = temp.log_path();
    let mut log = EventLog::open(&path).unwrap();
    log.append(&log_event(1)).unwrap();
    drop(log);
    let prefix = fs::read(&path).unwrap();
    let next = frame_for(&gauge_event(2));
    append_bytes(&path, &next[..7]);

    let mut recovered = EventLog::open(&path).unwrap();
    assert_eq!(fs::read(&path).unwrap(), prefix);
    assert_eq!(replay_all(&mut recovered), vec![log_event(1)]);
    recovered.append(&gauge_event(2)).unwrap();
    assert_eq!(
        replay_all(&mut recovered),
        vec![log_event(1), gauge_event(2)]
    );
}

#[test]
fn partial_payload_is_removed_without_changing_valid_prefix() {
    let temp = TempDir::new();
    let path = temp.log_path();
    let mut log = EventLog::open(&path).unwrap();
    log.append(&log_event(1)).unwrap();
    drop(log);
    let prefix = fs::read(&path).unwrap();
    let next = frame_for(&gauge_event(2));
    assert!(next.len() > HEADER_LEN + 1);
    append_bytes(&path, &next[..HEADER_LEN + 1]);

    let mut recovered = EventLog::open(&path).unwrap();
    assert_eq!(fs::read(&path).unwrap(), prefix);
    assert_eq!(replay_all(&mut recovered), vec![log_event(1)]);
    recovered.append(&gauge_event(2)).unwrap();
    assert_eq!(
        replay_all(&mut recovered),
        vec![log_event(1), gauge_event(2)]
    );
}

#[test]
fn complete_event_without_commit_marker_is_removed() {
    let temp = TempDir::new();
    let path = temp.log_path();
    let mut log = EventLog::open(&path).unwrap();
    log.append(&log_event(1)).unwrap();
    drop(log);
    let prefix = fs::read(&path).unwrap();
    let next = frame_for(&gauge_event(2));
    append_bytes(&path, &next[..next.len() - COMMIT_LEN]);

    let mut recovered = EventLog::open(&path).unwrap();
    assert_eq!(fs::read(&path).unwrap(), prefix);
    assert_eq!(replay_all(&mut recovered), vec![log_event(1)]);
    recovered.append(&gauge_event(2)).unwrap();
    assert_eq!(
        replay_all(&mut recovered),
        vec![log_event(1), gauge_event(2)]
    );
}

#[test]
fn partial_commit_marker_is_removed_with_its_unacknowledged_event() {
    let temp = TempDir::new();
    let path = temp.log_path();
    let mut log = EventLog::open(&path).unwrap();
    log.append(&log_event(1)).unwrap();
    drop(log);
    let prefix = fs::read(&path).unwrap();
    let next = frame_for_at(&gauge_event(2), prefix.len() as u64);
    append_bytes(&path, &next[..next.len() - COMMIT_LEN + 7]);

    let mut recovered = EventLog::open(&path).unwrap();
    assert_eq!(fs::read(&path).unwrap(), prefix);
    assert_eq!(replay_all(&mut recovered), vec![log_event(1)]);
    recovered.append(&gauge_event(2)).unwrap();
    assert_eq!(
        replay_all(&mut recovered),
        vec![log_event(1), gauge_event(2)]
    );
}

#[test]
fn distinguishably_invalid_tails_are_not_silently_removed() {
    for mutation in [
        "foreign short tail",
        "undersized partial header",
        "oversized partial header",
        "bad partial header checksum",
        "bad partial marker offset",
        "bad partial marker checksum",
        "bad complete marker",
    ] {
        let temp = TempDir::new();
        let path = temp.log_path();
        let mut log = EventLog::open(&path).unwrap();
        log.append(&log_event(1)).unwrap();
        drop(log);
        match mutation {
            "foreign short tail" => append_bytes(&path, b"hello"),
            "undersized partial header" => {
                let mut short = b"FOL2".to_vec();
                short.extend_from_slice(&56_u32.to_le_bytes());
                append_bytes(&path, &short);
            }
            "oversized partial header" => {
                let mut short = b"FOL2".to_vec();
                short.extend_from_slice(&((MAX_PAYLOAD_LEN as u32) + 1).to_le_bytes());
                append_bytes(&path, &short);
            }
            "bad partial header checksum" => {
                let mut next = frame_for(&gauge_event(2));
                next[8] ^= 0x01;
                append_bytes(&path, &next[..12]);
            }
            "bad partial marker offset" | "bad partial marker checksum" => {
                let offset = fs::metadata(&path).unwrap().len();
                let mut next = frame_for_at(&gauge_event(2), offset);
                let marker_start = next.len() - COMMIT_LEN;
                let short_len = if mutation == "bad partial marker offset" {
                    next[marker_start + 4] ^= 0x01;
                    marker_start + 5
                } else {
                    next[marker_start + 12] ^= 0x01;
                    marker_start + 13
                };
                append_bytes(&path, &next[..short_len]);
            }
            "bad complete marker" => {
                let offset = fs::metadata(&path).unwrap().len();
                let mut next = frame_for_at(&gauge_event(2), offset);
                let last = next.len() - 1;
                next[last] ^= 0x01;
                append_bytes(&path, &next);
            }
            _ => unreachable!(),
        }
        let before = fs::read(&path).unwrap();
        assert!(EventLog::open(&path).is_err(), "mutation: {mutation}");
        assert_eq!(fs::read(&path).unwrap(), before, "mutation: {mutation}");
    }
}

#[test]
fn full_malformed_or_corrupt_frame_is_rejected_without_truncation() {
    for mutation in ["magic", "length beyond tail", "header crc", "payload crc"] {
        let temp = TempDir::new();
        let path = temp.log_path();
        let mut log = EventLog::open(&path).unwrap();
        log.append(&log_event(1)).unwrap();
        drop(log);
        let valid_prefix = fs::read(&path).unwrap();
        let mut bad_frame = frame_for(&gauge_event(2));
        match mutation {
            "magic" => bad_frame[0] ^= 0x01,
            "length beyond tail" => {
                let len = u32::from_le_bytes(bad_frame[4..8].try_into().unwrap());
                bad_frame[4..8].copy_from_slice(&(len + 1).to_le_bytes());
            }
            "header crc" => bad_frame[8] ^= 0x01,
            "payload crc" => bad_frame[HEADER_LEN] ^= 0x01,
            _ => unreachable!(),
        }
        append_bytes(&path, &bad_frame);
        let before = fs::read(&path).unwrap();

        assert!(EventLog::open(&path).is_err(), "mutation: {mutation}");
        assert_eq!(fs::read(&path).unwrap(), before, "mutation: {mutation}");
        assert_eq!(&before[..valid_prefix.len()], valid_prefix);
    }
}

#[test]
fn oversized_append_fails_and_keeps_the_borrowed_event_intact() {
    let temp = TempDir::new();
    let path = temp.log_path();
    let mut event = log_event(7);
    event.payload = Payload::Log {
        body: "x".repeat(MAX_PAYLOAD_LEN + 1),
    };
    let mut log = EventLog::open(&path).unwrap();

    assert!(log.append(&event).is_err());
    assert_eq!(event.id, EventId(7));
    match &event.payload {
        Payload::Log { body } => {
            assert_eq!(body.len(), MAX_PAYLOAD_LEN + 1);
            assert!(body.bytes().all(|byte| byte == b'x'));
        }
        Payload::Gauge { .. } => panic!("append changed the caller's payload"),
    }
}

#[test]
fn a_second_open_cannot_take_the_writer_lock() {
    let temp = TempDir::new();
    let path = temp.log_path();
    let mut first = EventLog::open(&path).unwrap();
    first.append(&log_event(1)).unwrap();
    let before = fs::read(&path).unwrap();

    assert!(EventLog::open(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    drop(first);

    let mut second = EventLog::open(&path).unwrap();
    assert_eq!(replay_all(&mut second), vec![log_event(1)]);
}
