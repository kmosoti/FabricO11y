//! S3/S4 black-box contract tests, written from API.md before the layout implementation.
//! The scalar oracle deliberately does not call any layout query function.
use std::num::NonZeroUsize;

use arrow_schema::{DataType, Field};
use bytes::Bytes;
use fabric_o11y::{
    Attribute, Event, EventId, EventTime, ObservedTime, Payload, ResourceId, Scalar, SourceId,
    TenantId,
};
use layout_probe::{
    Codec, Hit, build_postings, decode, encode, query_full, query_postings, query_projected,
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use sha2::{Digest as _, Sha256};
use storage_probe::{Query, coverage};

fn log(id: u64, tenant: u64, time: i64, body: &str) -> Event {
    Event {
        id: EventId(id),
        tenant: TenantId(tenant),
        source: SourceId(u64::MAX - id),
        resource: ResourceId(id),
        event_time: EventTime(time),
        observed_time: ObservedTime(time.wrapping_neg()),
        attributes: vec![],
        payload: Payload::Log { body: body.into() },
    }
}

fn edge_rows() -> Vec<Event> {
    let mut first = log(u64::MAX, u64::MAX, i64::MIN, "rare\u{2003}β\tRARE\nrare");
    first.observed_time = ObservedTime(i64::MAX);
    first.attributes = vec![
        Attribute {
            key: "same".into(),
            value: Scalar::Bool(false),
        },
        Attribute {
            key: "same".into(),
            value: Scalar::I64(i64::MIN),
        },
        Attribute {
            key: "u".into(),
            value: Scalar::U64(u64::MAX),
        },
        Attribute {
            key: "nan".into(),
            value: Scalar::F64(f64::from_bits(0x7ff8_0000_0000_0042)),
        },
        Attribute {
            key: "zero".into(),
            value: Scalar::F64(-0.0),
        },
        Attribute {
            key: "unicode".into(),
            value: Scalar::String("é\u{0000}雪".into()),
        },
    ];
    let mut gauge = log(0, 0, i64::MAX, "");
    gauge.payload = Payload::Gauge {
        name: "cpu\u{0000}雪".into(),
        value: f64::from_bits(0xfff8_0000_0000_0007),
        unit: "".into(),
    };
    gauge.attributes = vec![
        Attribute {
            key: "inf".into(),
            value: Scalar::F64(f64::INFINITY),
        },
        Attribute {
            key: "neg_inf".into(),
            value: Scalar::F64(f64::NEG_INFINITY),
        },
        Attribute {
            key: "empty".into(),
            value: Scalar::String(String::new()),
        },
        Attribute {
            key: "true".into(),
            value: Scalar::Bool(true),
        },
        Attribute {
            key: "max".into(),
            value: Scalar::I64(i64::MAX),
        },
        Attribute {
            key: "min".into(),
            value: Scalar::U64(0),
        },
    ];
    vec![
        first,
        log(7, 1, -1, "rarely rare"),
        log(8, 1, 0, ""),
        gauge,
        log(9, 2, 1, "β rare"),
        log(10, 1, 2, "Rare"),
        log(11, 1, 3, "rare\u{00a0}rare"),
    ]
}

fn q(start_ns: i64, end_ns: i64, tenant: Option<u64>, token: Option<&str>) -> Query {
    Query {
        start_ns,
        end_ns,
        tenant,
        token: token.map(str::to_owned),
    }
}

fn oracle(rows: &[Event], query: &Query) -> Vec<Hit> {
    rows.iter().enumerate().filter_map(|(position, row)| {
        if row.event_time.0 < query.start_ns || row.event_time.0 > query.end_ns
            || query.tenant.is_some_and(|tenant| tenant != row.tenant.0)
            || query.token.as_ref().is_some_and(|token| {
                !matches!(&row.payload, Payload::Log { body } if body.split_whitespace().any(|word| word == token))
            }) {
            return None;
        }
        Some(Hit { position, digest: coverage::rows_digest(std::slice::from_ref(row)) })
    }).collect()
}

fn queries() -> Vec<Query> {
    vec![
        q(i64::MIN, i64::MAX, None, None),
        q(i64::MIN, i64::MIN, Some(u64::MAX), Some("rare")),
        q(-1, 3, Some(1), Some("rare")),
        q(-1, 3, Some(1), Some("Rare")),
        q(-1, 3, Some(1), Some("RARE")),
        q(-1, 3, Some(1), Some("rarely")),
        q(-1, 3, Some(1), Some("")),
        q(i64::MIN, i64::MAX, None, Some("β")),
        q(i64::MAX, i64::MAX, None, Some("rare")),
        q(5, 4, None, None),
        q(i64::MIN, i64::MAX, Some(999), None),
    ]
}

fn sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[test]
fn parquet_schema_metadata_and_row_group_limit_match_registered_layout() {
    for codec in [Codec::Plain, Codec::Zstd] {
        for group in [1, 3, 64] {
            for rows in [Vec::new(), edge_rows()] {
                let (bytes, _) = encode(&rows, NonZeroUsize::new(group).unwrap(), codec).unwrap();
                let builder = ParquetRecordBatchReaderBuilder::try_new(Bytes::from(bytes)).unwrap();
                let expected = [
                    Field::new("event_time", DataType::Int64, false),
                    Field::new("tenant", DataType::UInt64, false),
                    Field::new("payload_kind", DataType::UInt8, false),
                    Field::new("log_body", DataType::Utf8, true),
                    Field::new("raw_event", DataType::Binary, false),
                    Field::new("row_digest", DataType::FixedSizeBinary(32), false),
                ];
                let schema = builder.schema();
                assert_eq!(schema.fields().len(), expected.len());
                for (actual, expected) in schema.fields().iter().zip(&expected) {
                    assert_eq!(actual.as_ref(), expected);
                }
                let metadata = builder.metadata();
                let version = metadata
                    .file_metadata()
                    .key_value_metadata()
                    .and_then(|entries| {
                        entries
                            .iter()
                            .find(|entry| entry.key == "fabric.layout.version")
                    })
                    .and_then(|entry| entry.value.as_deref());
                assert_eq!(version, Some("1"));
                assert_eq!(metadata.file_metadata().num_rows(), rows.len() as i64);
                assert!(
                    metadata
                        .row_groups()
                        .iter()
                        .all(|row_group| row_group.num_rows() > 0
                            && row_group.num_rows() <= group as i64)
                );
            }
        }
    }
}

#[test]
fn round_trip_and_three_query_paths_match_scalar_oracle() {
    for codec in [Codec::Plain, Codec::Zstd] {
        for group in [1, 3, 64] {
            for rows in [Vec::new(), edge_rows(), {
                let mut rows = edge_rows();
                rows.push(log(12, 1, 4, "rare"));
                rows.push(log(12, 1, 4, "rare"));
                rows
            }] {
                let (bytes, anchor) =
                    encode(&rows, NonZeroUsize::new(group).unwrap(), codec).unwrap();
                assert_eq!(anchor.rows, rows.len());
                assert_eq!(anchor.version, 1);
                assert_eq!(anchor.sha256, sha(&bytes));
                let decoded = decode(&bytes, &anchor).unwrap();
                assert_eq!(decoded.len(), rows.len());
                for (expected, actual) in rows.iter().zip(&decoded) {
                    // The E1 digest binds every field and raw floating-point bit.
                    assert_eq!(
                        coverage::rows_digest(std::slice::from_ref(expected)),
                        coverage::rows_digest(std::slice::from_ref(actual))
                    );
                }
                for query in queries() {
                    let expected = oracle(&rows, &query);
                    assert_eq!(query_full(&bytes, &anchor, &query).unwrap(), expected);
                    assert_eq!(query_projected(&bytes, &anchor, &query).unwrap(), expected);
                    assert_eq!(
                        query_postings(&bytes, &anchor, &query, None).unwrap(),
                        expected
                    );
                }
            }
        }
    }
}

#[test]
fn duplicate_events_keep_distinct_physical_positions() {
    let same = log(7, 1, 0, "rare");
    let rows = vec![log(1, 2, -1, "rare"), same, log(7, 1, 0, "rare")];
    let (bytes, anchor) = encode(&rows, NonZeroUsize::new(1).unwrap(), Codec::Plain).unwrap();
    let query = q(0, 0, Some(1), Some("rare"));
    let expected = oracle(&rows, &query);
    assert_eq!(
        expected.iter().map(|hit| hit.position).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(expected[0].digest, expected[1].digest);
    assert_eq!(query_full(&bytes, &anchor, &query).unwrap(), expected);
    assert_eq!(query_projected(&bytes, &anchor, &query).unwrap(), expected);
    let (index, hash) = build_postings(&rows, &anchor, &["rare".into()]).unwrap();
    assert_eq!(
        query_postings(&bytes, &anchor, &query, Some((&index, hash))).unwrap(),
        expected
    );
}

#[test]
fn table_authentication_rejects_wrong_anchor_and_damaged_files_everywhere() {
    let rows = edge_rows();
    let (bytes, anchor) = encode(&rows, NonZeroUsize::new(3).unwrap(), Codec::Zstd).unwrap();
    let query = q(5, 4, None, None); // Empty answer must still authenticate the file.
    let mut wrong_hash = anchor.clone();
    wrong_hash.sha256[0] ^= 1;
    let mut wrong_count = anchor.clone();
    wrong_count.rows += 1;
    let mut wrong_version = anchor.clone();
    wrong_version.version += 1;
    let mut damaged = bytes.clone();
    let damaged_at = damaged.len() / 2;
    damaged[damaged_at] ^= 1;
    for (file, expected) in [
        (bytes.as_slice(), &wrong_hash),
        (bytes.as_slice(), &wrong_count),
        (bytes.as_slice(), &wrong_version),
        (damaged.as_slice(), &anchor),
        (&bytes[..bytes.len() - 1], &anchor),
        (&[][..], &anchor),
    ] {
        assert!(decode(file, expected).is_err());
        assert!(query_full(file, expected, &query).is_err());
        assert!(query_projected(file, expected, &query).is_err());
        assert!(query_postings(file, expected, &query, None).is_err());
    }
}

#[test]
fn postings_are_deterministic_exact_and_invalid_optional_indexes_fall_back() {
    let rows = edge_rows();
    let (bytes, anchor) = encode(&rows, NonZeroUsize::new(3).unwrap(), Codec::Plain).unwrap();
    let tokens = ["rare".into(), "β".into(), "rare".into()];
    let (index, hash): (Vec<u8>, [u8; 32]) = build_postings(&rows, &anchor, &tokens).unwrap();
    assert_eq!(hash, sha(&index));
    assert_eq!(
        build_postings(&rows, &anchor, &tokens).unwrap(),
        (index.clone(), hash)
    );
    assert_eq!(
        build_postings(&rows, &anchor, &["rare".into(), "β".into()]).unwrap(),
        (index.clone(), hash)
    );
    let mut wrong_count = anchor.clone();
    wrong_count.rows += 1;
    assert!(build_postings(&rows, &wrong_count, &tokens).is_err());

    let other_rows = vec![log(99, 1, 0, "rare")];
    let (_, other_anchor) =
        encode(&other_rows, NonZeroUsize::new(1).unwrap(), Codec::Plain).unwrap();
    let (stale, stale_hash) = build_postings(&other_rows, &other_anchor, &["rare".into()]).unwrap();
    let mut tampered = index.clone();
    let tampered_at = tampered.len() / 2;
    tampered[tampered_at] ^= 1;
    let malformed = b"{not valid JSON".as_slice();
    let missing_fields = b"{}".as_slice();
    for query in queries() {
        let expected = oracle(&rows, &query);
        assert_eq!(
            query_postings(&bytes, &anchor, &query, Some((&index, hash))).unwrap(),
            expected
        );
        for invalid in [
            Some((tampered.as_slice(), hash)),
            Some((index.as_slice(), [0; 32])),
            Some((stale.as_slice(), stale_hash)),
            Some((malformed, sha(malformed))),
            Some((missing_fields, sha(missing_fields))),
        ] {
            assert_eq!(
                query_postings(&bytes, &anchor, &query, invalid).unwrap(),
                expected
            );
        }
    }
}

#[test]
fn postings_cannot_hide_missing_or_wrong_raw_table() {
    let rows = edge_rows();
    let (bytes, anchor) = encode(&rows, NonZeroUsize::new(3).unwrap(), Codec::Plain).unwrap();
    let (index, hash) = build_postings(&rows, &anchor, &["rare".into()]).unwrap();
    for query in [q(5, 4, None, None), q(-1, 3, Some(1), Some("rare"))] {
        assert!(query_postings(&[], &anchor, &query, Some((&index, hash))).is_err());
        let mut bad = bytes.clone();
        bad[0] ^= 1;
        assert!(query_postings(&bad, &anchor, &query, Some((&index, hash))).is_err());
    }
}
