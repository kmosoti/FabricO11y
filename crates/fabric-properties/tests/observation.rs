//! Properties of the FOB1 observation codec, stated from its contract: every
//! valid block has one encoding, every accepted byte string is that encoding
//! of its decoding, and no input panics the decoder.

use fabric_observation::{
    Bits, Locators, MAX_SEVERITY, Number, Observation, PointKind, Signal, SpanKind, Status, Strand,
    Value, decode, decode_view, encode,
};
use proptest::prelude::*;
use std::collections::BTreeMap;

fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => "[a-z.]{0,12}",
        1 => any::<String>(),
        1 => Just(String::new()),
    ]
}

fn finite_bits() -> impl Strategy<Value = Bits> {
    any::<u64>().prop_filter_map("finite", |u| {
        let d = f64::from_bits(u);
        (!d.is_nan()).then_some(Bits(u))
    })
}

fn value() -> impl Strategy<Value = Value> {
    prop_oneof![
        text().prop_map(Value::Str),
        any::<i64>().prop_map(Value::Int),
        finite_bits().prop_map(Value::Double),
        any::<bool>().prop_map(Value::Bool),
        proptest::collection::vec(any::<u8>(), 0..24).prop_map(Value::Bytes),
    ]
}

fn attributes() -> impl Strategy<Value = Vec<(String, Value)>> {
    proptest::collection::btree_map(text(), value(), 0..5)
        .prop_map(|m: BTreeMap<String, Value>| m.into_iter().collect())
}

fn number() -> impl Strategy<Value = Number> {
    prop_oneof![
        any::<i64>().prop_map(Number::Int),
        finite_bits().prop_map(|b| Number::Double(b.to_f64())),
    ]
}

fn signal() -> impl Strategy<Value = Signal> {
    prop_oneof![
        (0..=MAX_SEVERITY, text(), text()).prop_map(|(severity, event, body)| Signal::Log {
            severity,
            event,
            body
        }),
        (
            text(),
            text(),
            prop_oneof![
                Just(PointKind::Gauge),
                (any::<bool>(), any::<u64>()).prop_map(|(monotonic, start_ns)| PointKind::Sum {
                    monotonic,
                    start_ns
                })
            ],
            number()
        )
            .prop_map(|(name, unit, kind, value)| Signal::Point {
                name,
                unit,
                kind,
                value
            }),
        (
            text(),
            any::<u64>(),
            prop_oneof![Just(Status::Unset), Just(Status::Ok), Just(Status::Error)],
            prop_oneof![
                Just(SpanKind::Unspecified),
                Just(SpanKind::Internal),
                Just(SpanKind::Server),
                Just(SpanKind::Client),
                Just(SpanKind::Producer),
                Just(SpanKind::Consumer)
            ]
        )
            .prop_map(|(name, end_ns, status, kind)| Signal::Span {
                name,
                end_ns,
                status,
                kind
            }),
    ]
}

fn locators() -> impl Strategy<Value = Option<Locators>> {
    proptest::option::of(
        (
            any::<[u8; 16]>(),
            any::<[u8; 8]>(),
            proptest::option::of(any::<[u8; 8]>()),
        )
            .prop_map(|(trace_id, span_id, parent_span_id)| Locators {
                trace_id,
                span_id,
                parent_span_id,
            }),
    )
}

fn observation() -> impl Strategy<Value = Observation> {
    (
        prop_oneof![Just([7_u8; 16]), Just([9_u8; 16]), any::<[u8; 16]>()],
        any::<u64>(),
        any::<u64>(),
        any::<u32>(),
        prop_oneof![
            1_800_000_000_000_000_000_u64..1_800_000_100_000_000_000,
            any::<u64>()
        ],
        locators(),
        attributes(),
        signal(),
    )
        .prop_map(
            |(node_id, generation, sequence, index, time_ns, locators, attributes, signal)| {
                Observation {
                    strand: Strand {
                        node_id,
                        generation,
                    },
                    sequence,
                    index,
                    time_ns,
                    locators,
                    attributes,
                    signal,
                }
            },
        )
}

fn block() -> impl Strategy<Value = Vec<Observation>> {
    proptest::collection::vec(observation(), 1..40)
}

fn recrc(mut bytes: Vec<u8>) -> Vec<u8> {
    let n = bytes.len() - 4;
    let crc = crc32fast_hash(&bytes[..n]);
    bytes.truncate(n);
    bytes.extend_from_slice(&crc.to_le_bytes());
    bytes
}

// A local CRC-32 (IEEE) so this crate adds no dependency; must equal crc32fast::hash.
fn crc32fast_hash(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for b in data {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_500))]

    /// Every valid block decodes to itself and has exactly the bytes it had,
    /// through the owned decoder and through the borrowing view.
    #[test]
    fn round_trip_is_identity(block in block()) {
        let bytes = encode(&block).unwrap();
        let back = decode(&bytes).unwrap();
        prop_assert_eq!(&back, &block);
        prop_assert_eq!(&encode(&back).unwrap(), &bytes);
        let view = decode_view(&bytes).unwrap();
        let owned: Vec<Observation> = view.iter().map(|r| r.to_owned()).collect();
        prop_assert_eq!(&owned, &block);
    }

    /// Canonicality under mutation: with the CRC repaired, any accepted byte
    /// string re-encodes to exactly itself, and nothing panics.
    #[test]
    fn mutations_are_rejected_or_canonical(block in block(), flips in proptest::collection::vec((any::<usize>(), 0_u8..8), 1..4)) {
        let bytes = encode(&block).unwrap();
        let mut m = bytes.clone();
        let body = m.len() - 4;
        for (at, bit) in flips {
            m[at % body] ^= 1 << bit;
        }
        let m = recrc(m);
        if let Ok(decoded) = decode(&m) {
            prop_assert_eq!(encode(&decoded).unwrap(), m);
        }
    }

    /// Arbitrary bytes never panic the decoder, and whatever it accepts is canonical.
    #[test]
    fn arbitrary_bytes_never_panic(data in proptest::collection::vec(any::<u8>(), 0..256)) {
        if let Ok(decoded) = decode(&data) {
            prop_assert_eq!(encode(&decoded).unwrap(), data);
        }
    }

    /// The CRC helper used above agrees with the codec's (a flipped CRC is the one mutation the codec must catch).
    #[test]
    fn crc_mismatch_is_always_caught(block in block(), bit in 0_u8..32) {
        let mut bytes = encode(&block).unwrap();
        let n = bytes.len();
        bytes[n - 4 + usize::from(bit / 8)] ^= 1 << (bit % 8);
        prop_assert!(decode(&bytes).is_err());
    }
}
