//! Hybrid Parquet event storage with authenticated projected queries.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    num::NonZeroUsize,
};

use arrow_array::{
    builder::FixedSizeBinaryBuilder, Array, BinaryArray, Int64Array, RecordBatch, StringArray,
    UInt64Array, UInt8Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use bytes::Bytes;
use fabric_o11y::{Event, Payload};
use parquet::{
    arrow::{
        arrow_reader::ParquetRecordBatchReaderBuilder, arrow_writer::ArrowWriter, ProjectionMask,
    },
    basic::{Compression, ZstdLevel},
    file::{metadata::KeyValue, properties::WriterProperties},
};
use serde::{
    de::{self, MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use sha2::{Digest as _, Sha256};
use storage_probe::{coverage::Digest, disk, Query};

const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Codec {
    Plain,
    Zstd,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableAnchor {
    pub sha256: Digest,
    pub rows: usize,
    pub version: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hit {
    pub position: usize,
    pub digest: Digest,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn schema() -> SchemaRef {
    SchemaRef::new(Schema::new(vec![
        Field::new("event_time", DataType::Int64, false),
        Field::new("tenant", DataType::UInt64, false),
        Field::new("payload_kind", DataType::UInt8, false),
        Field::new("log_body", DataType::Utf8, true),
        Field::new("raw_event", DataType::Binary, false),
        Field::new("row_digest", DataType::FixedSizeBinary(32), false),
    ]))
}

fn sha(bytes: &[u8]) -> Digest {
    Sha256::digest(bytes).into()
}

fn make_batch(rows: &[Event]) -> io::Result<RecordBatch> {
    let event_time = Int64Array::from_iter_values(rows.iter().map(|row| row.event_time.0));
    let tenant = UInt64Array::from_iter_values(rows.iter().map(|row| row.tenant.0));
    let payload_kind = UInt8Array::from_iter_values(rows.iter().map(|row| match row.payload {
        Payload::Log { .. } => 0,
        Payload::Gauge { .. } => 1,
    }));
    let log_body = StringArray::from(
        rows.iter()
            .map(|row| match &row.payload {
                Payload::Log { body } => Some(body.as_str()),
                Payload::Gauge { .. } => None,
            })
            .collect::<Vec<_>>(),
    );
    let raw_events = rows
        .iter()
        .map(|row| disk::encode_events(std::slice::from_ref(row)))
        .collect::<io::Result<Vec<_>>>()?;
    let raw_event = BinaryArray::from_iter_values(raw_events.iter().map(Vec::as_slice));
    let mut row_digest = FixedSizeBinaryBuilder::new(32);
    for row in rows {
        row_digest
            .append_value(storage_probe::coverage::rows_digest(std::slice::from_ref(
                row,
            )))
            .map_err(|error| invalid(error.to_string()))?;
    }
    let row_digest = row_digest.finish();
    RecordBatch::try_new(
        schema(),
        vec![
            std::sync::Arc::new(event_time),
            std::sync::Arc::new(tenant),
            std::sync::Arc::new(payload_kind),
            std::sync::Arc::new(log_body),
            std::sync::Arc::new(raw_event),
            std::sync::Arc::new(row_digest),
        ],
    )
    .map_err(|error| invalid(error.to_string()))
}

pub fn encode(
    rows: &[Event],
    row_group: NonZeroUsize,
    codec: Codec,
) -> io::Result<(Vec<u8>, TableAnchor)> {
    let properties = WriterProperties::builder()
        .set_max_row_group_row_count(Some(row_group.get()))
        .set_compression(match codec {
            Codec::Plain => Compression::UNCOMPRESSED,
            Codec::Zstd => Compression::ZSTD(
                ZstdLevel::try_new(3).map_err(|error| invalid(error.to_string()))?,
            ),
        })
        .set_key_value_metadata(Some(vec![KeyValue::new(
            "fabric.layout.version".into(),
            Some(VERSION.to_string()),
        )]))
        .build();
    let batch = make_batch(rows)?;
    let mut bytes = Vec::new();
    {
        let mut writer = ArrowWriter::try_new(&mut bytes, schema(), Some(properties))
            .map_err(|error| invalid(error.to_string()))?;
        writer
            .write(&batch)
            .map_err(|error| invalid(error.to_string()))?;
        writer.close().map_err(|error| invalid(error.to_string()))?;
    }
    let anchor = TableAnchor {
        sha256: sha(&bytes),
        rows: rows.len(),
        version: VERSION,
    };
    Ok((bytes, anchor))
}

fn authenticated_builder(
    bytes: &[u8],
    expected: &TableAnchor,
) -> io::Result<ParquetRecordBatchReaderBuilder<Bytes>> {
    if expected.version != VERSION {
        return Err(invalid("unsupported table version"));
    }
    if sha(bytes) != expected.sha256 {
        return Err(invalid("table hash mismatch"));
    }
    let builder = ParquetRecordBatchReaderBuilder::try_new(Bytes::copy_from_slice(bytes))
        .map_err(|error| invalid(error.to_string()))?;
    if builder.schema().fields() != schema().fields() {
        return Err(invalid("unexpected Arrow fields"));
    }
    let metadata = builder.metadata();
    if metadata.file_metadata().num_rows() != i64::try_from(expected.rows).unwrap_or(-1) {
        return Err(invalid("table row count mismatch"));
    }
    let version = metadata
        .file_metadata()
        .key_value_metadata()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.key == "fabric.layout.version")
        })
        .and_then(|item| item.value.as_deref());
    if version != Some("1") {
        return Err(invalid("missing or unsupported Parquet layout version"));
    }
    Ok(builder)
}

fn array<'a, A: Array + 'static>(batch: &'a RecordBatch, index: usize) -> io::Result<&'a A> {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<A>()
        .ok_or_else(|| invalid(format!("unexpected array type at column {index}")))
}

fn projected_hits(
    bytes: &[u8],
    expected: &TableAnchor,
    query: &Query,
    candidates: Option<&BTreeSet<usize>>,
) -> io::Result<Vec<Hit>> {
    let builder = authenticated_builder(bytes, expected)?;
    let mask = ProjectionMask::roots(builder.parquet_schema(), [0, 1, 2, 3, 5]);
    let reader = builder
        .with_projection(mask)
        .build()
        .map_err(|error| invalid(error.to_string()))?;
    let mut hits = Vec::new();
    let mut position = 0usize;
    for batch in reader {
        let batch = batch.map_err(|error| invalid(error.to_string()))?;
        let times = array::<Int64Array>(&batch, 0)?;
        let tenants = array::<UInt64Array>(&batch, 1)?;
        let kinds = array::<UInt8Array>(&batch, 2)?;
        let bodies = array::<StringArray>(&batch, 3)?;
        let digests = array::<arrow_array::FixedSizeBinaryArray>(&batch, 4)?;
        for offset in 0..batch.num_rows() {
            let current = position;
            position = position
                .checked_add(1)
                .ok_or_else(|| invalid("row position overflow"))?;
            if times.is_null(offset)
                || tenants.is_null(offset)
                || kinds.is_null(offset)
                || digests.is_null(offset)
            {
                return Err(invalid("null in required projected column"));
            }
            let kind = kinds.value(offset);
            if !matches!(kind, 0 | 1) {
                return Err(invalid("invalid payload kind"));
            }
            if (kind == 0 && bodies.is_null(offset)) || (kind == 1 && !bodies.is_null(offset)) {
                return Err(invalid("payload kind/body nullness mismatch"));
            }
            if candidates.is_some_and(|positions| !positions.contains(&current)) {
                continue;
            }
            let event_time = times.value(offset);
            if event_time < query.start_ns || event_time > query.end_ns {
                continue;
            }
            let tenant = tenants.value(offset);
            if query.tenant.is_some_and(|expected| tenant != expected) {
                continue;
            }
            if let Some(token) = query.token.as_deref() {
                if kind != 0
                    || !bodies
                        .value(offset)
                        .split_whitespace()
                        .any(|part| part == token)
                {
                    continue;
                }
            }
            let digest: Digest = digests
                .value(offset)
                .try_into()
                .map_err(|_| invalid("invalid digest width"))?;
            hits.push(Hit {
                position: current,
                digest,
            });
        }
    }
    if position != expected.rows {
        return Err(invalid("projected row count mismatch"));
    }
    Ok(hits)
}

pub fn decode(bytes: &[u8], expected: &TableAnchor) -> io::Result<Vec<Event>> {
    let builder = authenticated_builder(bytes, expected)?;
    let reader = builder
        .build()
        .map_err(|error| invalid(error.to_string()))?;
    let mut decoded = Vec::with_capacity(expected.rows);
    let mut position = 0usize;
    for batch in reader {
        let batch = batch.map_err(|error| invalid(error.to_string()))?;
        let times = array::<Int64Array>(&batch, 0)?;
        let tenants = array::<UInt64Array>(&batch, 1)?;
        let kinds = array::<UInt8Array>(&batch, 2)?;
        let bodies = array::<StringArray>(&batch, 3)?;
        let raw = array::<BinaryArray>(&batch, 4)?;
        let digests = array::<arrow_array::FixedSizeBinaryArray>(&batch, 5)?;
        for offset in 0..batch.num_rows() {
            if times.is_null(offset)
                || tenants.is_null(offset)
                || kinds.is_null(offset)
                || raw.is_null(offset)
                || digests.is_null(offset)
            {
                return Err(invalid("null in required Parquet column"));
            }
            let kind = kinds.value(offset);
            if !matches!(kind, 0 | 1) {
                return Err(invalid("invalid payload kind"));
            }
            let body = match kind {
                0 if !bodies.is_null(offset) => Some(bodies.value(offset)),
                1 if bodies.is_null(offset) => None,
                _ => return Err(invalid("payload kind/body nullness mismatch")),
            };
            let events = disk::decode_events(raw.value(offset))?;
            if events.len() != 1 {
                return Err(invalid("raw event cell must contain exactly one event"));
            }
            let event = events
                .into_iter()
                .next()
                .ok_or_else(|| invalid("raw event cell is empty"))?;
            let expected_digest: Digest = digests
                .value(offset)
                .try_into()
                .map_err(|_| invalid("invalid digest width"))?;
            let actual_digest = storage_probe::coverage::rows_digest(std::slice::from_ref(&event));
            if expected_digest != actual_digest
                || event.event_time.0 != times.value(offset)
                || event.tenant.0 != tenants.value(offset)
                || match (&event.payload, body) {
                    (Payload::Log { body: raw_body }, Some(projected_body)) => {
                        kind != 0 || raw_body != projected_body
                    }
                    (Payload::Gauge { .. }, None) => kind != 1,
                    _ => true,
                }
            {
                return Err(invalid("raw event and projection disagree"));
            }
            decoded.push(event);
            position = position
                .checked_add(1)
                .ok_or_else(|| invalid("row count overflow"))?;
        }
    }
    if position != expected.rows {
        return Err(invalid("decoded row count mismatch"));
    }
    Ok(decoded)
}

fn matches_query(event: &Event, query: &Query) -> bool {
    if event.event_time.0 < query.start_ns || event.event_time.0 > query.end_ns {
        return false;
    }
    if query.tenant.is_some_and(|tenant| event.tenant.0 != tenant) {
        return false;
    }
    query
        .token
        .as_deref()
        .is_none_or(|token| match &event.payload {
            Payload::Log { body } => body.split_whitespace().any(|word| word == token),
            Payload::Gauge { .. } => false,
        })
}

pub fn query_full(bytes: &[u8], expected: &TableAnchor, query: &Query) -> io::Result<Vec<Hit>> {
    decode(bytes, expected).map(|rows| {
        rows.iter()
            .enumerate()
            .filter(|(_, event)| matches_query(event, query))
            .map(|(position, event)| Hit {
                position,
                digest: storage_probe::coverage::rows_digest(std::slice::from_ref(event)),
            })
            .collect()
    })
}

pub fn query_projected(
    bytes: &[u8],
    expected: &TableAnchor,
    query: &Query,
) -> io::Result<Vec<Hit>> {
    projected_hits(bytes, expected, query, None)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PostingsWire {
    version: u32,
    table: TableAnchor,
    tokens: Vec<String>,
    postings: UniquePostings,
}

#[derive(Serialize)]
struct UniquePostings(BTreeMap<String, Vec<usize>>);

impl<'de> Deserialize<'de> for UniquePostings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PostingsVisitor;
        impl<'de> Visitor<'de> for PostingsVisitor {
            type Value = UniquePostings;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a map from tokens to position arrays with unique keys")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, Vec<usize>>()? {
                    if values.insert(key, value).is_some() {
                        return Err(de::Error::custom("duplicate postings token"));
                    }
                }
                Ok(UniquePostings(values))
            }
        }
        deserializer.deserialize_map(PostingsVisitor)
    }
}

pub fn build_postings(
    rows: &[Event],
    table: &TableAnchor,
    tokens: &[String],
) -> io::Result<(Vec<u8>, Digest)> {
    if table.version != VERSION {
        return Err(invalid("unsupported table version"));
    }
    if rows.len() != table.rows {
        return Err(invalid("table row count does not match source rows"));
    }
    let token_set: BTreeSet<String> = tokens.iter().cloned().collect();
    let mut postings: BTreeMap<String, Vec<usize>> = token_set
        .iter()
        .map(|token| (token.clone(), Vec::new()))
        .collect();
    for (position, event) in rows.iter().enumerate() {
        if let Payload::Log { body } = &event.payload {
            let words: BTreeSet<&str> = body.split_whitespace().collect();
            for word in words {
                if let Some(positions) = postings.get_mut(word) {
                    positions.push(position);
                }
            }
        }
    }
    let wire = PostingsWire {
        version: VERSION,
        table: table.clone(),
        tokens: token_set.into_iter().collect(),
        postings: UniquePostings(postings),
    };
    let bytes = serde_json::to_vec(&wire).map_err(|error| invalid(error.to_string()))?;
    let digest = sha(&bytes);
    Ok((bytes, digest))
}

fn parse_postings(bytes: &[u8], digest: Digest, table: &TableAnchor) -> Option<PostingsWire> {
    if sha(bytes) != digest {
        return None;
    }
    let wire: PostingsWire = serde_json::from_slice(bytes).ok()?;
    if wire.version != VERSION || wire.table != *table {
        return None;
    }
    if wire.tokens.windows(2).any(|pair| pair[0] >= pair[1]) {
        return None;
    }
    if wire.postings.0.len() != wire.tokens.len()
        || wire
            .tokens
            .iter()
            .any(|token| !wire.postings.0.contains_key(token))
    {
        return None;
    }
    for positions in wire.postings.0.values() {
        if positions.iter().any(|position| *position >= table.rows)
            || positions.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return None;
        }
    }
    Some(wire)
}

pub fn query_postings(
    bytes: &[u8],
    table: &TableAnchor,
    query: &Query,
    index: Option<(&[u8], Digest)>,
) -> io::Result<Vec<Hit>> {
    // Authenticate and validate the raw file regardless of whether an index might answer empty.
    let parsed = index.and_then(|(index_bytes, digest)| parse_postings(index_bytes, digest, table));
    let candidates = query
        .token
        .as_ref()
        .and_then(|token| parsed.as_ref()?.postings.0.get(token))
        .map(|positions| positions.iter().copied().collect::<BTreeSet<_>>());
    projected_hits(bytes, table, query, candidates.as_ref())
}
