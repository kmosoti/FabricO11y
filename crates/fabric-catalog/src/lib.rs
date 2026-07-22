//! Single-writer SQLite catalog for immutable segment metadata and maintained heads.

use std::path::{Component, Path};
use std::time::Duration;

use fabric_schema::EvidenceRecord;
use fabric_segment::SegmentManifest;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const CATALOG_VERSION: i64 = 1;
const APPLICATION_ID: i64 = 1_179_801_713; // ASCII-ish "FO11" namespace marker.

pub struct Catalog {
    connection: Connection,
}

impl Catalog {
    pub fn open(path: &Path) -> Result<Self, CatalogError> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(CatalogError::io)?;
        }
        let connection = Connection::open(path).map_err(CatalogError::sql)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(CatalogError::sql)?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 PRAGMA journal_mode = DELETE;
                 PRAGMA synchronous = FULL;",
            )
            .map_err(CatalogError::sql)?;
        migrate(&connection)?;
        Ok(Self { connection })
    }

    #[must_use]
    pub const fn schema_version() -> i64 {
        CATALOG_VERSION
    }

    /// Register a sealed segment and its searchable record metadata in one transaction.
    pub fn register_segment(
        &mut self,
        manifest: &SegmentManifest,
        relative_path: &str,
        records: &[EvidenceRecord],
    ) -> Result<Registration, CatalogError> {
        validate_relative_segment_path(relative_path)?;
        manifest.validate_records(records).map_err(|error| {
            CatalogError::Disagreement(format!(
                "segment {} disagrees with supplied records: {error}",
                manifest.segment_id
            ))
        })?;
        let expected_segment = CatalogSegment::expected(relative_path, manifest)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(CatalogError::sql)?;
        let existing = transaction
            .query_row(
                "SELECT segment_id, relative_path, manifest_json, format_version, schema_set_id,
                        row_count, created_at, recorded_at_min, recorded_at_max, payload_sha256,
                        content_sha256, dictionary_family, dictionary_version, dictionary_digest
                 FROM segments WHERE segment_id = ?1",
                [&manifest.segment_id],
                catalog_segment_from_row,
            )
            .optional()
            .map_err(CatalogError::sql)?;

        let inserted = if let Some(existing) = existing {
            if existing != expected_segment {
                return Err(CatalogError::Disagreement(format!(
                    "catalog metadata disagrees for segment {}",
                    manifest.segment_id
                )));
            }
            false
        } else {
            transaction
                .execute(
                    "INSERT INTO segments (
                        segment_id, relative_path, manifest_json, format_version, schema_set_id,
                        row_count, created_at, recorded_at_min, recorded_at_max,
                        payload_sha256, content_sha256, dictionary_family, dictionary_version,
                        dictionary_digest
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
                     )",
                    params![
                        expected_segment.segment_id,
                        expected_segment.relative_path,
                        expected_segment.manifest_json,
                        expected_segment.format_version,
                        expected_segment.schema_set_id,
                        expected_segment.row_count,
                        expected_segment.created_at,
                        expected_segment.recorded_at_min,
                        expected_segment.recorded_at_max,
                        expected_segment.payload_sha256,
                        expected_segment.content_sha256,
                        expected_segment.dictionary_family,
                        expected_segment.dictionary_version,
                        expected_segment.dictionary_digest,
                    ],
                )
                .map_err(CatalogError::sql)?;
            true
        };

        for (row_index, record) in records.iter().enumerate() {
            insert_record_metadata(
                &transaction,
                manifest,
                i64::try_from(row_index).map_err(|_| {
                    CatalogError::Disagreement("row index exceeds SQLite integer".to_owned())
                })?,
                record,
            )?;
        }
        let registered_rows: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM records WHERE segment_id = ?1",
                [&manifest.segment_id],
                |row| row.get(0),
            )
            .map_err(CatalogError::sql)?;
        if registered_rows
            != i64::try_from(manifest.row_count).map_err(|_| {
                CatalogError::Disagreement("manifest row count exceeds SQLite integer".to_owned())
            })?
        {
            return Err(CatalogError::Disagreement(format!(
                "catalog has {registered_rows} rows for segment {} but manifest declares {}",
                manifest.segment_id, manifest.row_count
            )));
        }
        transaction.commit().map_err(CatalogError::sql)?;
        Ok(if inserted {
            Registration::Inserted
        } else {
            Registration::AlreadyPresent
        })
    }

    /// Atomically replace the maintained observation-head projection.
    pub fn replace_observation_heads(
        &mut self,
        heads: &[ObservationHead],
    ) -> Result<(), CatalogError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(CatalogError::sql)?;
        transaction
            .execute("DELETE FROM observation_heads", [])
            .map_err(CatalogError::sql)?;
        for head in heads {
            transaction
                .execute(
                    "INSERT INTO observation_heads (
                        observation_id, disposition_json, qualifications_json, correction_ids_json
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        head.observation_id,
                        canonical_json(&head.disposition)?,
                        canonical_json(&head.qualifications)?,
                        canonical_json(&head.correction_ids)?,
                    ],
                )
                .map_err(CatalogError::sql)?;
        }
        transaction.commit().map_err(CatalogError::sql)
    }

    pub fn segments(&self) -> Result<Vec<CatalogSegment>, CatalogError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT segment_id, relative_path, manifest_json, format_version, schema_set_id,
                        row_count, created_at, recorded_at_min, recorded_at_max, payload_sha256,
                        content_sha256, dictionary_family, dictionary_version, dictionary_digest
                 FROM segments ORDER BY segment_id",
            )
            .map_err(CatalogError::sql)?;
        let rows = statement
            .query_map([], catalog_segment_from_row)
            .map_err(CatalogError::sql)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(CatalogError::sql)
    }

    pub fn record_location(&self, record_id: &str) -> Result<Option<RecordLocation>, CatalogError> {
        self.connection
            .query_row(
                "SELECT record_id, record_kind, segment_id, row_index, producer_id, stream_id,
                        recorded_at, classification
                 FROM records WHERE record_id = ?1",
                [record_id],
                |row| {
                    Ok(RecordLocation {
                        record_id: row.get(0)?,
                        record_kind: row.get(1)?,
                        segment_id: row.get(2)?,
                        row_index: row.get(3)?,
                        producer_id: row.get(4)?,
                        stream_id: row.get(5)?,
                        recorded_at: row.get(6)?,
                        classification: row.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(CatalogError::sql)
    }

    pub fn snapshot(&self) -> Result<CatalogSnapshot, CatalogError> {
        let segments = self.segments()?;
        let mut record_statement = self
            .connection
            .prepare(
                "SELECT record_id, record_kind, segment_id, row_index, producer_id, stream_id,
                        recorded_at, classification
                 FROM records ORDER BY record_id",
            )
            .map_err(CatalogError::sql)?;
        let records = record_statement
            .query_map([], |row| {
                Ok(RecordLocation {
                    record_id: row.get(0)?,
                    record_kind: row.get(1)?,
                    segment_id: row.get(2)?,
                    row_index: row.get(3)?,
                    producer_id: row.get(4)?,
                    stream_id: row.get(5)?,
                    recorded_at: row.get(6)?,
                    classification: row.get(7)?,
                })
            })
            .map_err(CatalogError::sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(CatalogError::sql)?;
        let mut head_statement = self
            .connection
            .prepare(
                "SELECT observation_id, disposition_json, qualifications_json,
                        correction_ids_json
                 FROM observation_heads ORDER BY observation_id",
            )
            .map_err(CatalogError::sql)?;
        let heads = head_statement
            .query_map([], |row| {
                Ok(ObservationHeadRow {
                    observation_id: row.get(0)?,
                    disposition_json: row.get(1)?,
                    qualifications_json: row.get(2)?,
                    correction_ids_json: row.get(3)?,
                })
            })
            .map_err(CatalogError::sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(CatalogError::sql)?;
        Ok(CatalogSnapshot {
            segments,
            records,
            heads,
        })
    }

    pub fn integrity_check(&self) -> Result<(), CatalogError> {
        let result: String = self
            .connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(CatalogError::sql)?;
        if result == "ok" {
            Ok(())
        } else {
            Err(CatalogError::Integrity(result))
        }
    }
}

fn migrate(connection: &Connection) -> Result<(), CatalogError> {
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(CatalogError::sql)?;
    if application_id != 0 && application_id != APPLICATION_ID {
        return Err(CatalogError::Migration(format!(
            "unexpected SQLite application_id {application_id}"
        )));
    }
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(CatalogError::sql)?;
    if version > CATALOG_VERSION {
        return Err(CatalogError::Migration(format!(
            "catalog version {version} is newer than supported version {CATALOG_VERSION}"
        )));
    }
    if version == 0 {
        connection
            .execute_batch(&format!(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS segments (
                    segment_id TEXT PRIMARY KEY,
                    relative_path TEXT NOT NULL UNIQUE,
                    manifest_json TEXT NOT NULL,
                    format_version INTEGER NOT NULL,
                    schema_set_id TEXT NOT NULL,
                    row_count INTEGER NOT NULL CHECK (row_count > 0),
                    created_at TEXT NOT NULL,
                    recorded_at_min TEXT,
                    recorded_at_max TEXT,
                    payload_sha256 TEXT NOT NULL,
                    content_sha256 TEXT NOT NULL,
                    dictionary_family TEXT,
                    dictionary_version INTEGER,
                    dictionary_digest TEXT,
                    CHECK ((dictionary_family IS NULL AND dictionary_version IS NULL AND dictionary_digest IS NULL)
                        OR (dictionary_family IS NOT NULL AND dictionary_version IS NOT NULL AND dictionary_digest IS NOT NULL))
                 ) STRICT;
                 CREATE TABLE IF NOT EXISTS records (
                    record_id TEXT PRIMARY KEY,
                    record_kind TEXT NOT NULL CHECK (record_kind IN ('observation', 'correction', 'frontier')),
                    segment_id TEXT NOT NULL REFERENCES segments(segment_id) ON DELETE RESTRICT,
                    row_index INTEGER NOT NULL CHECK (row_index >= 0),
                    producer_id TEXT NOT NULL,
                    stream_id TEXT NOT NULL,
                    recorded_at TEXT NOT NULL,
                    classification TEXT NOT NULL,
                    UNIQUE(segment_id, row_index)
                 ) STRICT;
                 CREATE INDEX IF NOT EXISTS records_source_recorded
                    ON records(producer_id, stream_id, recorded_at);
                 CREATE INDEX IF NOT EXISTS records_segment ON records(segment_id);
                 CREATE TABLE IF NOT EXISTS observation_heads (
                    observation_id TEXT PRIMARY KEY REFERENCES records(record_id) ON DELETE RESTRICT,
                    disposition_json TEXT NOT NULL,
                    qualifications_json TEXT NOT NULL,
                    correction_ids_json TEXT NOT NULL
                 ) STRICT;
                 PRAGMA application_id = {APPLICATION_ID};
                 PRAGMA user_version = {CATALOG_VERSION};
                 COMMIT;"
            ))
            .map_err(CatalogError::sql)?;
    }
    Ok(())
}

fn insert_record_metadata(
    transaction: &rusqlite::Transaction<'_>,
    manifest: &SegmentManifest,
    row_index: i64,
    record: &EvidenceRecord,
) -> Result<(), CatalogError> {
    let metadata = RecordMetadata::from(record);
    let existing = transaction
        .query_row(
            "SELECT segment_id, row_index, record_kind, producer_id, stream_id, recorded_at,
                    classification
             FROM records WHERE record_id = ?1",
            [&metadata.record_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(CatalogError::sql)?;
    if let Some((
        segment_id,
        existing_row,
        kind,
        producer_id,
        stream_id,
        recorded_at,
        classification,
    )) = existing
    {
        if segment_id == manifest.segment_id
            && existing_row == row_index
            && kind == metadata.kind
            && producer_id == metadata.producer_id
            && stream_id == metadata.stream_id
            && recorded_at == metadata.recorded_at
            && classification == metadata.classification
        {
            return Ok(());
        }
        return Err(CatalogError::DuplicateRecord(metadata.record_id.to_owned()));
    }
    transaction
        .execute(
            "INSERT INTO records (
                record_id, record_kind, segment_id, row_index, producer_id, stream_id,
                recorded_at, classification
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                metadata.record_id,
                metadata.kind,
                manifest.segment_id,
                row_index,
                metadata.producer_id,
                metadata.stream_id,
                metadata.recorded_at,
                metadata.classification,
            ],
        )
        .map_err(CatalogError::sql)?;
    Ok(())
}

struct RecordMetadata<'a> {
    record_id: &'a str,
    kind: &'static str,
    producer_id: &'a str,
    stream_id: &'a str,
    recorded_at: &'a str,
    classification: &'a str,
}

impl<'a> From<&'a EvidenceRecord> for RecordMetadata<'a> {
    fn from(record: &'a EvidenceRecord) -> Self {
        match record {
            EvidenceRecord::Observation(record) => Self {
                record_id: &record.observation_id,
                kind: "observation",
                producer_id: &record.producer.producer_id,
                stream_id: &record.producer.stream_id,
                recorded_at: record.recorded_at.as_str(),
                classification: &record.classification,
            },
            EvidenceRecord::Correction(record) => Self {
                record_id: &record.correction_id,
                kind: "correction",
                producer_id: &record.producer.producer_id,
                stream_id: &record.producer.stream_id,
                recorded_at: record.recorded_at.as_str(),
                classification: &record.classification,
            },
            EvidenceRecord::Frontier(record) => Self {
                record_id: &record.frontier_id,
                kind: "frontier",
                producer_id: &record.producer.producer_id,
                stream_id: &record.producer.stream_id,
                recorded_at: record.as_of_recorded_at.as_str(),
                classification: &record.classification,
            },
        }
    }
}

fn validate_relative_segment_path(path: &str) -> Result<(), CatalogError> {
    let mut components = Path::new(path).components();
    let Some(Component::Normal(_)) = components.next() else {
        return Err(CatalogError::InvalidRelativePath(path.to_owned()));
    };
    if components.next().is_some() || !path.ends_with(".fseg") {
        return Err(CatalogError::InvalidRelativePath(path.to_owned()));
    }
    Ok(())
}

fn canonical_json<T: Serialize + ?Sized>(value: &T) -> Result<String, CatalogError> {
    let value = serde_json::to_value(value).map_err(CatalogError::json)?;
    serde_json::to_string(&value).map_err(CatalogError::json)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Registration {
    Inserted,
    AlreadyPresent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObservationHead {
    pub observation_id: String,
    pub disposition: serde_json::Value,
    pub qualifications: Vec<String>,
    pub correction_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CatalogSegment {
    pub segment_id: String,
    pub relative_path: String,
    pub manifest_json: String,
    pub format_version: i64,
    pub schema_set_id: String,
    pub row_count: i64,
    pub created_at: String,
    pub recorded_at_min: Option<String>,
    pub recorded_at_max: Option<String>,
    pub payload_sha256: String,
    pub content_sha256: String,
    pub dictionary_family: Option<String>,
    pub dictionary_version: Option<i64>,
    pub dictionary_digest: Option<String>,
}

impl CatalogSegment {
    fn expected(relative_path: &str, manifest: &SegmentManifest) -> Result<Self, CatalogError> {
        let dictionary = manifest.compression.dictionary.as_ref();
        Ok(Self {
            segment_id: manifest.segment_id.clone(),
            relative_path: relative_path.to_owned(),
            manifest_json: canonical_json(manifest)?,
            format_version: i64::from(manifest.format_version),
            schema_set_id: manifest.schema_set_id.clone(),
            row_count: i64::try_from(manifest.row_count).map_err(|_| {
                CatalogError::Disagreement("manifest row count exceeds SQLite integer".to_owned())
            })?,
            created_at: manifest.created_at.to_string(),
            recorded_at_min: manifest
                .ordering_bounds
                .recorded_at
                .as_ref()
                .map(|range| range.min.to_string()),
            recorded_at_max: manifest
                .ordering_bounds
                .recorded_at
                .as_ref()
                .map(|range| range.max.to_string()),
            payload_sha256: manifest.integrity.payload_sha256.clone(),
            content_sha256: manifest.integrity.content_sha256.clone(),
            dictionary_family: dictionary.map(|value| value.family.clone()),
            dictionary_version: dictionary
                .map(|value| i64::try_from(value.version))
                .transpose()
                .map_err(|_| {
                    CatalogError::Disagreement(
                        "dictionary version exceeds SQLite integer".to_owned(),
                    )
                })?,
            dictionary_digest: dictionary.map(|value| value.digest.clone()),
        })
    }

    pub fn agrees_with(
        &self,
        relative_path: &str,
        manifest: &SegmentManifest,
    ) -> Result<bool, CatalogError> {
        Ok(self == &Self::expected(relative_path, manifest)?)
    }

    pub fn manifest(&self) -> Result<SegmentManifest, CatalogError> {
        serde_json::from_str(&self.manifest_json).map_err(CatalogError::json)
    }
}

fn catalog_segment_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogSegment> {
    Ok(CatalogSegment {
        segment_id: row.get(0)?,
        relative_path: row.get(1)?,
        manifest_json: row.get(2)?,
        format_version: row.get(3)?,
        schema_set_id: row.get(4)?,
        row_count: row.get(5)?,
        created_at: row.get(6)?,
        recorded_at_min: row.get(7)?,
        recorded_at_max: row.get(8)?,
        payload_sha256: row.get(9)?,
        content_sha256: row.get(10)?,
        dictionary_family: row.get(11)?,
        dictionary_version: row.get(12)?,
        dictionary_digest: row.get(13)?,
    })
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecordLocation {
    pub record_id: String,
    pub record_kind: String,
    pub segment_id: String,
    pub row_index: i64,
    pub producer_id: String,
    pub stream_id: String,
    pub recorded_at: String,
    pub classification: String,
}

impl RecordLocation {
    #[must_use]
    pub fn agrees_with(&self, segment_id: &str, row_index: i64, record: &EvidenceRecord) -> bool {
        let expected = RecordMetadata::from(record);
        self.record_id == expected.record_id
            && self.record_kind == expected.kind
            && self.segment_id == segment_id
            && self.row_index == row_index
            && self.producer_id == expected.producer_id
            && self.stream_id == expected.stream_id
            && self.recorded_at == expected.recorded_at
            && self.classification == expected.classification
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ObservationHeadRow {
    pub observation_id: String,
    pub disposition_json: String,
    pub qualifications_json: String,
    pub correction_ids_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CatalogSnapshot {
    pub segments: Vec<CatalogSegment>,
    pub records: Vec<RecordLocation>,
    pub heads: Vec<ObservationHeadRow>,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum CatalogError {
    #[error("catalog IO failed: {0}")]
    Io(String),
    #[error("SQLite operation failed: {0}")]
    Sql(String),
    #[error("catalog migration failed: {0}")]
    Migration(String),
    #[error("catalog and segment disagree: {0}")]
    Disagreement(String),
    #[error("record id already belongs to different archived evidence: {0}")]
    DuplicateRecord(String),
    #[error("invalid relative segment path: {0}")]
    InvalidRelativePath(String),
    #[error("catalog integrity check failed: {0}")]
    Integrity(String),
    #[error("catalog JSON failed: {0}")]
    Json(String),
}

impl CatalogError {
    fn io(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }

    fn sql(error: rusqlite::Error) -> Self {
        Self::Sql(error.to_string())
    }

    fn json(error: serde_json::Error) -> Self {
        Self::Json(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_schema::{Correction, Observation};
    use fabric_segment::{EncodeOptions, encode_segment};
    use fabric_time::Timestamp;
    use tempfile::tempdir;

    fn records() -> Vec<EvidenceRecord> {
        let observation: Observation = serde_json::from_str(include_str!(
            "../../../fixtures/golden/contracts/valid/observation.json"
        ))
        .expect("observation fixture");
        let correction: Correction = serde_json::from_str(include_str!(
            "../../../fixtures/golden/contracts/valid/correction.json"
        ))
        .expect("correction fixture");
        vec![
            EvidenceRecord::from(observation),
            EvidenceRecord::Correction(correction),
        ]
    }

    #[test]
    fn migration_registration_and_reopen_are_idempotent() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("catalog.sqlite3");
        let segment = encode_segment(
            &records(),
            &EncodeOptions::new(
                Timestamp::parse("2026-07-20T15:00:00Z").expect("fixture timestamp"),
            ),
        )
        .expect("encode segment");
        {
            let mut catalog = Catalog::open(&path).expect("open catalog");
            assert_eq!(
                catalog
                    .register_segment(&segment.manifest, "segment-example.fseg", &records())
                    .expect("register segment"),
                Registration::Inserted
            );
            assert_eq!(
                catalog
                    .register_segment(&segment.manifest, "segment-example.fseg", &records())
                    .expect("register segment again"),
                Registration::AlreadyPresent
            );
            catalog.integrity_check().expect("catalog integrity");
        }
        let catalog = Catalog::open(&path).expect("reopen catalog");
        assert_eq!(catalog.segments().expect("segments").len(), 1);
        assert_eq!(
            catalog
                .record_location("obs-0001")
                .expect("record lookup")
                .expect("record exists")
                .row_index,
            0
        );
    }

    #[test]
    fn clean_rebuild_snapshot_is_equivalent() {
        let directory = tempdir().expect("temporary directory");
        let segment = encode_segment(
            &records(),
            &EncodeOptions::new(
                Timestamp::parse("2026-07-20T15:00:00Z").expect("fixture timestamp"),
            ),
        )
        .expect("encode segment");
        let mut first =
            Catalog::open(&directory.path().join("first.sqlite3")).expect("first catalog");
        let mut second =
            Catalog::open(&directory.path().join("second.sqlite3")).expect("second catalog");
        first
            .register_segment(&segment.manifest, "segment.fseg", &records())
            .expect("first registration");
        second
            .register_segment(&segment.manifest, "segment.fseg", &records())
            .expect("second registration");
        assert_eq!(
            first.snapshot().expect("first snapshot"),
            second.snapshot().expect("second snapshot")
        );
    }

    #[test]
    fn duplicate_record_in_another_segment_fails_without_partial_commit() {
        let directory = tempdir().expect("temporary directory");
        let mut catalog =
            Catalog::open(&directory.path().join("catalog.sqlite3")).expect("catalog");
        let options = EncodeOptions::new(
            Timestamp::parse("2026-07-20T15:00:00Z").expect("fixture timestamp"),
        );
        let first = encode_segment(&records(), &options).expect("first segment");
        catalog
            .register_segment(&first.manifest, "first.fseg", &records())
            .expect("first registration");

        let mut changed = records();
        let EvidenceRecord::Observation(observation) = &mut changed[0] else {
            panic!("first record is observation");
        };
        observation.payload = serde_json::json!({"changed": true});
        let second = encode_segment(&changed, &options).expect("second segment");
        assert!(matches!(
            catalog.register_segment(&second.manifest, "second.fseg", &changed),
            Err(CatalogError::DuplicateRecord(_))
        ));
        assert_eq!(catalog.segments().expect("segments").len(), 1);
    }

    #[test]
    fn registration_rejects_manifest_metadata_from_different_records() {
        let directory = tempdir().expect("temporary directory");
        let mut catalog =
            Catalog::open(&directory.path().join("catalog.sqlite3")).expect("catalog");
        let mut segment = encode_segment(
            &records(),
            &EncodeOptions::new(
                Timestamp::parse("2026-07-20T15:00:00Z").expect("fixture timestamp"),
            ),
        )
        .expect("encode segment");
        segment.manifest.classifications =
            std::collections::BTreeSet::from(["fabric-review-probe".to_owned()]);

        assert!(matches!(
            catalog.register_segment(&segment.manifest, "segment.fseg", &records()),
            Err(CatalogError::Disagreement(_))
        ));
        assert!(catalog.segments().expect("segments").is_empty());
    }

    #[test]
    fn repeated_registration_checks_every_duplicated_segment_column() {
        let directory = tempdir().expect("temporary directory");
        let mut catalog =
            Catalog::open(&directory.path().join("catalog.sqlite3")).expect("catalog");
        let segment = encode_segment(
            &records(),
            &EncodeOptions::new(
                Timestamp::parse("2026-07-20T15:00:00Z").expect("fixture timestamp"),
            ),
        )
        .expect("encode segment");
        catalog
            .register_segment(&segment.manifest, "segment.fseg", &records())
            .expect("register segment");
        catalog
            .connection
            .execute("UPDATE segments SET row_count = 999", [])
            .expect("alter duplicated row count");

        assert!(matches!(
            catalog.register_segment(&segment.manifest, "segment.fseg", &records()),
            Err(CatalogError::Disagreement(_))
        ));
    }
}
