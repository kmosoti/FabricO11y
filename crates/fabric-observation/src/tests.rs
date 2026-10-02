//! Canonical-form checks with the defects the decoder must reject.

use super::*;

fn node(n: u8) -> [u8; 16] {
    let mut id = [0_u8; 16];
    id[0] = n;
    id[15] = n;
    id
}

fn sample() -> Vec<Observation> {
    let strand = Strand {
        node_id: node(7),
        generation: 3,
    };
    vec![
        Observation {
            strand,
            sequence: 41,
            index: 0,
            time_ns: 1_800_000_000_000_000_000,
            locators: None,
            attributes: vec![
                ("log.file.offset".into(), Value::Int(4096)),
                ("log.file.path".into(), Value::Str("/var/log/syslog".into())),
            ],
            signal: Signal::Log {
                severity: 9,
                event: String::new(),
                body: "kernel: eth0 link up".into(),
            },
        },
        Observation {
            strand,
            sequence: 41,
            index: 1,
            time_ns: 1_800_000_000_000_500_000,
            locators: Some(Locators {
                trace_id: [1; 16],
                span_id: [2; 8],
                parent_span_id: None,
            }),
            attributes: vec![("state".into(), Value::Str("idle".into()))],
            signal: Signal::Point {
                name: "system.cpu.time".into(),
                unit: "s".into(),
                kind: PointKind::Sum {
                    monotonic: true,
                    start_ns: 1_799_999_999_000_000_000,
                },
                value: Number::Double(12.5),
            },
        },
        Observation {
            strand: Strand {
                node_id: node(9),
                generation: 1,
            },
            sequence: 42,
            index: 0,
            time_ns: 1_800_000_000_000_200_000,
            locators: Some(Locators {
                trace_id: [1; 16],
                span_id: [3; 8],
                parent_span_id: Some([2; 8]),
            }),
            attributes: vec![
                ("http.status".into(), Value::Int(200)),
                ("ok".into(), Value::Bool(true)),
                ("ratio".into(), Value::Double(Bits::from_f64(-0.0))),
                ("raw".into(), Value::Bytes(vec![0, 255])),
            ],
            signal: Signal::Span {
                name: "GET /".into(),
                end_ns: 1_800_000_000_000_900_000,
                status: Status::Ok,
                kind: SpanKind::Server,
            },
        },
    ]
}

/// Recomputes the CRC after a structural mutation, so the structure is what gets tested.
fn recrc(mut bytes: Vec<u8>) -> Vec<u8> {
    let n = bytes.len() - 4;
    let crc = crc32fast::hash(&bytes[..n]);
    bytes.truncate(n);
    bytes.extend_from_slice(&crc.to_le_bytes());
    bytes
}

fn reason(bytes: &[u8]) -> &'static str {
    decode(bytes).expect_err("must be rejected").reason
}

#[test]
fn round_trip_all_three_signals() {
    let block = sample();
    let bytes = encode(&block).unwrap();
    assert_eq!(&bytes[..4], MAGIC);
    let back = decode(&bytes).unwrap();
    assert_eq!(back, block);
    assert_eq!(
        encode(&back).unwrap(),
        bytes,
        "re-encoding is the same bytes"
    );
}

#[test]
fn rejects_empty_and_oversized_blocks() {
    assert_eq!(encode(&[]), Err(EncodeError::Empty));
    let one = sample().remove(0);
    let many = vec![one; MAX_RECORDS + 1];
    assert_eq!(
        encode(&many),
        Err(EncodeError::TooManyRecords(MAX_RECORDS + 1))
    );
}

#[test]
fn rejects_non_canonical_records_before_encoding() {
    let mut r = sample().remove(0);
    r.attributes = vec![("b".into(), Value::Int(1)), ("a".into(), Value::Int(2))];
    assert!(matches!(
        encode(&[r.clone()]),
        Err(EncodeError::Invalid { index: 0, .. })
    ));
    r.attributes = vec![("a".into(), Value::Int(1)), ("a".into(), Value::Int(2))];
    assert!(encode(&[r.clone()]).is_err(), "duplicate keys");
    r.attributes = vec![("a".into(), Value::Double(Bits::from_f64(f64::NAN)))];
    assert!(encode(&[r.clone()]).is_err(), "NaN attribute");
    r.attributes.clear();
    r.signal = Signal::Log {
        severity: 25,
        event: String::new(),
        body: String::new(),
    };
    assert!(encode(&[r.clone()]).is_err(), "severity 25");
    if let Signal::Point { value, .. } = &mut sample()[1].signal {
        *value = Number::Double(f64::NAN);
    }
    let mut p = sample().remove(1);
    p.signal = Signal::Point {
        name: "x".into(),
        unit: String::new(),
        kind: PointKind::Gauge,
        value: Number::Double(f64::NAN),
    };
    assert!(encode(&[p]).is_err(), "NaN point");
}

#[test]
fn rejects_crc_mismatch_truncation_and_trailing_bytes() {
    let bytes = encode(&sample()).unwrap();
    let mut bad = bytes.clone();
    bad[10] ^= 1;
    assert_eq!(reason(&bad), "CRC mismatch");
    assert_eq!(reason(&bytes[..bytes.len() - 1]), "CRC mismatch");
    assert_eq!(reason(&bytes[..3]), "truncated");
    let mut longer = bytes.clone();
    longer.insert(bytes.len() - 4, 0);
    assert_eq!(reason(&recrc(longer)), "trailing bytes");
    let mut magic = bytes.clone();
    magic[0] = b'X';
    assert_eq!(reason(&recrc(magic)), "not an FOB1 block");
}

#[test]
fn rejects_overlong_varints() {
    // The record count (byte 4) is 3, encoded as one byte; 0x83 0x00 is the same value overlong.
    let bytes = encode(&sample()).unwrap();
    assert_eq!(bytes[4], 3);
    let mut bad = bytes.clone();
    bad[4] = 0x83;
    bad.insert(5, 0x00);
    assert_eq!(reason(&recrc(bad)), "overlong varint");
}

#[test]
fn rejects_dictionaries_out_of_first_use_order_or_unused() {
    let bytes = encode(&sample()).unwrap();
    // Find the strands column: header, node table, string table.
    let mut cur = Cursor {
        bytes: &bytes,
        pos: 0,
    };
    cur.take(4).unwrap();
    cur.uvar().unwrap();
    let node_count = cur.count(16).unwrap();
    assert_eq!(node_count, 2, "two nodes");
    cur.take(16 * node_count).unwrap();
    let string_count = cur.count(1).unwrap();
    for _ in 0..string_count {
        cur.bytes_field().unwrap();
    }
    // Skip the time column: one varint per record.
    for _ in 0..3 {
        cur.uvar().unwrap();
    }
    let strands_at = cur.pos;
    assert_eq!(bytes[strands_at], 0, "record 0 names node id 0");
    // Record 0 naming id 1 first breaks first-use order.
    let mut early = bytes.clone();
    early[strands_at] = 1;
    assert_eq!(
        reason(&recrc(early)),
        "dictionary id used before its first-use turn"
    );
    // A third node id that nothing references.
    let mut unused = bytes.clone();
    unused[5] = 3;
    let extra = [0xAB_u8; 16];
    for (i, b) in extra.iter().enumerate() {
        unused.insert(38 + i, *b);
    }
    assert_eq!(reason(&recrc(unused)), "unused node dictionary entry");
    // Two equal table entries: found by the mutation property
    // (CX-FOB1-DUPLICATE-DICTIONARY); the encoder would intern them as one.
    let mut dup = bytes.clone();
    dup.copy_within(6..22, 22);
    assert_eq!(reason(&recrc(dup)), "duplicate node dictionary entry");
    // Swapping the two table entries is a different, still canonical, block.
    let mut swapped = bytes.clone();
    for i in 0..16 {
        swapped.swap(6 + i, 22 + i);
    }
    let swapped = recrc(swapped);
    assert_eq!(encode(&decode(&swapped).unwrap()).unwrap(), swapped);
}

#[test]
fn single_bit_flips_are_rejected_or_canonical() {
    // The canonical property, structurally: with the CRC repaired, any accepted
    // mutation must re-encode to exactly itself.
    let bytes = encode(&sample()).unwrap();
    let mut accepted = 0;
    for i in 0..bytes.len() - 4 {
        for bit in 0..8 {
            let mut m = bytes.clone();
            m[i] ^= 1 << bit;
            let m = recrc(m);
            if let Ok(block) = decode(&m) {
                accepted += 1;
                assert_eq!(encode(&block).unwrap(), m, "flip at byte {i} bit {bit}");
            }
        }
    }
    assert!(
        accepted > 0,
        "some flips (body bytes, values) must still be canonical"
    );
}

#[test]
fn varint_and_delta_helpers_are_bijections_on_samples() {
    for v in [0_i64, 1, -1, i64::MAX, i64::MIN, 12345, -98765] {
        assert_eq!(unzigzag(zigzag(v)), v);
    }
    for (a, b) in [(0_u64, 0_u64), (5, 9), (u64::MAX, 0), (0, u64::MAX), (7, 7)] {
        assert_eq!(undelta(delta(a, b), b), a);
    }
    for v in [0_u64, 127, 128, 300, u64::MAX, 1 << 63] {
        let mut out = Vec::new();
        put_uvar(&mut out, v);
        let mut cur = Cursor {
            bytes: &out,
            pos: 0,
        };
        assert_eq!(cur.uvar().unwrap(), v);
        assert_eq!(cur.pos, out.len());
    }
}

/// Writes the fuzz corpus seeds. `cargo test -p fabric-observation -- --ignored write_fuzz_seeds`.
#[test]
#[ignore]
fn write_fuzz_seeds() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/corpus/observation_block");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("three-signals.fob"), encode(&sample()).unwrap()).unwrap();
    let one = vec![sample().remove(0)];
    std::fs::write(dir.join("one-log.fob"), encode(&one).unwrap()).unwrap();
    let mut many = Vec::new();
    for i in 0..200_u64 {
        let mut r = sample().remove(1);
        r.sequence = 100 + i / 10;
        r.index = (i % 10) as u32;
        r.time_ns = 1_800_000_000_000_000_000 + i * 15_000_000_000;
        many.push(r);
    }
    std::fs::write(dir.join("two-hundred-points.fob"), encode(&many).unwrap()).unwrap();
    // CX-FOB1-DUPLICATE-DICTIONARY as a fuzz regression input: a repeated node entry, CRC repaired.
    let reg = dir.join("../../regressions/observation_block");
    std::fs::create_dir_all(&reg).unwrap();
    let mut dup = encode(&sample()).unwrap();
    dup.copy_within(6..22, 22);
    std::fs::write(reg.join("duplicate-dictionary.fob"), recrc(dup)).unwrap();
}
