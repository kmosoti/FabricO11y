//! Integrator probes added after independent candidates, not the frozen oracle.
use arrow_array::{Array, BinaryArray, RecordBatch};
use bytes::Bytes;
use fabric_o11y::{
    Event, EventId, EventTime, ObservedTime, Payload, ResourceId, SourceId, TenantId,
};
use layout_probe::{self as layout, Codec, TableAnchor};
use parquet::{
    arrow::{ArrowWriter, arrow_reader::ParquetRecordBatchReaderBuilder},
    file::{metadata::KeyValue, properties::WriterProperties},
};
use sha2::{Digest as _, Sha256};
use std::{num::NonZeroUsize, sync::Arc};
use storage_probe::Query;

fn rows() -> Vec<Event> {
    (0..9)
        .map(|i| Event {
            id: EventId(i / 2),
            tenant: TenantId(i % 2),
            source: SourceId(1),
            resource: ResourceId(1),
            event_time: EventTime(i as i64 - 4),
            observed_time: ObservedTime(0),
            attributes: vec![],
            payload: Payload::Log {
                body: if i % 2 == 0 { "rare β rare" } else { "normal" }.into(),
            },
        })
        .collect()
}
fn query() -> Query {
    Query {
        start_ns: i64::MIN,
        end_ns: i64::MAX,
        tenant: None,
        token: Some("rare".into()),
    }
}
fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[test]
fn malformed_postings_with_matching_hash_still_fall_back() {
    let rows = rows();
    let (table, anchor) =
        layout::encode(&rows, NonZeroUsize::new(3).unwrap(), Codec::Zstd).unwrap();
    let (index, _) = layout::build_postings(&rows, &anchor, &["rare".into(), "β".into()]).unwrap();
    let base: serde_json::Value = serde_json::from_slice(&index).unwrap();
    let expected = layout::query_projected(&table, &anchor, &query()).unwrap();
    assert_eq!(
        expected.iter().map(|h| h.position).collect::<Vec<_>>(),
        vec![0, 2, 4, 6, 8]
    );
    let mut variants = vec![];
    for field in ["version", "table", "tokens", "postings"] {
        let mut v = base.clone();
        v.as_object_mut().unwrap().remove(field);
        variants.push(v);
    }
    let mut v = base.clone();
    v["version"] = 2.into();
    variants.push(v);
    let mut v = base.clone();
    v["unknown"] = true.into();
    variants.push(v);
    let mut v = base.clone();
    v["table"]["version"] = 2.into();
    variants.push(v);
    let mut v = base.clone();
    v["table"]["rows"] = 0.into();
    variants.push(v);
    let mut v = base.clone();
    v["table"]["sha256"][0] = ((anchor.sha256[0] ^ 1) as u64).into();
    variants.push(v);
    let mut v = base.clone();
    v["table"]["extra"] = 0.into();
    variants.push(v);
    for positions in [
        serde_json::json!([2, 0]),
        serde_json::json!([0, 0]),
        serde_json::json!([9]),
        serde_json::json!([-1]),
        serde_json::json!(["0"]),
    ] {
        let mut v = base.clone();
        v["postings"]["rare"] = positions;
        variants.push(v);
    }
    let mut v = base.clone();
    v["tokens"] = serde_json::json!(["β", "rare"]);
    variants.push(v);
    let mut v = base.clone();
    v["tokens"] = serde_json::json!(["rare", "rare"]);
    variants.push(v);
    let mut v = base.clone();
    v["postings"].as_object_mut().unwrap().remove("rare");
    variants.push(v);
    let mut v = base.clone();
    v["postings"]["extra"] = serde_json::json!([]);
    variants.push(v);
    for variant in variants {
        let bytes = serde_json::to_vec(&variant).unwrap();
        assert_eq!(
            layout::query_postings(&table, &anchor, &query(), Some((&bytes, digest(&bytes))))
                .unwrap(),
            expected
        );
    }
    let text = String::from_utf8(index).unwrap();
    for corrupted in [
        text.replacen("\"version\":1", "\"version\":1,\"version\":2", 1),
        text.replacen("\"postings\":{", "\"postings\":{\"rare\":[],", 1),
    ] {
        let bytes = corrupted.as_bytes();
        assert_eq!(
            layout::query_postings(&table, &anchor, &query(), Some((bytes, digest(bytes))))
                .unwrap(),
            expected
        );
    }
    let mut bad = anchor.clone();
    bad.sha256[0] ^= 1;
    let none = Query {
        token: Some("absent".into()),
        ..query()
    };
    assert!(layout::query_postings(&table, &bad, &none, None).is_err());
}

#[test]
fn projection_does_not_decode_raw_column_but_full_decode_checks_consistency() {
    let (bytes, anchor) =
        layout::encode(&rows(), NonZeroUsize::new(64).unwrap(), Codec::Plain).unwrap();
    let mut reader = ParquetRecordBatchReaderBuilder::try_new(Bytes::from(bytes.clone()))
        .unwrap()
        .build()
        .unwrap();
    let batch = reader.next().unwrap().unwrap();
    let raw = batch
        .column(4)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap();
    let changed = BinaryArray::from_iter_values((0..batch.num_rows()).map(|i| {
        if i == 0 {
            b"invalid-json".as_slice()
        } else {
            raw.value(i)
        }
    }));
    let mut columns = batch.columns().to_vec();
    columns[4] = Arc::new(changed);
    let altered = RecordBatch::try_new(batch.schema(), columns).unwrap();
    let properties = WriterProperties::builder()
        .set_key_value_metadata(Some(vec![KeyValue::new(
            "fabric.layout.version".into(),
            Some("1".into()),
        )]))
        .build();
    let mut output = vec![];
    let mut writer = ArrowWriter::try_new(&mut output, altered.schema(), Some(properties)).unwrap();
    writer.write(&altered).unwrap();
    writer.close().unwrap();
    // Deliberately mint a new anchor to test parser/projection behavior. This is
    // outside the retained-anchor security claim, not a forgery accepted by it.
    let altered_anchor = TableAnchor {
        sha256: digest(&output),
        ..anchor.clone()
    };
    assert!(layout::decode(&output, &altered_anchor).is_err());
    assert_eq!(
        layout::query_projected(&output, &altered_anchor, &query()).unwrap(),
        layout::query_full(&bytes, &anchor, &query()).unwrap()
    );
    assert!(layout::query_projected(&output, &anchor, &query()).is_err());
}
