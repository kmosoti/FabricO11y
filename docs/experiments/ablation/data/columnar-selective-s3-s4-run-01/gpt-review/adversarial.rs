//! Independent review probes; this file exists only in review scratch.
use std::num::NonZeroUsize;
use fabric_o11y::{Attribute, Event, EventId, EventTime, ObservedTime, Payload, ResourceId, Scalar, SourceId, TenantId};
use layout_probe::{build_postings, decode, encode, query_full, query_postings, query_projected, Codec, Hit};
use storage_probe::{coverage, Query};

fn event(i: u64, t: i64, body: &str) -> Event {
    Event {
        id: EventId(i), tenant: TenantId(i % 3), source: SourceId(u64::MAX - i),
        resource: ResourceId(i), event_time: EventTime(t), observed_time: ObservedTime(-t),
        attributes: vec![], payload: Payload::Log { body: body.into() },
    }
}

fn oracle(rows: &[Event], q: &Query) -> Vec<Hit> {
    rows.iter().enumerate().filter_map(|(position, row)| {
        let token_ok = q.token.as_deref().is_none_or(|token| match &row.payload {
            Payload::Log { body } => body.split_whitespace().any(|word| word == token),
            Payload::Gauge { .. } => false,
        });
        (row.event_time.0 >= q.start_ns && row.event_time.0 <= q.end_ns
            && q.tenant.is_none_or(|tenant| row.tenant.0 == tenant) && token_ok)
            .then(|| Hit { position, digest: coverage::rows_digest(std::slice::from_ref(row)) })
    }).collect()
}

#[test]
fn float_bits_survive_without_digest_as_proxy() {
    let patterns = [0x7ff0_0000_0000_0001, 0x7ff8_0000_0000_0001,
        0xfff0_0000_0000_0001, 0x8000_0000_0000_0000, 0x0000_0000_0000_0000,
        0x7ff0_0000_0000_0000, 0xfff0_0000_0000_0000];
    let rows: Vec<_> = patterns.iter().enumerate().map(|(i, bits)| {
        let mut row = event(i as u64, i as i64, "α\u{2003}β");
        row.attributes = vec![Attribute { key: "duplicate".into(), value: Scalar::F64(f64::from_bits(*bits)) },
            Attribute { key: "duplicate".into(), value: Scalar::F64(f64::from_bits(!bits)) }];
        row.payload = Payload::Gauge { name: "".into(), value: f64::from_bits(*bits), unit: "雪".into() };
        row
    }).collect();
    for codec in [Codec::Plain, Codec::Zstd] {
        let (bytes, anchor) = encode(&rows, NonZeroUsize::new(2).unwrap(), codec).unwrap();
        let back = decode(&bytes, &anchor).unwrap();
        for (i, row) in back.iter().enumerate() {
            assert_eq!(row.attributes.len(), 2);
            for (j, expected_bits) in [patterns[i], !patterns[i]].iter().enumerate() {
                assert_eq!(row.attributes[j].key, "duplicate");
                match row.attributes[j].value { Scalar::F64(x) => assert_eq!(x.to_bits(), *expected_bits), _ => panic!("scalar variant changed") }
            }
            match &row.payload { Payload::Gauge { name, value, unit } => {
                assert_eq!(name, ""); assert_eq!(unit, "雪"); assert_eq!(value.to_bits(), patterns[i]);
            }, _ => panic!("payload variant changed") }
        }
    }
}

#[test]
fn query_matrix_including_empty_and_unregistered_token() {
    let bodies = ["rare β", "rarely", "β\u{2003}rare", "RARE", "", "rare\u{00a0}rare"];
    let mut rows: Vec<_> = (0..54).map(|i| event(i as u64, i as i64 % 7 - 3, bodies[i % bodies.len()])).collect();
    rows[17].payload = Payload::Gauge { name: "rare".into(), value: -0.0, unit: "".into() };
    rows.push(event(0, -3, "rare β"));
    for codec in [Codec::Plain, Codec::Zstd] {
        let (bytes, anchor) = encode(&rows, NonZeroUsize::new(7).unwrap(), codec).unwrap();
        let (index, hash) = build_postings(&rows, &anchor, &["rare".into(), "β".into()]).unwrap();
        for start in [-4, -3, -1, 0, 2, 3, 4] {
            for end in [-4, -3, 0, 2, 3, 4] {
                for tenant in [None, Some(0), Some(1), Some(2), Some(u64::MAX)] {
                    for token in [None, Some("rare"), Some("β"), Some("RARE"), Some(""), Some("rarely")] {
                        let q = Query { start_ns: start, end_ns: end, tenant, token: token.map(str::to_owned) };
                        let expected = oracle(&rows, &q);
                        assert_eq!(query_full(&bytes, &anchor, &q).unwrap(), expected, "full {q:?}");
                        assert_eq!(query_projected(&bytes, &anchor, &q).unwrap(), expected, "projected {q:?}");
                        assert_eq!(query_postings(&bytes, &anchor, &q, Some((&index, hash))).unwrap(), expected, "postings {q:?}");
                    }
                }
            }
        }
    }
}
