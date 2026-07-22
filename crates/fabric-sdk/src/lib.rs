//! Stable, single-owner embedded engine over the deterministic semantic core.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fabric_catalog::{
    Catalog, CatalogError, CatalogSnapshot, ObservationHead, ObservationHeadRow, RecordLocation,
};
use fabric_core::{CoreError, EvidenceState};
use fabric_schema::{EvidenceRecord, SchemaError};
use fabric_segment::{
    DecodedSegment, DictionaryLocator, DictionaryResolver, EncodeOptions, SegmentError,
    SegmentErrorCategory, SegmentManifest, canonical_record_json, decode_segment_file,
    encode_segment, segment_id_for_records,
};
use fabric_time::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const SEGMENTS_DIRECTORY: &str = "segments";
const DICTIONARIES_DIRECTORY: &str = "dictionaries";
const CATALOG_FILE: &str = "catalog.sqlite3";
const STAGING_FILE: &str = "staging.jsonl";
const MAX_STAGING_BYTES: u64 = 268_435_456;

/// Admission policy is the explicit pre-persistence classification and redaction hook.
pub trait AdmissionPolicy: Send + Sync {
    fn classify_and_redact(
        &self,
        record: EvidenceRecord,
    ) -> Result<EvidenceRecord, AdmissionPolicyError>;
}

/// The default policy requires an existing classification and preserves already-redacted content.
#[derive(Clone, Copy, Debug, Default)]
pub struct RequireClassification;

impl AdmissionPolicy for RequireClassification {
    fn classify_and_redact(
        &self,
        record: EvidenceRecord,
    ) -> Result<EvidenceRecord, AdmissionPolicyError> {
        let classification = match &record {
            EvidenceRecord::Observation(record) => &record.classification,
            EvidenceRecord::Correction(record) => &record.classification,
            EvidenceRecord::Frontier(record) => &record.classification,
        };
        if classification.is_empty() {
            Err(AdmissionPolicyError::new("classification is required"))
        } else {
            Ok(record)
        }
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("admission policy rejected the record: {message}")]
pub struct AdmissionPolicyError {
    message: String,
}

impl AdmissionPolicyError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

pub trait AdmissionClock: Send + Sync {
    fn now(&self) -> Result<Timestamp, ClockError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl AdmissionClock for SystemClock {
    fn now(&self) -> Result<Timestamp, ClockError> {
        let value = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|error| ClockError(error.to_string()))?;
        Timestamp::parse(value).map_err(|error| ClockError(error.to_string()))
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("admission clock failed: {0}")]
pub struct ClockError(String);

impl ClockError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

struct DirectoryDictionaryResolver {
    directory: PathBuf,
}

impl DictionaryResolver for DirectoryDictionaryResolver {
    fn resolve(&self, locator: &DictionaryLocator) -> Result<Vec<u8>, SegmentError> {
        let filename = format!(
            "{}-v{}-{}.zdict",
            locator.family, locator.version, locator.digest
        );
        let path = self.directory.join(filename);
        fs::read(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SegmentError::MissingDictionary(locator.clone())
            } else {
                SegmentError::Io(error.to_string())
            }
        })
    }
}

#[derive(Clone, Debug)]
struct LoadedSegment {
    manifest: SegmentManifest,
    records: Vec<EvidenceRecord>,
}

/// A single-owner engine. Callers must externally serialize multiple processes using one root.
pub struct Engine {
    root: PathBuf,
    segments_directory: PathBuf,
    dictionaries_directory: PathBuf,
    staging_path: PathBuf,
    catalog: Catalog,
    sealed_segments: Vec<LoadedSegment>,
    staged_records: Vec<EvidenceRecord>,
    admission_policy: Box<dyn AdmissionPolicy>,
    clock: Box<dyn AdmissionClock>,
    recovery_report: RecoveryReport,
}

impl Engine {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, SdkError> {
        Self::open_with_components(root, RequireClassification, SystemClock)
    }

    pub fn open_with_components(
        root: impl AsRef<Path>,
        admission_policy: impl AdmissionPolicy + 'static,
        clock: impl AdmissionClock + 'static,
    ) -> Result<Self, SdkError> {
        let root = root.as_ref().to_path_buf();
        let segments_directory = root.join(SEGMENTS_DIRECTORY);
        let dictionaries_directory = root.join(DICTIONARIES_DIRECTORY);
        let staging_path = root.join(STAGING_FILE);
        fs::create_dir_all(&segments_directory).map_err(SdkError::io)?;
        fs::create_dir_all(&dictionaries_directory).map_err(SdkError::io)?;
        sync_directory(&root)?;

        let mut recovery_report = RecoveryReport {
            removed_partial_files: remove_partial_files(&segments_directory)?,
            ..RecoveryReport::default()
        };
        let mut catalog = Catalog::open(&root.join(CATALOG_FILE))?;
        let resolver = DirectoryDictionaryResolver {
            directory: dictionaries_directory.clone(),
        };
        let mut sealed_segments = Vec::new();
        let mut discovered_paths = BTreeMap::new();
        for path in segment_files(&segments_directory)? {
            let decoded = decode_segment_file(&path, &resolver)?;
            enforce_readonly(&path)?;
            let expected_filename = format!("{}.fseg", decoded.manifest.segment_id);
            let filename = file_name(&path)?;
            if filename != expected_filename {
                return Err(SdkError::CatalogDisagreement(format!(
                    "segment filename {filename} does not match identity {}",
                    decoded.manifest.segment_id
                )));
            }
            let registration =
                catalog.register_segment(&decoded.manifest, &filename, &decoded.records)?;
            if registration == fabric_catalog::Registration::Inserted {
                recovery_report.registered_orphan_segments += 1;
            }
            discovered_paths.insert(decoded.manifest.segment_id.clone(), filename.clone());
            sealed_segments.push(LoadedSegment {
                manifest: decoded.manifest,
                records: decoded.records,
            });
        }
        sealed_segments
            .sort_by(|left, right| left.manifest.segment_id.cmp(&right.manifest.segment_id));

        for entry in catalog.segments()? {
            match discovered_paths.get(&entry.segment_id) {
                Some(path) if path == &entry.relative_path => {}
                _ => {
                    return Err(SdkError::CatalogDisagreement(format!(
                        "catalog segment {} is missing from immutable storage",
                        entry.segment_id
                    )));
                }
            }
        }

        let mut staged_records = load_staging(&staging_path)?;
        if !staged_records.is_empty() {
            let staged_segment_id = segment_id_for_records(&staged_records)?;
            if discovered_paths.contains_key(&staged_segment_id) {
                clear_staging(&staging_path)?;
                staged_records.clear();
                recovery_report.cleared_redundant_staging = true;
            }
        }

        let sealed_state = EvidenceState::replay(flatten_records(&sealed_segments))?;
        catalog.replace_observation_heads(&observation_heads(&sealed_state)?)?;
        let mut complete_history = flatten_records(&sealed_segments);
        complete_history.extend(staged_records.iter().cloned());
        EvidenceState::replay(complete_history)?;

        Ok(Self {
            root,
            segments_directory,
            dictionaries_directory,
            staging_path,
            catalog,
            sealed_segments,
            staged_records,
            admission_policy: Box::new(admission_policy),
            clock: Box::new(clock),
            recovery_report,
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub const fn recovery_report(&self) -> &RecoveryReport {
        &self.recovery_report
    }

    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.staged_records.len()
    }

    /// Admit JSON after assigning durable admission time and running the policy hook.
    pub fn admit_json(&mut self, json: &str) -> Result<AdmissionReceipt, SdkError> {
        let mut value: Value = serde_json::from_str(json).map_err(SdkError::json)?;
        let object = value.as_object_mut().ok_or_else(|| {
            SdkError::AdmissionInvalid("evidence envelope must be a JSON object".to_owned())
        })?;
        let api_version = object
            .get("api_version")
            .and_then(Value::as_str)
            .ok_or_else(|| SdkError::AdmissionInvalid("api_version is required".to_owned()))?;
        let assigned = self.clock.now()?;
        let time_field = match api_version {
            "fabric.observation/v1" | "fabric.correction/v1" => "recorded_at",
            "fabric.frontier/v1" => "as_of_recorded_at",
            _ => {
                return Err(SdkError::AdmissionInvalid(format!(
                    "unsupported evidence envelope {api_version}"
                )));
            }
        };
        object.insert(time_field.to_owned(), Value::String(assigned.to_string()));
        let record: EvidenceRecord = serde_json::from_value(value)
            .map_err(|error| SdkError::SchemaInvalid(error.to_string()))?;
        self.admit_record_at(record, assigned)
    }

    /// Admit an already typed record. Its admission timestamp is replaced by the engine clock.
    pub fn admit(&mut self, record: EvidenceRecord) -> Result<AdmissionReceipt, SdkError> {
        let assigned = self.clock.now()?;
        self.admit_record_at(record, assigned)
    }

    fn admit_record_at(
        &mut self,
        mut record: EvidenceRecord,
        assigned: Timestamp,
    ) -> Result<AdmissionReceipt, SdkError> {
        assign_recorded_at(&mut record, assigned.clone());
        record.validate()?;
        record = self.admission_policy.classify_and_redact(record)?;
        assign_recorded_at(&mut record, assigned.clone());
        record.validate()?;

        let mut candidate = flatten_records(&self.sealed_segments);
        candidate.extend(self.staged_records.iter().cloned());
        candidate.push(record.clone());
        EvidenceState::replay(candidate)?;

        append_staging(&self.staging_path, &record)?;
        let record_id = record.record_id().to_owned();
        self.staged_records.push(record);
        Ok(AdmissionReceipt {
            record_id,
            recorded_at: assigned,
            pending_count: self.staged_records.len(),
        })
    }

    /// Seal the current durable staging batch into one immutable segment.
    pub fn seal(&mut self) -> Result<SealReceipt, SdkError> {
        if self.staged_records.is_empty() {
            return Err(SdkError::NoPendingRecords);
        }
        let created_at = latest_recorded_at(&self.staged_records);
        let encoded = encode_segment(&self.staged_records, &EncodeOptions::new(created_at))?;
        let path = encoded.write_atomic(&self.segments_directory)?;
        let relative_path = file_name(&path)?;
        self.catalog
            .register_segment(&encoded.manifest, &relative_path, &self.staged_records)?;

        let mut sealed_records = flatten_records(&self.sealed_segments);
        sealed_records.extend(self.staged_records.iter().cloned());
        let sealed_state = EvidenceState::replay(sealed_records)?;
        self.catalog
            .replace_observation_heads(&observation_heads(&sealed_state)?)?;
        clear_staging(&self.staging_path)?;

        let records = std::mem::take(&mut self.staged_records);
        let manifest = encoded.manifest;
        self.sealed_segments.push(LoadedSegment {
            manifest: manifest.clone(),
            records,
        });
        self.sealed_segments
            .sort_by(|left, right| left.manifest.segment_id.cmp(&right.manifest.segment_id));
        Ok(SealReceipt {
            segment_id: manifest.segment_id,
            row_count: manifest.row_count,
            content_sha256: manifest.integrity.content_sha256,
        })
    }

    /// Replay sealed and durably staged evidence through the one semantic implementation.
    pub fn replay(&self) -> Result<EvidenceState, SdkError> {
        let mut records = flatten_records(&self.sealed_segments);
        records.extend(self.staged_records.iter().cloned());
        EvidenceState::replay(records).map_err(Into::into)
    }

    pub fn replay_json(&self) -> Result<String, SdkError> {
        canonical_json(&self.replay()?)
    }

    pub fn locate(&self, record_id: &str) -> Result<Option<RecordLocation>, SdkError> {
        self.catalog.record_location(record_id).map_err(Into::into)
    }

    /// Validate SQLite, every immutable segment, maintained metadata, and canonical replay.
    pub fn validate(&self) -> Result<ValidationReport, SdkError> {
        self.catalog.integrity_check()?;
        let catalog_snapshot = self.catalog.snapshot()?;
        let resolver = DirectoryDictionaryResolver {
            directory: self.dictionaries_directory.clone(),
        };
        let segment_entries = catalog_snapshot
            .segments
            .iter()
            .map(|entry| (entry.segment_id.as_str(), entry))
            .collect::<BTreeMap<_, _>>();
        let storage_paths = segment_files(&self.segments_directory)?
            .into_iter()
            .map(|path| file_name(&path))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let catalog_paths = catalog_snapshot
            .segments
            .iter()
            .map(|entry| entry.relative_path.clone())
            .collect::<BTreeSet<_>>();
        if storage_paths != catalog_paths {
            return Err(SdkError::CatalogDisagreement(
                "catalog paths and immutable segment paths differ".to_owned(),
            ));
        }

        let mut decoded_segments = Vec::new();
        for relative_path in &storage_paths {
            let path = self.segments_directory.join(relative_path);
            if !fs::metadata(&path)
                .map_err(SdkError::io)?
                .permissions()
                .readonly()
            {
                return Err(SdkError::CatalogDisagreement(format!(
                    "sealed segment is writable: {relative_path}"
                )));
            }
            let decoded = decode_segment_file(&path, &resolver)?;
            let entry = segment_entries
                .get(decoded.manifest.segment_id.as_str())
                .ok_or_else(|| {
                    SdkError::CatalogDisagreement(format!(
                        "segment {} is absent from catalog",
                        decoded.manifest.segment_id
                    ))
                })?;
            if !entry.agrees_with(relative_path, &decoded.manifest)? {
                return Err(SdkError::CatalogDisagreement(format!(
                    "catalog metadata disagrees for segment {}",
                    decoded.manifest.segment_id
                )));
            }
            validate_record_locations(&self.catalog, &decoded)?;
            decoded_segments.push(LoadedSegment {
                manifest: decoded.manifest,
                records: decoded.records,
            });
        }
        let sealed_records = flatten_records(&decoded_segments);
        if catalog_snapshot.records.len() != sealed_records.len() {
            return Err(SdkError::CatalogDisagreement(format!(
                "catalog has {} record rows for {} archived records",
                catalog_snapshot.records.len(),
                sealed_records.len()
            )));
        }
        let sealed_state = EvidenceState::replay(sealed_records.clone())?;
        let expected_heads = observation_head_rows(&observation_heads(&sealed_state)?)?;
        if catalog_snapshot.heads != expected_heads {
            return Err(SdkError::CatalogDisagreement(
                "maintained observation heads disagree with replay".to_owned(),
            ));
        }
        let mut complete_history = sealed_records;
        complete_history.extend(self.staged_records.iter().cloned());
        let complete_state = EvidenceState::replay(complete_history)?;
        Ok(ValidationReport {
            segment_count: decoded_segments.len(),
            archived_record_count: catalog_snapshot.records.len(),
            pending_record_count: self.staged_records.len(),
            observation_count: complete_state.observations.len(),
            correction_count: complete_state.corrections.len(),
            frontier_count: complete_state.frontiers.len(),
        })
    }

    pub fn catalog_snapshot(&self) -> Result<CatalogSnapshot, SdkError> {
        self.catalog.snapshot().map_err(Into::into)
    }
}

fn validate_record_locations(catalog: &Catalog, decoded: &DecodedSegment) -> Result<(), SdkError> {
    for (index, record) in decoded.records.iter().enumerate() {
        let location = catalog
            .record_location(record.record_id())?
            .ok_or_else(|| {
                SdkError::CatalogDisagreement(format!(
                    "record {} is absent from catalog",
                    record.record_id()
                ))
            })?;
        if !location.agrees_with(
            &decoded.manifest.segment_id,
            i64::try_from(index).unwrap_or(i64::MAX),
            record,
        ) {
            return Err(SdkError::CatalogDisagreement(format!(
                "record location disagrees for {}",
                record.record_id()
            )));
        }
    }
    Ok(())
}

fn observation_heads(state: &EvidenceState) -> Result<Vec<ObservationHead>, SdkError> {
    state
        .observations
        .iter()
        .map(|(observation_id, state)| {
            Ok(ObservationHead {
                observation_id: observation_id.clone(),
                disposition: serde_json::to_value(&state.disposition).map_err(SdkError::json)?,
                qualifications: state.qualifications.iter().cloned().collect(),
                correction_ids: state.correction_ids.iter().cloned().collect(),
            })
        })
        .collect()
}

fn observation_head_rows(heads: &[ObservationHead]) -> Result<Vec<ObservationHeadRow>, SdkError> {
    heads
        .iter()
        .map(|head| {
            Ok(ObservationHeadRow {
                observation_id: head.observation_id.clone(),
                disposition_json: canonical_json(&head.disposition)?,
                qualifications_json: canonical_json(&head.qualifications)?,
                correction_ids_json: canonical_json(&head.correction_ids)?,
            })
        })
        .collect()
}

fn assign_recorded_at(record: &mut EvidenceRecord, timestamp: Timestamp) {
    match record {
        EvidenceRecord::Observation(record) => record.recorded_at = timestamp,
        EvidenceRecord::Correction(record) => record.recorded_at = timestamp,
        EvidenceRecord::Frontier(record) => record.as_of_recorded_at = timestamp,
    }
}

fn flatten_records(segments: &[LoadedSegment]) -> Vec<EvidenceRecord> {
    segments
        .iter()
        .flat_map(|segment| segment.records.iter().cloned())
        .collect()
}

fn latest_recorded_at(records: &[EvidenceRecord]) -> Timestamp {
    records
        .iter()
        .map(EvidenceRecord::recorded_at)
        .max_by_key(|timestamp| timestamp_instant(timestamp))
        .expect("seal requires a non-empty staging batch")
        .clone()
}

fn timestamp_instant(timestamp: &Timestamp) -> i128 {
    OffsetDateTime::parse(timestamp.as_str(), &Rfc3339)
        .expect("Timestamp guarantees RFC 3339")
        .unix_timestamp_nanos()
}

fn append_staging(path: &Path, record: &EvidenceRecord) -> Result<(), SdkError> {
    let row = canonical_record_json(record)?;
    let existing_length = fs::metadata(path).map_or(0, |metadata| metadata.len());
    let next_length = existing_length
        .checked_add(u64::try_from(row.len() + 1).unwrap_or(u64::MAX))
        .ok_or_else(|| SdkError::StagingCorrupt("staging length overflow".to_owned()))?;
    if next_length > MAX_STAGING_BYTES {
        return Err(SdkError::StagingCorrupt(format!(
            "staging exceeds {MAX_STAGING_BYTES} bytes"
        )));
    }
    let created = !path.exists();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(SdkError::io)?;
    file.write_all(&row).map_err(SdkError::io)?;
    file.write_all(b"\n").map_err(SdkError::io)?;
    file.sync_all().map_err(SdkError::io)?;
    if created {
        sync_directory(path.parent().expect("staging path has repository parent"))?;
    }
    Ok(())
}

fn load_staging(path: &Path) -> Result<Vec<EvidenceRecord>, SdkError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let metadata = fs::metadata(path).map_err(SdkError::io)?;
    if metadata.len() > MAX_STAGING_BYTES {
        return Err(SdkError::StagingCorrupt(format!(
            "staging exceeds {MAX_STAGING_BYTES} bytes"
        )));
    }
    let bytes = fs::read(path).map_err(SdkError::io)?;
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.ends_with(b"\n") {
        return Err(SdkError::StagingCorrupt(
            "staging is truncated before LF".to_owned(),
        ));
    }
    let mut records = Vec::new();
    for (index, row) in bytes[..bytes.len() - 1]
        .split(|byte| *byte == b'\n')
        .enumerate()
    {
        if row.is_empty() {
            return Err(SdkError::StagingCorrupt(format!(
                "staging row {index} is blank"
            )));
        }
        let record: EvidenceRecord = serde_json::from_slice(row)
            .map_err(|error| SdkError::StagingCorrupt(format!("staging row {index}: {error}")))?;
        record.validate()?;
        if canonical_record_json(&record)? != row {
            return Err(SdkError::StagingCorrupt(format!(
                "staging row {index} is not canonical"
            )));
        }
        records.push(record);
    }
    Ok(records)
}

fn clear_staging(path: &Path) -> Result<(), SdkError> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(SdkError::io)?;
    file.sync_all().map_err(SdkError::io)?;
    sync_directory(path.parent().expect("staging path has repository parent"))
}

fn remove_partial_files(directory: &Path) -> Result<usize, SdkError> {
    let mut removed = 0;
    for entry in fs::read_dir(directory).map_err(SdkError::io)? {
        let path = entry.map_err(SdkError::io)?.path();
        let name = file_name(&path)?;
        if name.starts_with('.') && name.ends_with(".partial") {
            fs::remove_file(path).map_err(SdkError::io)?;
            removed += 1;
        }
    }
    if removed > 0 {
        sync_directory(directory)?;
    }
    Ok(removed)
}

fn segment_files(directory: &Path) -> Result<Vec<PathBuf>, SdkError> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory).map_err(SdkError::io)? {
        let path = entry.map_err(SdkError::io)?.path();
        let name = file_name(&path)?;
        if name.starts_with('.') && name.ends_with(".partial") {
            return Err(SdkError::RecoveryRequired(format!(
                "partial file appeared during operation: {name}"
            )));
        }
        if path.extension().and_then(|value| value.to_str()) != Some("fseg") {
            return Err(SdkError::UnexpectedStorageEntry(name));
        }
        paths.push(path);
    }
    paths.sort();
    Ok(paths)
}

fn sync_directory(directory: &Path) -> Result<(), SdkError> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(SdkError::io)
}

fn enforce_readonly(path: &Path) -> Result<(), SdkError> {
    let mut permissions = fs::metadata(path).map_err(SdkError::io)?.permissions();
    if !permissions.readonly() {
        permissions.set_readonly(true);
        fs::set_permissions(path, permissions).map_err(SdkError::io)?;
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(SdkError::io)?;
    }
    Ok(())
}

fn file_name(path: &Path) -> Result<String, SdkError> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .ok_or_else(|| SdkError::UnexpectedStorageEntry(path.display().to_string()))
}

fn canonical_json<T: Serialize + ?Sized>(value: &T) -> Result<String, SdkError> {
    let value = serde_json::to_value(value).map_err(SdkError::json)?;
    serde_json::to_string(&value).map_err(SdkError::json)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdmissionReceipt {
    pub record_id: String,
    pub recorded_at: Timestamp,
    pub pending_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SealReceipt {
    pub segment_id: String,
    pub row_count: u64,
    pub content_sha256: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecoveryReport {
    pub removed_partial_files: usize,
    pub registered_orphan_segments: usize,
    pub cleared_redundant_staging: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub segment_count: usize,
    pub archived_record_count: usize,
    pub pending_record_count: usize,
    pub observation_count: usize,
    pub correction_count: usize,
    pub frontier_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SdkErrorCategory {
    Io,
    JsonInvalid,
    SchemaInvalid,
    SemanticInvalid,
    AdmissionRejected,
    AdmissionInvalid,
    ClockFailed,
    SegmentInvalid,
    MissingDictionary,
    CatalogInvalid,
    CatalogDisagreement,
    StagingCorrupt,
    NoPendingRecords,
    UnexpectedStorageEntry,
    RecoveryRequired,
}

#[derive(Debug, Error)]
pub enum SdkError {
    #[error("engine IO failed: {0}")]
    Io(String),
    #[error("invalid JSON: {0}")]
    JsonInvalid(String),
    #[error("schema validation failed: {0}")]
    SchemaInvalid(String),
    #[error("semantic validation failed: {0}")]
    SemanticInvalid(String),
    #[error(transparent)]
    AdmissionRejected(#[from] AdmissionPolicyError),
    #[error("invalid admission input: {0}")]
    AdmissionInvalid(String),
    #[error(transparent)]
    ClockFailed(#[from] ClockError),
    #[error(transparent)]
    Segment(#[from] SegmentError),
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error("catalog and storage disagree: {0}")]
    CatalogDisagreement(String),
    #[error("durable staging is corrupt: {0}")]
    StagingCorrupt(String),
    #[error("there are no pending records to seal")]
    NoPendingRecords,
    #[error("unexpected entry in immutable segment directory: {0}")]
    UnexpectedStorageEntry(String),
    #[error("reopen the engine to complete deterministic recovery: {0}")]
    RecoveryRequired(String),
}

impl SdkError {
    #[must_use]
    pub fn category(&self) -> SdkErrorCategory {
        match self {
            Self::Io(_) => SdkErrorCategory::Io,
            Self::JsonInvalid(_) => SdkErrorCategory::JsonInvalid,
            Self::SchemaInvalid(_) => SdkErrorCategory::SchemaInvalid,
            Self::SemanticInvalid(_) => SdkErrorCategory::SemanticInvalid,
            Self::AdmissionRejected(_) => SdkErrorCategory::AdmissionRejected,
            Self::AdmissionInvalid(_) => SdkErrorCategory::AdmissionInvalid,
            Self::ClockFailed(_) => SdkErrorCategory::ClockFailed,
            Self::Segment(error) if error.category() == SegmentErrorCategory::MissingDictionary => {
                SdkErrorCategory::MissingDictionary
            }
            Self::Segment(_) => SdkErrorCategory::SegmentInvalid,
            Self::Catalog(_) => SdkErrorCategory::CatalogInvalid,
            Self::CatalogDisagreement(_) => SdkErrorCategory::CatalogDisagreement,
            Self::StagingCorrupt(_) => SdkErrorCategory::StagingCorrupt,
            Self::NoPendingRecords => SdkErrorCategory::NoPendingRecords,
            Self::UnexpectedStorageEntry(_) => SdkErrorCategory::UnexpectedStorageEntry,
            Self::RecoveryRequired(_) => SdkErrorCategory::RecoveryRequired,
        }
    }

    fn io(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }

    fn json(error: serde_json::Error) -> Self {
        Self::JsonInvalid(error.to_string())
    }
}

impl From<SchemaError> for SdkError {
    fn from(error: SchemaError) -> Self {
        Self::SchemaInvalid(error.to_string())
    }
}

impl From<CoreError> for SdkError {
    fn from(error: CoreError) -> Self {
        Self::SemanticInvalid(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_schema::{Correction, CorrectionOperation, Observation};
    use rusqlite::Connection;
    use tempfile::tempdir;

    #[derive(Clone)]
    struct FixedClock(Timestamp);

    impl AdmissionClock for FixedClock {
        fn now(&self) -> Result<Timestamp, ClockError> {
            Ok(self.0.clone())
        }
    }

    fn fixed_clock() -> FixedClock {
        FixedClock(Timestamp::parse("2026-07-20T16:00:00Z").expect("fixed timestamp"))
    }

    fn observation_json() -> &'static str {
        include_str!("../../../fixtures/golden/contracts/valid/observation.json")
    }

    fn correction_json() -> &'static str {
        include_str!("../../../fixtures/golden/contracts/valid/correction.json")
    }

    #[test]
    fn append_seal_reopen_and_replay_are_equivalent() {
        let directory = tempdir().expect("temporary directory");
        let expected = {
            let mut engine = Engine::open_with_components(
                directory.path(),
                RequireClassification,
                fixed_clock(),
            )
            .expect("open engine");
            let receipt = engine
                .admit_json(observation_json())
                .expect("admit observation");
            assert_eq!(receipt.recorded_at.as_str(), "2026-07-20T16:00:00Z");
            engine
                .admit_json(correction_json())
                .expect("admit correction");
            let before_seal = engine.replay().expect("replay pending records");
            let sealed = engine.seal().expect("seal records");
            assert_eq!(sealed.row_count, 2);
            assert_eq!(engine.pending_count(), 0);
            assert_eq!(engine.replay().expect("replay sealed records"), before_seal);
            engine.validate().expect("validate engine");
            before_seal
        };

        let reopened = Engine::open(directory.path()).expect("reopen engine");
        assert_eq!(reopened.replay().expect("replay reopened"), expected);
        assert_eq!(reopened.replay().expect("repeat replay"), expected);
        assert_eq!(
            reopened
                .validate()
                .expect("validate reopened")
                .segment_count,
            1
        );
    }

    #[test]
    fn orphan_segment_is_registered_and_matching_staging_is_cleared() {
        let directory = tempdir().expect("temporary directory");
        {
            let mut engine = Engine::open_with_components(
                directory.path(),
                RequireClassification,
                fixed_clock(),
            )
            .expect("open engine");
            engine
                .admit_json(observation_json())
                .expect("admit observation");
            let created_at = latest_recorded_at(&engine.staged_records);
            let encoded = encode_segment(&engine.staged_records, &EncodeOptions::new(created_at))
                .expect("encode interrupted segment");
            encoded
                .write_atomic(&engine.segments_directory)
                .expect("write interrupted segment");
        }

        let recovered = Engine::open(directory.path()).expect("recover engine");
        assert_eq!(recovered.pending_count(), 0);
        assert_eq!(recovered.recovery_report().registered_orphan_segments, 1);
        assert!(recovered.recovery_report().cleared_redundant_staging);
        assert_eq!(
            recovered
                .replay()
                .expect("replay recovered")
                .observations
                .len(),
            1
        );
    }

    #[test]
    fn partial_file_is_rolled_back_without_losing_staging() {
        let directory = tempdir().expect("temporary directory");
        {
            let mut engine = Engine::open_with_components(
                directory.path(),
                RequireClassification,
                fixed_clock(),
            )
            .expect("open engine");
            engine
                .admit_json(observation_json())
                .expect("admit observation");
            fs::write(
                engine.segments_directory.join(".interrupted.partial"),
                b"incomplete",
            )
            .expect("write interrupted partial");
        }
        let recovered = Engine::open(directory.path()).expect("recover partial");
        assert_eq!(recovered.recovery_report().removed_partial_files, 1);
        assert_eq!(recovered.pending_count(), 1);
        assert_eq!(
            recovered
                .replay()
                .expect("replay staging")
                .observations
                .len(),
            1
        );
    }

    #[test]
    fn cataloged_segment_with_uncleared_staging_recovers_idempotently() {
        let directory = tempdir().expect("temporary directory");
        {
            let mut engine = Engine::open_with_components(
                directory.path(),
                RequireClassification,
                fixed_clock(),
            )
            .expect("open engine");
            engine
                .admit_json(observation_json())
                .expect("admit observation");
            let encoded = encode_segment(
                &engine.staged_records,
                &EncodeOptions::new(latest_recorded_at(&engine.staged_records)),
            )
            .expect("encode segment");
            let path = encoded
                .write_atomic(&engine.segments_directory)
                .expect("write segment");
            engine
                .catalog
                .register_segment(
                    &encoded.manifest,
                    &file_name(&path).expect("segment filename"),
                    &engine.staged_records,
                )
                .expect("register segment before interrupted cleanup");
        }
        let recovered = Engine::open(directory.path()).expect("recover cataloged segment");
        assert_eq!(recovered.recovery_report().registered_orphan_segments, 0);
        assert!(recovered.recovery_report().cleared_redundant_staging);
        assert_eq!(recovered.pending_count(), 0);
    }

    #[test]
    fn corrupt_segment_and_missing_catalog_file_fail_cleanly() {
        let directory = tempdir().expect("temporary directory");
        let segment_path = {
            let mut engine = Engine::open_with_components(
                directory.path(),
                RequireClassification,
                fixed_clock(),
            )
            .expect("open engine");
            engine
                .admit_json(observation_json())
                .expect("admit observation");
            let receipt = engine.seal().expect("seal observation");
            engine
                .segments_directory
                .join(format!("{}.fseg", receipt.segment_id))
        };
        let original = fs::read(&segment_path).expect("read segment");
        let mut corrupt = original.clone();
        let header_length =
            u32::from_be_bytes([corrupt[12], corrupt[13], corrupt[14], corrupt[15]]) as usize;
        corrupt[24 + header_length] ^= 1;
        fs::remove_file(&segment_path).expect("remove read-only segment for corruption test");
        fs::write(&segment_path, &corrupt).expect("write corruption");
        assert!(matches!(
            Engine::open(directory.path()),
            Err(SdkError::Segment(SegmentError::PayloadChecksumMismatch))
        ));

        fs::write(&segment_path, original).expect("restore segment");
        fs::remove_file(&segment_path).expect("remove segment");
        assert!(matches!(
            Engine::open(directory.path()),
            Err(SdkError::CatalogDisagreement(_))
        ));
    }

    #[test]
    fn validation_rejects_record_metadata_that_disagrees_with_evidence() {
        let directory = tempdir().expect("temporary directory");
        let mut engine =
            Engine::open_with_components(directory.path(), RequireClassification, fixed_clock())
                .expect("open engine");
        engine
            .admit_json(observation_json())
            .expect("admit observation");
        engine.seal().expect("seal observation");
        let catalog =
            Connection::open(directory.path().join(CATALOG_FILE)).expect("open catalog directly");
        catalog
            .execute(
                "UPDATE records SET classification = 'fabric-review-probe' \
                 WHERE record_id = 'obs-0001'",
                [],
            )
            .expect("alter record metadata");
        drop(catalog);

        assert!(matches!(
            engine.validate(),
            Err(SdkError::CatalogDisagreement(_))
        ));
    }

    #[test]
    fn validation_rejects_duplicated_segment_metadata_that_disagrees() {
        let directory = tempdir().expect("temporary directory");
        let mut engine =
            Engine::open_with_components(directory.path(), RequireClassification, fixed_clock())
                .expect("open engine");
        engine
            .admit_json(observation_json())
            .expect("admit observation");
        engine.seal().expect("seal observation");
        let catalog =
            Connection::open(directory.path().join(CATALOG_FILE)).expect("open catalog directly");
        catalog
            .execute("UPDATE segments SET row_count = 999", [])
            .expect("alter duplicated segment metadata");
        drop(catalog);

        assert!(matches!(
            engine.validate(),
            Err(SdkError::CatalogDisagreement(_))
        ));
    }

    struct RedactPayload;

    impl AdmissionPolicy for RedactPayload {
        fn classify_and_redact(
            &self,
            mut record: EvidenceRecord,
        ) -> Result<EvidenceRecord, AdmissionPolicyError> {
            if let EvidenceRecord::Observation(observation) = &mut record {
                observation.payload = serde_json::json!({"redacted": true});
            }
            Ok(record)
        }
    }

    #[test]
    fn policy_runs_before_any_durable_staging_write() {
        let directory = tempdir().expect("temporary directory");
        let mut engine =
            Engine::open_with_components(directory.path(), RedactPayload, fixed_clock())
                .expect("open engine");
        engine
            .admit_json(observation_json())
            .expect("admit redacted observation");
        let staging =
            fs::read_to_string(directory.path().join(STAGING_FILE)).expect("read durable staging");
        assert!(!staging.contains("order.accepted"));
        assert!(staging.contains("redacted"));
    }

    struct MustNotRun;

    impl AdmissionPolicy for MustNotRun {
        fn classify_and_redact(
            &self,
            _record: EvidenceRecord,
        ) -> Result<EvidenceRecord, AdmissionPolicyError> {
            panic!("schema-invalid input must not reach the admission policy")
        }
    }

    #[test]
    fn schema_invalid_json_is_rejected_before_policy_or_staging() {
        let mut missing_nullable: Value =
            serde_json::from_str(observation_json()).expect("observation fixture");
        missing_nullable
            .as_object_mut()
            .expect("observation is an object")
            .remove("observed_at");

        let mut unknown_property: Value =
            serde_json::from_str(observation_json()).expect("observation fixture");
        unknown_property["unexpected"] = serde_json::json!(true);

        let mut malformed_classification: Value =
            serde_json::from_str(observation_json()).expect("observation fixture");
        malformed_classification["classification"] = serde_json::json!("Internal");

        for (case, value) in [
            ("missing required nullable field", missing_nullable),
            ("unknown top-level property", unknown_property),
            ("malformed classification", malformed_classification),
        ] {
            let directory = tempdir().expect("temporary directory");
            let mut engine =
                Engine::open_with_components(directory.path(), MustNotRun, fixed_clock())
                    .expect("open engine");
            let json = serde_json::to_string(&value).expect("serialize invalid input");
            let error = engine.admit_json(&json).expect_err(case);

            assert_eq!(error.category(), SdkErrorCategory::SchemaInvalid, "{case}");
            assert_eq!(engine.pending_count(), 0, "{case}");
            assert!(
                fs::read(directory.path().join(STAGING_FILE))
                    .unwrap_or_default()
                    .is_empty(),
                "{case}"
            );
        }
    }

    #[test]
    fn engine_owned_recorded_at_may_be_omitted_from_json_input() {
        let directory = tempdir().expect("temporary directory");
        let mut engine =
            Engine::open_with_components(directory.path(), RequireClassification, fixed_clock())
                .expect("open engine");
        let mut observation: Value =
            serde_json::from_str(observation_json()).expect("observation fixture");
        observation
            .as_object_mut()
            .expect("observation is an object")
            .remove("recorded_at");

        let receipt = engine
            .admit_json(&serde_json::to_string(&observation).expect("serialize observation"))
            .expect("engine assigns recorded_at");
        assert_eq!(receipt.recorded_at.as_str(), "2026-07-20T16:00:00Z");
        assert_eq!(engine.pending_count(), 1);
    }

    #[test]
    fn mixed_epistemic_class_is_not_coerced() {
        let directory = tempdir().expect("temporary directory");
        let mut engine =
            Engine::open_with_components(directory.path(), RequireClassification, fixed_clock())
                .expect("open engine");
        let mut observation: Observation =
            serde_json::from_str(observation_json()).expect("observation fixture");
        observation.epistemic_class = fabric_schema::EpistemicClass::Assumption;
        let error = engine
            .admit(EvidenceRecord::from(observation))
            .expect_err("mixed class must fail");
        assert_eq!(error.category(), SdkErrorCategory::SchemaInvalid);
        assert_eq!(engine.pending_count(), 0);
    }

    #[test]
    fn late_arriving_forward_correction_is_rejected_before_staging() {
        let directory = tempdir().expect("temporary directory");
        let mut engine =
            Engine::open_with_components(directory.path(), RequireClassification, fixed_clock())
                .expect("open engine");
        let mut target: Observation =
            serde_json::from_str(observation_json()).expect("observation fixture");
        target.producer_sequence = Some(42);
        target
            .logical_time
            .as_mut()
            .expect("target logical time")
            .counter = 95;
        let mut replacement = target.clone();
        replacement.observation_id = "obs-0002".to_owned();
        replacement.producer_sequence = Some(43);
        replacement
            .logical_time
            .as_mut()
            .expect("replacement logical time")
            .counter = 96;
        let mut correction: Correction =
            serde_json::from_str(correction_json()).expect("correction fixture");
        correction.operation = CorrectionOperation::Replacement;
        correction.targets = BTreeSet::from([target.observation_id.clone()]);
        correction.replacement_ids = Some(BTreeSet::from([replacement.observation_id.clone()]));
        correction.qualification = None;
        correction.producer_sequence = Some(41);
        correction
            .logical_time
            .as_mut()
            .expect("correction logical time")
            .counter = 94;

        engine
            .admit(EvidenceRecord::from(target))
            .expect("admit target");
        engine
            .admit(EvidenceRecord::from(replacement))
            .expect("admit replacement");
        let staging_before =
            fs::read_to_string(directory.path().join(STAGING_FILE)).expect("read staging");
        let error = engine
            .admit(EvidenceRecord::Correction(correction))
            .expect_err("semantically forward correction must fail");

        assert_eq!(error.category(), SdkErrorCategory::SemanticInvalid);
        assert_eq!(engine.pending_count(), 2);
        assert_eq!(
            fs::read_to_string(directory.path().join(STAGING_FILE)).expect("reread staging"),
            staging_before
        );
    }

    #[test]
    fn catalog_can_be_rebuilt_from_immutable_segments() {
        let first_root = tempdir().expect("first root");
        let second_root = tempdir().expect("second root");
        let first_snapshot = {
            let mut engine = Engine::open_with_components(
                first_root.path(),
                RequireClassification,
                fixed_clock(),
            )
            .expect("open first engine");
            engine
                .admit_json(observation_json())
                .expect("admit observation");
            engine.seal().expect("seal observation");
            engine.catalog_snapshot().expect("first snapshot")
        };
        let destination = second_root.path().join(SEGMENTS_DIRECTORY);
        fs::create_dir_all(&destination).expect("create destination");
        for path in
            segment_files(&first_root.path().join(SEGMENTS_DIRECTORY)).expect("source segments")
        {
            fs::copy(&path, destination.join(path.file_name().expect("filename")))
                .expect("copy immutable segment");
        }
        let rebuilt = Engine::open(second_root.path()).expect("rebuild catalog");
        assert_eq!(
            rebuilt.catalog_snapshot().expect("rebuilt snapshot"),
            first_snapshot
        );
    }

    #[test]
    fn canonical_correction_remains_qualification_after_admission() {
        let directory = tempdir().expect("temporary directory");
        let mut engine =
            Engine::open_with_components(directory.path(), RequireClassification, fixed_clock())
                .expect("open engine");
        let observation: Observation =
            serde_json::from_str(observation_json()).expect("observation fixture");
        let correction: Correction =
            serde_json::from_str(correction_json()).expect("correction fixture");
        engine
            .admit(EvidenceRecord::from(observation))
            .expect("admit observation");
        engine
            .admit(EvidenceRecord::Correction(correction))
            .expect("admit correction");
        let state = engine.replay().expect("replay");
        assert_eq!(state.corrections.len(), 1);
        assert_eq!(state.observations["obs-0001"].qualifications.len(), 1);
    }
}
