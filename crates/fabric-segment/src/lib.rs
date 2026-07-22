//! Immutable, bounded, integrity-checked segment containers.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use fabric_schema::{EvidenceRecord, SchemaError};
use fabric_time::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub const MANIFEST_VERSION: &str = "fabric.segment.manifest/v1";
pub const SCHEMA_SET_VERSION: &str = "fabric.evidence/v1";
pub const FORMAT_VERSION: u16 = 1;
pub const FILE_EXTENSION: &str = "fseg";
pub const PRELUDE_LENGTH: usize = 24;
pub const TRAILER_LENGTH: usize = 72;
pub const MAX_MANIFEST_BYTES: usize = 65_536;
pub const MAX_COMPRESSED_BYTES: u64 = 268_435_456;
pub const MAX_UNCOMPRESSED_BYTES: u64 = 268_435_456;
pub const MAX_ROWS: u64 = 10_000_000;

const PRELUDE_MAGIC: &[u8; 8] = b"FABSEG01";
const TRAILER_MAGIC: &[u8; 8] = b"FABEND01";
const FLAG_DICTIONARY: u16 = 1;
const FLAG_CONTENT_DIGEST: u16 = 1 << 1;
const SUPPORTED_FLAGS: u16 = FLAG_DICTIONARY | FLAG_CONTENT_DIGEST;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TimeRange {
    pub min: Timestamp,
    pub max: Timestamp,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct ProducerSequenceRange {
    pub producer_id: String,
    pub stream_id: String,
    pub min: u64,
    pub max: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct LogicalRange {
    pub clock_id: String,
    pub min: u64,
    pub max: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OrderingBounds {
    pub producer_sequences: Vec<ProducerSequenceRange>,
    pub logical_clocks: Vec<LogicalRange>,
    pub observed_at: Option<TimeRange>,
    pub observed_by_at: Option<TimeRange>,
    pub recorded_at: Option<TimeRange>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct DictionaryLocator {
    pub family: String,
    pub version: u64,
    pub digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FrameManifest {
    pub frame_index: u64,
    pub payload_offset: u64,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    pub row_start: u64,
    pub row_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompressionManifest {
    pub codec: String,
    pub level: i32,
    pub dictionary: Option<DictionaryLocator>,
    pub frames: Vec<FrameManifest>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SegmentIntegrity {
    pub payload_sha256: String,
    pub content_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SegmentManifest {
    pub api_version: String,
    pub format_version: u16,
    pub segment_id: String,
    pub created_at: Timestamp,
    pub schema_set_id: String,
    pub row_count: u64,
    pub frame_count: u64,
    pub ordering_bounds: OrderingBounds,
    pub classifications: BTreeSet<String>,
    pub compression: CompressionManifest,
    pub integrity: SegmentIntegrity,
}

impl SegmentManifest {
    fn validate_shape(&self, flags: u16, compressed_length: u64) -> Result<(), SegmentError> {
        if self.api_version != MANIFEST_VERSION || self.format_version != FORMAT_VERSION {
            return Err(SegmentError::ManifestInvalid(
                "manifest envelope or format version is unsupported".to_owned(),
            ));
        }
        if self.schema_set_id != SCHEMA_SET_VERSION {
            return Err(SegmentError::ManifestInvalid(
                "unsupported schema_set_id".to_owned(),
            ));
        }
        if self.row_count == 0 || self.row_count > MAX_ROWS {
            return Err(SegmentError::RowLimitExceeded(self.row_count));
        }
        if self.frame_count != 1 || self.compression.frames.len() != 1 {
            return Err(SegmentError::ManifestInvalid(
                "format v1 requires exactly one frame".to_owned(),
            ));
        }
        if self.classifications.is_empty() {
            return Err(SegmentError::ManifestInvalid(
                "classifications must not be empty".to_owned(),
            ));
        }
        if self.compression.codec != "zstd" {
            return Err(SegmentError::ManifestInvalid(
                "compression codec must be zstd".to_owned(),
            ));
        }
        if !(-7..=22).contains(&self.compression.level) {
            return Err(SegmentError::ManifestInvalid(
                "Zstandard compression level is outside -7..=22".to_owned(),
            ));
        }
        let frame = &self.compression.frames[0];
        if frame.frame_index != 0
            || frame.payload_offset != 0
            || frame.row_start != 0
            || frame.row_count != self.row_count
            || frame.compressed_bytes == 0
            || frame.compressed_bytes != compressed_length
        {
            return Err(SegmentError::ManifestInvalid(
                "frame offsets, sizes, or row counts disagree".to_owned(),
            ));
        }
        if frame.uncompressed_bytes == 0 || frame.uncompressed_bytes > MAX_UNCOMPRESSED_BYTES {
            return Err(SegmentError::ContentTooLarge(frame.uncompressed_bytes));
        }
        let dictionary_flag = flags & FLAG_DICTIONARY != 0;
        if dictionary_flag != self.compression.dictionary.is_some() {
            return Err(SegmentError::ManifestInvalid(
                "dictionary flag and locator disagree".to_owned(),
            ));
        }
        if let Some(locator) = &self.compression.dictionary
            && (!valid_label(&locator.family)
                || locator.version == 0
                || !is_lower_hex_digest(&locator.digest))
        {
            return Err(SegmentError::ManifestInvalid(
                "dictionary locator is malformed".to_owned(),
            ));
        }
        if !self.classifications.iter().all(|value| valid_label(value)) {
            return Err(SegmentError::ManifestInvalid(
                "classification label is malformed".to_owned(),
            ));
        }
        if flags & FLAG_CONTENT_DIGEST == 0 {
            return Err(SegmentError::ManifestInvalid(
                "format v1 requires the uncompressed content digest flag".to_owned(),
            ));
        }
        let expected_segment_id = format!("seg-{}", self.integrity.content_sha256);
        if self.segment_id != expected_segment_id
            || !is_lower_hex_digest(&self.integrity.payload_sha256)
            || !is_lower_hex_digest(&self.integrity.content_sha256)
        {
            return Err(SegmentError::ManifestInvalid(
                "segment identity or integrity digest is malformed".to_owned(),
            ));
        }
        validate_ranges(&self.ordering_bounds)?;
        Ok(())
    }

    /// Validate every manifest field derived from the canonical record batch.
    pub fn validate_records(&self, records: &[EvidenceRecord]) -> Result<(), SegmentError> {
        let inferred_flags = FLAG_CONTENT_DIGEST
            | if self.compression.dictionary.is_some() {
                FLAG_DICTIONARY
            } else {
                0
            };
        let compressed_length = self
            .compression
            .frames
            .first()
            .map_or(0, |frame| frame.compressed_bytes);
        self.validate_shape(inferred_flags, compressed_length)?;

        let actual_rows = u64::try_from(records.len()).unwrap_or(u64::MAX);
        if self.row_count != actual_rows {
            return Err(SegmentError::ManifestInvalid(format!(
                "manifest declares {} rows but decoded {actual_rows}",
                self.row_count
            )));
        }
        let expected_segment_id = segment_id_for_records(records)?;
        let expected_content_digest = expected_segment_id
            .strip_prefix("seg-")
            .expect("segment identity helper returns a prefixed digest");
        if self.segment_id != expected_segment_id
            || self.integrity.content_sha256 != expected_content_digest
        {
            return Err(SegmentError::ManifestInvalid(
                "segment identity disagrees with canonical records".to_owned(),
            ));
        }
        if self.classifications != classifications(records) {
            return Err(SegmentError::ManifestInvalid(
                "classifications disagree with canonical records".to_owned(),
            ));
        }
        if self.ordering_bounds != ordering_bounds(records) {
            return Err(SegmentError::ManifestInvalid(
                "ordering bounds disagree with canonical records".to_owned(),
            ));
        }
        Ok(())
    }
}

fn validate_ranges(bounds: &OrderingBounds) -> Result<(), SegmentError> {
    for range in &bounds.producer_sequences {
        if range.producer_id.is_empty() || range.stream_id.is_empty() || range.min > range.max {
            return Err(SegmentError::ManifestInvalid(
                "producer sequence range is inverted".to_owned(),
            ));
        }
    }
    for range in &bounds.logical_clocks {
        if range.clock_id.is_empty() || range.min > range.max {
            return Err(SegmentError::ManifestInvalid(
                "logical clock range is inverted".to_owned(),
            ));
        }
    }
    for range in [
        &bounds.observed_at,
        &bounds.observed_by_at,
        &bounds.recorded_at,
    ]
    .into_iter()
    .flatten()
    {
        if timestamp_instant(&range.min) > timestamp_instant(&range.max) {
            return Err(SegmentError::ManifestInvalid(
                "time range is inverted".to_owned(),
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct DictionaryMaterial {
    locator: DictionaryLocator,
    bytes: Vec<u8>,
}

impl DictionaryMaterial {
    pub fn new(locator: DictionaryLocator, bytes: Vec<u8>) -> Result<Self, SegmentError> {
        if sha256_hex(&bytes) != locator.digest {
            return Err(SegmentError::DictionaryDigestMismatch(locator));
        }
        Ok(Self { locator, bytes })
    }

    #[must_use]
    pub fn locator(&self) -> &DictionaryLocator {
        &self.locator
    }
}

#[derive(Clone, Debug)]
pub struct EncodeOptions {
    pub created_at: Timestamp,
    pub compression_level: i32,
    pub dictionary: Option<DictionaryMaterial>,
}

impl EncodeOptions {
    #[must_use]
    pub const fn new(created_at: Timestamp) -> Self {
        Self {
            created_at,
            compression_level: 3,
            dictionary: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedSegment {
    pub manifest: SegmentManifest,
    pub bytes: Vec<u8>,
}

impl EncodedSegment {
    /// Write once through a same-directory temporary file and make the result read-only.
    pub fn write_atomic(&self, directory: &Path) -> Result<PathBuf, SegmentError> {
        fs::create_dir_all(directory).map_err(SegmentError::io)?;
        let final_path = segment_path(directory, &self.manifest.segment_id);
        if final_path.exists() {
            let existing = fs::read(&final_path).map_err(SegmentError::io)?;
            if existing == self.bytes {
                return Ok(final_path);
            }
            return Err(SegmentError::ImmutableConflict(final_path));
        }

        let temporary_path = directory.join(format!(
            ".{}.{}.partial",
            self.manifest.segment_id,
            std::process::id()
        ));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)
            .map_err(SegmentError::io)?;
        file.write_all(&self.bytes).map_err(SegmentError::io)?;
        file.sync_all().map_err(SegmentError::io)?;
        drop(file);
        fs::rename(&temporary_path, &final_path).map_err(SegmentError::io)?;
        let mut permissions = fs::metadata(&final_path)
            .map_err(SegmentError::io)?
            .permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&final_path, permissions).map_err(SegmentError::io)?;
        File::open(directory)
            .and_then(|directory_file| directory_file.sync_all())
            .map_err(SegmentError::io)?;
        Ok(final_path)
    }
}

#[must_use]
pub fn segment_path(directory: &Path, segment_id: &str) -> PathBuf {
    directory.join(format!("{segment_id}.{FILE_EXTENSION}"))
}

pub fn encode_segment(
    records: &[EvidenceRecord],
    options: &EncodeOptions,
) -> Result<EncodedSegment, SegmentError> {
    let row_count =
        u64::try_from(records.len()).map_err(|_| SegmentError::RowLimitExceeded(u64::MAX))?;
    if row_count == 0 || row_count > MAX_ROWS {
        return Err(SegmentError::RowLimitExceeded(row_count));
    }

    let mut content = Vec::new();
    for record in records {
        record.validate()?;
        let row = canonical_json(record)?;
        let next_length = content
            .len()
            .checked_add(row.len() + 1)
            .ok_or(SegmentError::ContentTooLarge(u64::MAX))?;
        if u64::try_from(next_length).unwrap_or(u64::MAX) > MAX_UNCOMPRESSED_BYTES {
            return Err(SegmentError::ContentTooLarge(
                u64::try_from(next_length).unwrap_or(u64::MAX),
            ));
        }
        content.extend_from_slice(&row);
        content.push(b'\n');
    }

    let compressed = if let Some(dictionary) = &options.dictionary {
        let mut encoder = zstd::stream::Encoder::with_dictionary(
            Vec::new(),
            options.compression_level,
            &dictionary.bytes,
        )
        .map_err(SegmentError::compression)?;
        encoder
            .write_all(&content)
            .map_err(SegmentError::compression)?;
        encoder.finish().map_err(SegmentError::compression)?
    } else {
        zstd::stream::encode_all(Cursor::new(&content), options.compression_level)
            .map_err(SegmentError::compression)?
    };
    let compressed_length = u64::try_from(compressed.len()).unwrap_or(u64::MAX);
    if compressed_length > MAX_COMPRESSED_BYTES {
        return Err(SegmentError::PayloadTooLarge(compressed_length));
    }

    let content_digest = sha256_bytes(&content);
    let payload_digest = sha256_bytes(&compressed);
    let content_digest_hex = lower_hex(&content_digest);
    let payload_digest_hex = lower_hex(&payload_digest);
    let dictionary = options
        .dictionary
        .as_ref()
        .map(|material| material.locator.clone());
    let manifest = SegmentManifest {
        api_version: MANIFEST_VERSION.to_owned(),
        format_version: FORMAT_VERSION,
        segment_id: format!("seg-{content_digest_hex}"),
        created_at: options.created_at.clone(),
        schema_set_id: SCHEMA_SET_VERSION.to_owned(),
        row_count,
        frame_count: 1,
        ordering_bounds: ordering_bounds(records),
        classifications: classifications(records),
        compression: CompressionManifest {
            codec: "zstd".to_owned(),
            level: options.compression_level,
            dictionary,
            frames: vec![FrameManifest {
                frame_index: 0,
                payload_offset: 0,
                compressed_bytes: compressed_length,
                uncompressed_bytes: u64::try_from(content.len()).unwrap_or(u64::MAX),
                row_start: 0,
                row_count,
            }],
        },
        integrity: SegmentIntegrity {
            payload_sha256: payload_digest_hex,
            content_sha256: content_digest_hex,
        },
    };
    let flags = FLAG_CONTENT_DIGEST
        | if options.dictionary.is_some() {
            FLAG_DICTIONARY
        } else {
            0
        };
    manifest.validate_shape(flags, compressed_length)?;
    let manifest_bytes = canonical_json(&manifest)?;
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(SegmentError::HeaderTooLarge(
            u64::try_from(manifest_bytes.len()).unwrap_or(u64::MAX),
        ));
    }
    let manifest_digest = sha256_bytes(&manifest_bytes);
    let mut bytes = Vec::with_capacity(
        PRELUDE_LENGTH + manifest_bytes.len() + compressed.len() + TRAILER_LENGTH,
    );
    bytes.extend_from_slice(PRELUDE_MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_be_bytes());
    bytes.extend_from_slice(&flags.to_be_bytes());
    bytes.extend_from_slice(
        &u32::try_from(manifest_bytes.len())
            .expect("bounded manifest length fits u32")
            .to_be_bytes(),
    );
    bytes.extend_from_slice(&compressed_length.to_be_bytes());
    bytes.extend_from_slice(&manifest_bytes);
    bytes.extend_from_slice(&compressed);
    bytes.extend_from_slice(TRAILER_MAGIC);
    bytes.extend_from_slice(&manifest_digest);
    bytes.extend_from_slice(&payload_digest);
    Ok(EncodedSegment { manifest, bytes })
}

/// Return the exact canonical JSON bytes used for one segment row.
pub fn canonical_record_json(record: &EvidenceRecord) -> Result<Vec<u8>, SegmentError> {
    record.validate()?;
    canonical_json(record)
}

/// Compute the content-addressed identity for a non-empty record batch.
pub fn segment_id_for_records(records: &[EvidenceRecord]) -> Result<String, SegmentError> {
    let row_count = u64::try_from(records.len()).unwrap_or(u64::MAX);
    if row_count == 0 || row_count > MAX_ROWS {
        return Err(SegmentError::RowLimitExceeded(row_count));
    }
    let mut content_length = 0_u64;
    let mut hasher = Sha256::new();
    for record in records {
        let row = canonical_record_json(record)?;
        let row_length = u64::try_from(row.len())
            .unwrap_or(u64::MAX)
            .checked_add(1)
            .ok_or(SegmentError::ContentTooLarge(u64::MAX))?;
        content_length = content_length
            .checked_add(row_length)
            .ok_or(SegmentError::ContentTooLarge(u64::MAX))?;
        if content_length > MAX_UNCOMPRESSED_BYTES {
            return Err(SegmentError::ContentTooLarge(content_length));
        }
        hasher.update(&row);
        hasher.update(b"\n");
    }
    Ok(format!("seg-{}", lower_hex(&hasher.finalize())))
}

pub trait DictionaryResolver {
    fn resolve(&self, locator: &DictionaryLocator) -> Result<Vec<u8>, SegmentError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoDictionaries;

impl DictionaryResolver for NoDictionaries {
    fn resolve(&self, locator: &DictionaryLocator) -> Result<Vec<u8>, SegmentError> {
        Err(SegmentError::MissingDictionary(locator.clone()))
    }
}

#[derive(Clone, Debug, Default)]
pub struct MemoryDictionaryResolver {
    dictionaries: BTreeMap<DictionaryLocator, Vec<u8>>,
}

impl MemoryDictionaryResolver {
    pub fn insert(
        &mut self,
        locator: DictionaryLocator,
        bytes: Vec<u8>,
    ) -> Result<(), SegmentError> {
        if sha256_hex(&bytes) != locator.digest {
            return Err(SegmentError::DictionaryDigestMismatch(locator));
        }
        self.dictionaries.insert(locator, bytes);
        Ok(())
    }
}

impl DictionaryResolver for MemoryDictionaryResolver {
    fn resolve(&self, locator: &DictionaryLocator) -> Result<Vec<u8>, SegmentError> {
        self.dictionaries
            .get(locator)
            .cloned()
            .ok_or_else(|| SegmentError::MissingDictionary(locator.clone()))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedSegment {
    pub manifest: SegmentManifest,
    pub records: Vec<EvidenceRecord>,
}

pub fn inspect_segment_bytes(bytes: &[u8]) -> Result<SegmentManifest, SegmentError> {
    parse_container(bytes).map(|parsed| parsed.manifest)
}

pub fn inspect_segment_file(path: &Path) -> Result<SegmentManifest, SegmentError> {
    let bytes = read_bounded(path)?;
    inspect_segment_bytes(&bytes)
}

pub fn decode_segment_bytes(
    bytes: &[u8],
    resolver: &dyn DictionaryResolver,
) -> Result<DecodedSegment, SegmentError> {
    let parsed = parse_container(bytes)?;
    let dictionary = if let Some(locator) = &parsed.manifest.compression.dictionary {
        let dictionary = resolver.resolve(locator)?;
        if sha256_hex(&dictionary) != locator.digest {
            return Err(SegmentError::DictionaryDigestMismatch(locator.clone()));
        }
        Some(dictionary)
    } else {
        None
    };
    validate_single_zstd_frame(parsed.payload)?;
    let mut decoder: Box<dyn Read> = if let Some(dictionary) = &dictionary {
        Box::new(
            zstd::stream::read::Decoder::with_dictionary(Cursor::new(parsed.payload), dictionary)
                .map_err(SegmentError::decompression)?
                .single_frame(),
        )
    } else {
        Box::new(
            zstd::stream::read::Decoder::new(Cursor::new(parsed.payload))
                .map_err(SegmentError::decompression)?
                .single_frame(),
        )
    };
    let declared_length = parsed.manifest.compression.frames[0].uncompressed_bytes;
    let mut content =
        Vec::with_capacity(usize::try_from(declared_length.min(16 * 1024 * 1024)).unwrap_or(0));
    decoder
        .by_ref()
        .take(MAX_UNCOMPRESSED_BYTES + 1)
        .read_to_end(&mut content)
        .map_err(SegmentError::decompression)?;
    let actual_length = u64::try_from(content.len()).unwrap_or(u64::MAX);
    if actual_length > MAX_UNCOMPRESSED_BYTES {
        return Err(SegmentError::ContentTooLarge(actual_length));
    }
    if actual_length != declared_length
        || sha256_hex(&content) != parsed.manifest.integrity.content_sha256
    {
        return Err(SegmentError::ContentChecksumMismatch);
    }
    let records = decode_rows(&content, parsed.manifest.row_count)?;
    parsed.manifest.validate_records(&records)?;
    Ok(DecodedSegment {
        manifest: parsed.manifest,
        records,
    })
}

fn validate_single_zstd_frame(payload: &[u8]) -> Result<(), SegmentError> {
    let first_frame_length =
        zstd::zstd_safe::find_frame_compressed_size(payload).map_err(|code| {
            SegmentError::DecompressionFailed(zstd::zstd_safe::get_error_name(code).to_owned())
        })?;
    if first_frame_length != payload.len() {
        return Err(SegmentError::DecompressionFailed(
            "format v1 payload contains more than one Zstandard frame".to_owned(),
        ));
    }
    Ok(())
}

pub fn decode_segment_file(
    path: &Path,
    resolver: &dyn DictionaryResolver,
) -> Result<DecodedSegment, SegmentError> {
    let bytes = read_bounded(path)?;
    decode_segment_bytes(&bytes, resolver)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, SegmentError> {
    let length = fs::metadata(path).map_err(SegmentError::io)?.len();
    let maximum = u64::try_from(PRELUDE_LENGTH + MAX_MANIFEST_BYTES + TRAILER_LENGTH)
        .unwrap_or(u64::MAX)
        .saturating_add(MAX_COMPRESSED_BYTES);
    if length > maximum {
        return Err(SegmentError::PayloadTooLarge(length));
    }
    fs::read(path).map_err(SegmentError::io)
}

struct ParsedContainer<'a> {
    manifest: SegmentManifest,
    payload: &'a [u8],
}

fn parse_container(bytes: &[u8]) -> Result<ParsedContainer<'_>, SegmentError> {
    if bytes.len() < PRELUDE_LENGTH {
        return Err(SegmentError::Truncated);
    }
    if &bytes[..8] != PRELUDE_MAGIC {
        return Err(SegmentError::InvalidMagic);
    }
    let version = u16::from_be_bytes([bytes[8], bytes[9]]);
    if version != FORMAT_VERSION {
        return Err(SegmentError::UnsupportedVersion(version));
    }
    let flags = u16::from_be_bytes([bytes[10], bytes[11]]);
    if flags & !SUPPORTED_FLAGS != 0 {
        return Err(SegmentError::UnsupportedFlags(flags));
    }
    let manifest_length = u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let manifest_length = usize::try_from(manifest_length).unwrap_or(usize::MAX);
    if manifest_length > MAX_MANIFEST_BYTES {
        return Err(SegmentError::HeaderTooLarge(
            u64::try_from(manifest_length).unwrap_or(u64::MAX),
        ));
    }
    let compressed_length = u64::from_be_bytes([
        bytes[16], bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
    ]);
    if compressed_length > MAX_COMPRESSED_BYTES {
        return Err(SegmentError::PayloadTooLarge(compressed_length));
    }
    let compressed_usize = usize::try_from(compressed_length)
        .map_err(|_| SegmentError::PayloadTooLarge(compressed_length))?;
    let expected_length = PRELUDE_LENGTH
        .checked_add(manifest_length)
        .and_then(|length| length.checked_add(compressed_usize))
        .and_then(|length| length.checked_add(TRAILER_LENGTH))
        .ok_or(SegmentError::PayloadTooLarge(compressed_length))?;
    if bytes.len() < expected_length {
        return Err(SegmentError::Truncated);
    }
    if bytes.len() > expected_length {
        return Err(SegmentError::TrailingData);
    }
    let manifest_start = PRELUDE_LENGTH;
    let manifest_end = manifest_start + manifest_length;
    let payload_end = manifest_end + compressed_usize;
    let trailer = &bytes[payload_end..];
    if &trailer[..8] != TRAILER_MAGIC {
        return Err(SegmentError::InvalidMagic);
    }
    let manifest_bytes = &bytes[manifest_start..manifest_end];
    if trailer[8..40] != sha256_bytes(manifest_bytes) {
        return Err(SegmentError::ManifestChecksumMismatch);
    }
    let manifest: SegmentManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|error| SegmentError::ManifestInvalid(error.to_string()))?;
    let payload = &bytes[manifest_end..payload_end];
    manifest.validate_shape(flags, compressed_length)?;
    if canonical_json(&manifest)? != manifest_bytes {
        return Err(SegmentError::ManifestInvalid(
            "manifest JSON is not canonical".to_owned(),
        ));
    }
    let payload_digest = sha256_bytes(payload);
    if trailer[40..72] != payload_digest
        || manifest.integrity.payload_sha256 != lower_hex(&payload_digest)
    {
        return Err(SegmentError::PayloadChecksumMismatch);
    }
    Ok(ParsedContainer { manifest, payload })
}

fn decode_rows(content: &[u8], expected_rows: u64) -> Result<Vec<EvidenceRecord>, SegmentError> {
    if !content.ends_with(b"\n") {
        return Err(SegmentError::RowInvalid {
            row: expected_rows.saturating_sub(1),
            message: "canonical JSONL must end with LF".to_owned(),
        });
    }
    let mut records = Vec::new();
    for (index, row) in content[..content.len() - 1]
        .split(|byte| *byte == b'\n')
        .enumerate()
    {
        let row_number = u64::try_from(index).unwrap_or(u64::MAX);
        if row.is_empty() {
            return Err(SegmentError::RowInvalid {
                row: row_number,
                message: "blank row".to_owned(),
            });
        }
        let record: EvidenceRecord =
            serde_json::from_slice(row).map_err(|error| SegmentError::RowInvalid {
                row: row_number,
                message: error.to_string(),
            })?;
        record
            .validate()
            .map_err(|error| SegmentError::RowInvalid {
                row: row_number,
                message: error.to_string(),
            })?;
        if canonical_json(&record)? != row {
            return Err(SegmentError::RowInvalid {
                row: row_number,
                message: "row JSON is not canonical".to_owned(),
            });
        }
        records.push(record);
    }
    let actual_rows = u64::try_from(records.len()).unwrap_or(u64::MAX);
    if actual_rows != expected_rows {
        return Err(SegmentError::RowInvalid {
            row: actual_rows,
            message: format!("manifest declares {expected_rows} rows, decoded {actual_rows}"),
        });
    }
    Ok(records)
}

fn canonical_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, SegmentError> {
    let value = serde_json::to_value(value)
        .map_err(|error| SegmentError::ManifestInvalid(error.to_string()))?;
    serde_json::to_vec(&value).map_err(|error| SegmentError::ManifestInvalid(error.to_string()))
}

fn classifications(records: &[EvidenceRecord]) -> BTreeSet<String> {
    records
        .iter()
        .map(|record| match record {
            EvidenceRecord::Observation(record) => record.classification.clone(),
            EvidenceRecord::Correction(record) => record.classification.clone(),
            EvidenceRecord::Frontier(record) => record.classification.clone(),
        })
        .collect()
}

fn ordering_bounds(records: &[EvidenceRecord]) -> OrderingBounds {
    let mut producer_sequences = BTreeMap::<(String, String), (u64, u64)>::new();
    let mut logical_clocks = BTreeMap::<String, (u64, u64)>::new();
    let mut observed_at = Vec::new();
    let mut observed_by_at = Vec::new();
    let mut recorded_at = Vec::new();

    for record in records {
        match record {
            EvidenceRecord::Observation(record) => {
                push_position(
                    &mut producer_sequences,
                    &record.producer.producer_id,
                    &record.producer.stream_id,
                    record.producer_sequence,
                );
                push_logical(&mut logical_clocks, record.logical_time.as_ref());
                if let Some(timestamp) = &record.observed_at {
                    observed_at.push(timestamp.clone());
                }
                observed_by_at.push(record.observed_by_at.clone());
                recorded_at.push(record.recorded_at.clone());
            }
            EvidenceRecord::Correction(record) => {
                push_position(
                    &mut producer_sequences,
                    &record.producer.producer_id,
                    &record.producer.stream_id,
                    record.producer_sequence,
                );
                push_logical(&mut logical_clocks, record.logical_time.as_ref());
                observed_by_at.push(record.observed_by_at.clone());
                recorded_at.push(record.recorded_at.clone());
            }
            EvidenceRecord::Frontier(record) => {
                push_position(
                    &mut producer_sequences,
                    &record.producer.producer_id,
                    &record.producer.stream_id,
                    record.producer_sequence_through,
                );
                push_logical(&mut logical_clocks, record.logical_time_through.as_ref());
                recorded_at.push(record.as_of_recorded_at.clone());
            }
        }
    }
    OrderingBounds {
        producer_sequences: producer_sequences
            .into_iter()
            .map(
                |((producer_id, stream_id), (min, max))| ProducerSequenceRange {
                    producer_id,
                    stream_id,
                    min,
                    max,
                },
            )
            .collect(),
        logical_clocks: logical_clocks
            .into_iter()
            .map(|(clock_id, (min, max))| LogicalRange { clock_id, min, max })
            .collect(),
        observed_at: time_range(observed_at),
        observed_by_at: time_range(observed_by_at),
        recorded_at: time_range(recorded_at),
    }
}

fn push_position(
    ranges: &mut BTreeMap<(String, String), (u64, u64)>,
    producer_id: &str,
    stream_id: &str,
    sequence: Option<u64>,
) {
    if let Some(sequence) = sequence {
        ranges
            .entry((producer_id.to_owned(), stream_id.to_owned()))
            .and_modify(|range| {
                range.0 = range.0.min(sequence);
                range.1 = range.1.max(sequence);
            })
            .or_insert((sequence, sequence));
    }
}

fn push_logical(
    ranges: &mut BTreeMap<String, (u64, u64)>,
    logical_time: Option<&fabric_time::LogicalTime>,
) {
    if let Some(logical_time) = logical_time {
        ranges
            .entry(logical_time.clock_id.clone())
            .and_modify(|range| {
                range.0 = range.0.min(logical_time.counter);
                range.1 = range.1.max(logical_time.counter);
            })
            .or_insert((logical_time.counter, logical_time.counter));
    }
}

fn time_range(mut timestamps: Vec<Timestamp>) -> Option<TimeRange> {
    if timestamps.is_empty() {
        return None;
    }
    timestamps.sort_by_key(timestamp_instant);
    Some(TimeRange {
        min: timestamps.first().expect("non-empty timestamps").clone(),
        max: timestamps.last().expect("non-empty timestamps").clone(),
    })
}

fn timestamp_instant(timestamp: &Timestamp) -> i128 {
    OffsetDateTime::parse(timestamp.as_str(), &Rfc3339)
        .expect("Timestamp guarantees RFC 3339")
        .unix_timestamp_nanos()
}

#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    lower_hex(&sha256_bytes(bytes))
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn lower_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_label(value: &str) -> bool {
    value.len() <= 64
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentErrorCategory {
    Io,
    InvalidMagic,
    UnsupportedVersion,
    UnsupportedFlags,
    HeaderTooLarge,
    PayloadTooLarge,
    ContentTooLarge,
    Truncated,
    TrailingData,
    ManifestChecksumMismatch,
    ManifestInvalid,
    PayloadChecksumMismatch,
    MissingDictionary,
    DictionaryDigestMismatch,
    DecompressionFailed,
    ContentChecksumMismatch,
    RowLimitExceeded,
    RowInvalid,
    ImmutableConflict,
    SchemaInvalid,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum SegmentError {
    #[error("segment IO failed: {0}")]
    Io(String),
    #[error("invalid segment magic")]
    InvalidMagic,
    #[error("unsupported segment version: {0}")]
    UnsupportedVersion(u16),
    #[error("unsupported segment flags: {0:#06x}")]
    UnsupportedFlags(u16),
    #[error("manifest is too large: {0} bytes")]
    HeaderTooLarge(u64),
    #[error("compressed payload is too large: {0} bytes")]
    PayloadTooLarge(u64),
    #[error("uncompressed content is too large: {0} bytes")]
    ContentTooLarge(u64),
    #[error("segment is truncated")]
    Truncated,
    #[error("segment has trailing data")]
    TrailingData,
    #[error("manifest checksum mismatch")]
    ManifestChecksumMismatch,
    #[error("invalid segment manifest: {0}")]
    ManifestInvalid(String),
    #[error("payload checksum mismatch")]
    PayloadChecksumMismatch,
    #[error("missing dictionary: {0:?}")]
    MissingDictionary(DictionaryLocator),
    #[error("dictionary digest mismatch: {0:?}")]
    DictionaryDigestMismatch(DictionaryLocator),
    #[error("Zstandard decompression failed: {0}")]
    DecompressionFailed(String),
    #[error("uncompressed content checksum or length mismatch")]
    ContentChecksumMismatch,
    #[error("row count is outside the supported range: {0}")]
    RowLimitExceeded(u64),
    #[error("invalid row {row}: {message}")]
    RowInvalid { row: u64, message: String },
    #[error("immutable segment path contains different bytes: {0}")]
    ImmutableConflict(PathBuf),
    #[error("evidence schema validation failed: {0}")]
    SchemaInvalid(String),
}

impl SegmentError {
    #[must_use]
    pub const fn category(&self) -> SegmentErrorCategory {
        match self {
            Self::Io(_) => SegmentErrorCategory::Io,
            Self::InvalidMagic => SegmentErrorCategory::InvalidMagic,
            Self::UnsupportedVersion(_) => SegmentErrorCategory::UnsupportedVersion,
            Self::UnsupportedFlags(_) => SegmentErrorCategory::UnsupportedFlags,
            Self::HeaderTooLarge(_) => SegmentErrorCategory::HeaderTooLarge,
            Self::PayloadTooLarge(_) => SegmentErrorCategory::PayloadTooLarge,
            Self::ContentTooLarge(_) => SegmentErrorCategory::ContentTooLarge,
            Self::Truncated => SegmentErrorCategory::Truncated,
            Self::TrailingData => SegmentErrorCategory::TrailingData,
            Self::ManifestChecksumMismatch => SegmentErrorCategory::ManifestChecksumMismatch,
            Self::ManifestInvalid(_) => SegmentErrorCategory::ManifestInvalid,
            Self::PayloadChecksumMismatch => SegmentErrorCategory::PayloadChecksumMismatch,
            Self::MissingDictionary(_) => SegmentErrorCategory::MissingDictionary,
            Self::DictionaryDigestMismatch(_) => SegmentErrorCategory::DictionaryDigestMismatch,
            Self::DecompressionFailed(_) => SegmentErrorCategory::DecompressionFailed,
            Self::ContentChecksumMismatch => SegmentErrorCategory::ContentChecksumMismatch,
            Self::RowLimitExceeded(_) => SegmentErrorCategory::RowLimitExceeded,
            Self::RowInvalid { .. } => SegmentErrorCategory::RowInvalid,
            Self::ImmutableConflict(_) => SegmentErrorCategory::ImmutableConflict,
            Self::SchemaInvalid(_) => SegmentErrorCategory::SchemaInvalid,
        }
    }

    fn io(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }

    fn compression(error: std::io::Error) -> Self {
        Self::DecompressionFailed(error.to_string())
    }

    fn decompression(error: std::io::Error) -> Self {
        Self::DecompressionFailed(error.to_string())
    }
}

impl From<SchemaError> for SegmentError {
    fn from(error: SchemaError) -> Self {
        Self::SchemaInvalid(error.to_string())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictionaryStatus {
    Active,
    Deprecated,
    Retained,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DictionaryEntry {
    pub family: String,
    pub version: u64,
    pub digest: String,
    pub byte_length: u64,
    pub training_corpus_digest: String,
    pub status: DictionaryStatus,
    pub activated_at: Timestamp,
    pub deprecated_at: Option<Timestamp>,
    pub retained_until: Option<Timestamp>,
}

impl DictionaryEntry {
    #[must_use]
    pub fn locator(&self) -> DictionaryLocator {
        DictionaryLocator {
            family: self.family.clone(),
            version: self.version,
            digest: self.digest.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DictionaryRegistry {
    pub api_version: String,
    pub registry_id: String,
    pub generated_at: Timestamp,
    pub entries: Vec<DictionaryEntry>,
    pub rollback_versions: BTreeSet<DictionaryLocator>,
}

impl DictionaryRegistry {
    pub fn validate(&self) -> Result<(), SegmentError> {
        if self.api_version != "fabric.dictionary-registry/v1" || self.registry_id.is_empty() {
            return Err(SegmentError::ManifestInvalid(
                "invalid dictionary registry identity".to_owned(),
            ));
        }
        let mut locators = BTreeSet::new();
        let mut active_families = BTreeSet::new();
        for entry in &self.entries {
            let locator = entry.locator();
            if entry.version == 0
                || entry.byte_length == 0
                || !is_lower_hex_digest(&entry.digest)
                || !is_lower_hex_digest(&entry.training_corpus_digest)
                || !locators.insert(locator)
            {
                return Err(SegmentError::ManifestInvalid(
                    "invalid or duplicate dictionary entry".to_owned(),
                ));
            }
            if entry.status == DictionaryStatus::Active
                && !active_families.insert(entry.family.clone())
            {
                return Err(SegmentError::ManifestInvalid(
                    "a dictionary family has multiple active versions".to_owned(),
                ));
            }
            if entry.status == DictionaryStatus::Active && entry.deprecated_at.is_some() {
                return Err(SegmentError::ManifestInvalid(
                    "active dictionary cannot have deprecated_at".to_owned(),
                ));
            }
        }
        if !self
            .rollback_versions
            .iter()
            .all(|locator| locators.contains(locator))
        {
            return Err(SegmentError::ManifestInvalid(
                "rollback locator is absent from entries".to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_schema::{Correction, Observation};
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

    fn options() -> EncodeOptions {
        EncodeOptions::new(Timestamp::parse("2026-07-20T15:00:00Z").expect("fixture timestamp"))
    }

    fn rebuild_container(manifest: &SegmentManifest, payload: &[u8], flags: u16) -> Vec<u8> {
        let manifest_bytes = canonical_json(manifest).expect("canonical manifest");
        let mut container = Vec::with_capacity(
            PRELUDE_LENGTH + manifest_bytes.len() + payload.len() + TRAILER_LENGTH,
        );
        container.extend_from_slice(PRELUDE_MAGIC);
        container.extend_from_slice(&FORMAT_VERSION.to_be_bytes());
        container.extend_from_slice(&flags.to_be_bytes());
        container.extend_from_slice(
            &u32::try_from(manifest_bytes.len())
                .expect("manifest length")
                .to_be_bytes(),
        );
        container.extend_from_slice(
            &u64::try_from(payload.len())
                .expect("payload length")
                .to_be_bytes(),
        );
        container.extend_from_slice(&manifest_bytes);
        container.extend_from_slice(payload);
        container.extend_from_slice(TRAILER_MAGIC);
        container.extend_from_slice(&sha256_bytes(&manifest_bytes));
        container.extend_from_slice(&sha256_bytes(payload));
        container
    }

    #[test]
    fn segment_round_trip_is_canonical_and_immutable() {
        let encoded = encode_segment(&records(), &options()).expect("encode segment");
        let decoded = decode_segment_bytes(&encoded.bytes, &NoDictionaries)
            .expect("decode segment without dictionary");
        assert_eq!(decoded.records, records());
        assert_eq!(decoded.manifest, encoded.manifest);

        let directory = tempdir().expect("temporary directory");
        let first_path = encoded
            .write_atomic(directory.path())
            .expect("first immutable write");
        let second_path = encoded
            .write_atomic(directory.path())
            .expect("idempotent immutable write");
        assert_eq!(first_path, second_path);
        assert!(
            fs::metadata(first_path)
                .expect("segment metadata")
                .permissions()
                .readonly()
        );
    }

    #[test]
    fn checksum_truncation_and_trailing_fail_distinctly() {
        let encoded = encode_segment(&records(), &options()).expect("encode segment");
        let manifest_length = u32::from_be_bytes([
            encoded.bytes[12],
            encoded.bytes[13],
            encoded.bytes[14],
            encoded.bytes[15],
        ]) as usize;
        let mut corrupt = encoded.bytes.clone();
        corrupt[PRELUDE_LENGTH + manifest_length] ^= 0x01;
        assert_eq!(
            decode_segment_bytes(&corrupt, &NoDictionaries)
                .expect_err("corrupt payload must fail")
                .category(),
            SegmentErrorCategory::PayloadChecksumMismatch
        );

        let truncated = &encoded.bytes[..encoded.bytes.len() - 1];
        assert_eq!(
            decode_segment_bytes(truncated, &NoDictionaries)
                .expect_err("truncated segment must fail")
                .category(),
            SegmentErrorCategory::Truncated
        );

        let mut trailing = encoded.bytes;
        trailing.push(0);
        assert_eq!(
            decode_segment_bytes(&trailing, &NoDictionaries)
                .expect_err("trailing data must fail")
                .category(),
            SegmentErrorCategory::TrailingData
        );
    }

    #[test]
    fn multiple_physical_frames_fail_even_when_the_container_is_consistent() {
        let records = records();
        let mut content_parts = Vec::new();
        for record in &records {
            let mut row = canonical_record_json(record).expect("canonical row");
            row.push(b'\n');
            content_parts.push(row);
        }
        let first_frame =
            zstd::stream::encode_all(Cursor::new(&content_parts[0]), 3).expect("first frame");
        let second_frame =
            zstd::stream::encode_all(Cursor::new(&content_parts[1]), 3).expect("second frame");
        let mut content = content_parts[0].clone();
        content.extend_from_slice(&content_parts[1]);
        let mut payload = first_frame;
        payload.extend_from_slice(&second_frame);

        let mut manifest = encode_segment(&records, &options())
            .expect("manifest template")
            .manifest;
        manifest.compression.frames[0].compressed_bytes =
            u64::try_from(payload.len()).expect("payload length");
        manifest.compression.frames[0].uncompressed_bytes =
            u64::try_from(content.len()).expect("content length");
        manifest.integrity.payload_sha256 = sha256_hex(&payload);
        manifest.integrity.content_sha256 = sha256_hex(&content);
        manifest.segment_id = format!("seg-{}", manifest.integrity.content_sha256);

        let container = rebuild_container(&manifest, &payload, FLAG_CONTENT_DIGEST);

        assert_eq!(
            decode_segment_bytes(&container, &NoDictionaries)
                .expect_err("multiple physical frames must fail")
                .category(),
            SegmentErrorCategory::DecompressionFailed
        );
    }

    #[test]
    fn manifest_pruning_metadata_must_match_canonical_records() {
        let encoded = encode_segment(&records(), &options()).expect("encode segment");
        let manifest_length = usize::try_from(u32::from_be_bytes([
            encoded.bytes[12],
            encoded.bytes[13],
            encoded.bytes[14],
            encoded.bytes[15],
        ]))
        .expect("manifest length");
        let payload_length = usize::try_from(u64::from_be_bytes([
            encoded.bytes[16],
            encoded.bytes[17],
            encoded.bytes[18],
            encoded.bytes[19],
            encoded.bytes[20],
            encoded.bytes[21],
            encoded.bytes[22],
            encoded.bytes[23],
        ]))
        .expect("payload length");
        let payload_start = PRELUDE_LENGTH + manifest_length;
        let payload = &encoded.bytes[payload_start..payload_start + payload_length];

        let mut false_classification = encoded.manifest.clone();
        false_classification.classifications = BTreeSet::from(["fabric-review-probe".to_owned()]);
        let mut false_ordering_bound = encoded.manifest.clone();
        false_ordering_bound.ordering_bounds.producer_sequences[0].max += 1;

        for (case, manifest) in [
            ("classifications", false_classification),
            ("ordering bounds", false_ordering_bound),
        ] {
            let container = rebuild_container(&manifest, payload, FLAG_CONTENT_DIGEST);
            assert_eq!(
                decode_segment_bytes(&container, &NoDictionaries)
                    .expect_err(case)
                    .category(),
                SegmentErrorCategory::ManifestInvalid,
                "{case}"
            );
        }
    }

    #[test]
    fn dictionary_is_digest_addressed_and_required() {
        let dictionary =
            b"fabric observation correction producer stream recorded_at payload".repeat(32);
        let locator = DictionaryLocator {
            family: "structured-evidence".to_owned(),
            version: 1,
            digest: sha256_hex(&dictionary),
        };
        let mut dictionary_options = options();
        dictionary_options.dictionary = Some(
            DictionaryMaterial::new(locator.clone(), dictionary.clone())
                .expect("valid dictionary material"),
        );
        let encoded =
            encode_segment(&records(), &dictionary_options).expect("encode dictionary segment");
        assert_eq!(
            decode_segment_bytes(&encoded.bytes, &NoDictionaries)
                .expect_err("missing dictionary must fail")
                .category(),
            SegmentErrorCategory::MissingDictionary
        );
        let mut resolver = MemoryDictionaryResolver::default();
        resolver
            .insert(locator, dictionary)
            .expect("insert dictionary");
        assert_eq!(
            decode_segment_bytes(&encoded.bytes, &resolver)
                .expect("dictionary decode")
                .records,
            records()
        );
    }

    #[test]
    fn registry_fixture_has_one_valid_active_version() {
        let registry: DictionaryRegistry = serde_json::from_str(include_str!(
            "../../../fixtures/segment-format/valid/dictionary-registry.json"
        ))
        .expect("registry fixture");
        registry.validate().expect("valid registry");
    }

    #[test]
    fn sealed_byte_fixtures_map_to_the_declared_categories() {
        let valid = decode_hex(include_str!(
            "../../../fixtures/segment-format/binary/valid.hex"
        ));
        assert_eq!(
            decode_segment_bytes(&valid, &NoDictionaries)
                .expect("valid byte fixture")
                .records,
            records()
        );

        let corrupt = decode_hex(include_str!(
            "../../../fixtures/segment-format/binary/corrupt-payload.hex"
        ));
        assert_eq!(
            decode_segment_bytes(&corrupt, &NoDictionaries)
                .expect_err("corrupt byte fixture")
                .category(),
            SegmentErrorCategory::PayloadChecksumMismatch
        );

        let truncated = decode_hex(include_str!(
            "../../../fixtures/segment-format/binary/truncated.hex"
        ));
        assert_eq!(
            decode_segment_bytes(&truncated, &NoDictionaries)
                .expect_err("truncated byte fixture")
                .category(),
            SegmentErrorCategory::Truncated
        );

        let missing_dictionary = decode_hex(include_str!(
            "../../../fixtures/segment-format/binary/missing-dictionary.hex"
        ));
        assert_eq!(
            decode_segment_bytes(&missing_dictionary, &NoDictionaries)
                .expect_err("missing-dictionary byte fixture")
                .category(),
            SegmentErrorCategory::MissingDictionary
        );
    }

    fn decode_hex(text: &str) -> Vec<u8> {
        let text = text.trim();
        assert_eq!(text.len() % 2, 0, "hex fixture length must be even");
        text.as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let pair = std::str::from_utf8(pair).expect("hex fixture is ASCII");
                u8::from_str_radix(pair, 16).expect("hex fixture byte")
            })
            .collect()
    }
}
