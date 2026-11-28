//! Bounded, local archives of attempted worker segment and terminal events.
//!
//! This module records a finite transcript supplied by its caller. It does not
//! capture a live worker stream, authenticate a producer, reserve storage, or
//! establish custody. The caller must retain the returned digest independently
//! and choose an archive filename. Writes atomically replace that filename in
//! the supplied directory FD, so callers must use a dedicated archive
//! destination; this is not a durable authority or a worker checkpoint.

use crate::olap::{
    read_non_symlink_bytes_in_dir_bounded_if_present, write_atomic_with_in_dir_fd_pinned,
};
use fabric_core::{
    ArtifactContentDigest, DigestAlgorithm, DigestClaim, DigestRepresentation, PhysicalFileDigest,
    WorkerSegmentAdmission, WorkerSegmentCompleteness, WorkerSegmentError, WorkerSegmentScope,
    WorkerSegmentVerifier,
};
use std::{fmt, fs::File, io::Write};

const ARCHIVE_MAGIC: &[u8; 4] = b"FWCA";
const ARCHIVE_VERSION: u16 = 1;
const HEADER_BYTES: usize = 4 + 2 + 16 + 16 + 4;
const ATTEMPT_FIXED_BYTES: usize = 1 + 4 + 1 + 1 + 1;
const MAX_CLAIM_HEX_BYTES: usize = 64;

/// Maximum bytes in the complete encoded archive, including its header.
pub const MAX_WORKER_ARCHIVE_BYTES: usize = 4 * 1024 * 1024;
/// Maximum number of ordered events retained in one archive.
pub const MAX_WORKER_ARCHIVE_ATTEMPTS: usize = 1024;
/// Maximum exact event bytes retained for one attempted worker event.
pub const MAX_WORKER_ARCHIVE_EVENT_BYTES: usize = 1024 * 1024;

/// Which worker verifier entry point receives an archived event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptKind {
    Segment,
    Terminal,
}

/// One caller-supplied event attempt with its independent exact-byte claim.
#[derive(Clone, Copy, Debug)]
pub struct AttemptedEvent<'a> {
    pub kind: AttemptKind,
    pub bytes: &'a [u8],
    pub claim: &'a ArtifactContentDigest,
}

/// Receipt for the exact archive bytes written by [`write_archive`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveReceipt {
    pub scope: WorkerSegmentScope,
    pub digest: PhysicalFileDigest,
    pub bytes: u64,
    pub attempts: u32,
}

/// Result of replaying every archived attempt through the worker verifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveReplay {
    pub scope: WorkerSegmentScope,
    pub digest: PhysicalFileDigest,
    /// Same-scope verifier state. Foreign-scope attempts remain visible in
    /// `outcomes` but do not taint this scope, matching the worker verifier.
    pub completeness: WorkerSegmentCompleteness,
    pub accepted_attempts: u32,
    pub rejected_attempts: u32,
    pub outcomes: Vec<AttemptOutcome>,
}

/// Result for one archived attempt, in the original transcript order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttemptOutcome {
    pub kind: AttemptKind,
    pub result: Result<WorkerSegmentAdmission, WorkerSegmentError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveError {
    Limit,
    MissingArchive,
    DigestMismatch,
    ScopeMismatch,
    Malformed,
    UnsupportedVersion,
    Storage(String),
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Limit => f.write_str("worker event archive exceeds a configured bound"),
            Self::MissingArchive => f.write_str("worker event archive is missing"),
            Self::DigestMismatch => f.write_str("worker event archive digest does not match"),
            Self::ScopeMismatch => f.write_str("worker event archive scope does not match"),
            Self::Malformed => f.write_str("worker event archive is malformed"),
            Self::UnsupportedVersion => f.write_str("worker event archive version is unsupported"),
            Self::Storage(error) => write!(f, "worker event archive storage failed: {error}"),
        }
    }
}

impl std::error::Error for ArchiveError {}

/// Atomically writes a bounded archive in `dir_fd` under a caller-selected name.
///
/// The existing store writer may replace a regular file at `filename` as part
/// of its atomic rename. Callers must therefore dedicate the supplied
/// destination to this archive; the file is evidence for a local replay only,
/// not authority, custody, or an externally acknowledged capture.
pub fn write_archive(
    dir_fd: &File,
    filename: &str,
    scope: WorkerSegmentScope,
    attempts: &[AttemptedEvent<'_>],
) -> Result<ArchiveReceipt, ArchiveError> {
    let encoded = encode_archive(scope, attempts)?;
    let digest = physical_digest(&encoded);
    let attempts = u32::try_from(attempts.len()).map_err(|_| ArchiveError::Limit)?;
    let (published_file, published_identity) =
        write_atomic_with_in_dir_fd_pinned(dir_fd, filename, |file, tmp_name| {
            file.write_all(&encoded)
                .map_err(|error| format!("failed writing {tmp_name}: {error}"))
        })
        .map_err(ArchiveError::Storage)?;
    drop(published_file);
    if published_identity.len != Some(encoded.len() as u64)
        || published_identity.sha256.as_deref() != Some(digest.0.hex.as_str())
    {
        return Err(ArchiveError::DigestMismatch);
    }
    Ok(ArchiveReceipt {
        scope,
        digest,
        bytes: encoded.len() as u64,
        attempts,
    })
}

/// Opens a regular archive without following symlinks, verifies independent
/// expected digest and scope, then replays each attempt through the pure worker
/// verifier. Missing, torn, oversized, or mismatched files return an error and
/// never produce a completion result.
pub fn replay_archive(
    dir_fd: &File,
    filename: &str,
    expected_scope: WorkerSegmentScope,
    expected_digest: &PhysicalFileDigest,
) -> Result<ArchiveReplay, ArchiveError> {
    let bytes = read_non_symlink_bytes_in_dir_bounded_if_present(
        dir_fd,
        filename,
        "worker event archive",
        MAX_WORKER_ARCHIVE_BYTES as u64,
    )
    .map_err(ArchiveError::Storage)?
    .ok_or(ArchiveError::MissingArchive)?;
    if bytes.len() > MAX_WORKER_ARCHIVE_BYTES {
        return Err(ArchiveError::Limit);
    }
    expected_digest
        .verify_exact_bytes(&bytes)
        .map_err(|_| ArchiveError::DigestMismatch)?;
    let (archive_scope, attempts) = validate_archive(&bytes)?;
    if archive_scope != expected_scope {
        return Err(ArchiveError::ScopeMismatch);
    }

    let digest = physical_digest(&bytes);
    let mut verifier = WorkerSegmentVerifier::new(archive_scope);
    let mut outcomes = Vec::with_capacity(attempts as usize);
    let mut accepted_attempts = 0u32;
    let mut rejected_attempts = 0u32;
    let mut reader = Reader::new(&bytes);
    let _header = reader.header()?;
    for _ in 0..attempts {
        let kind = reader.kind()?;
        let event_len = reader.u32()? as usize;
        let event_bytes = reader.take(event_len)?;
        let claim = reader.claim()?;
        let result = match kind {
            AttemptKind::Segment => verifier.append(event_bytes, &claim),
            AttemptKind::Terminal => verifier.seal(event_bytes, &claim),
        };
        if result.is_ok() {
            accepted_attempts = accepted_attempts.saturating_add(1);
        } else {
            rejected_attempts = rejected_attempts.saturating_add(1);
        }
        outcomes.push(AttemptOutcome { kind, result });
    }
    debug_assert!(reader.is_done());
    Ok(ArchiveReplay {
        scope: archive_scope,
        digest,
        completeness: verifier.completeness(),
        accepted_attempts,
        rejected_attempts,
        outcomes,
    })
}

fn encode_archive(
    scope: WorkerSegmentScope,
    attempts: &[AttemptedEvent<'_>],
) -> Result<Vec<u8>, ArchiveError> {
    let size = encoded_size(attempts)?;
    let count = u32::try_from(attempts.len()).map_err(|_| ArchiveError::Limit)?;
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(ARCHIVE_MAGIC);
    out.extend_from_slice(&ARCHIVE_VERSION.to_be_bytes());
    out.extend_from_slice(&scope.job);
    out.extend_from_slice(&scope.epoch);
    out.extend_from_slice(&count.to_be_bytes());
    for attempt in attempts {
        out.push(match attempt.kind {
            AttemptKind::Segment => 1,
            AttemptKind::Terminal => 2,
        });
        out.extend_from_slice(&(attempt.bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(attempt.bytes);
        out.push(encode_algorithm(attempt.claim.0.algorithm));
        out.push(encode_representation(attempt.claim.0.representation));
        out.push(attempt.claim.0.hex.len() as u8);
        out.extend_from_slice(attempt.claim.0.hex.as_bytes());
    }
    debug_assert_eq!(out.len(), size);
    Ok(out)
}

fn encoded_size(attempts: &[AttemptedEvent<'_>]) -> Result<usize, ArchiveError> {
    if attempts.len() > MAX_WORKER_ARCHIVE_ATTEMPTS {
        return Err(ArchiveError::Limit);
    }
    let mut size = HEADER_BYTES;
    for attempt in attempts {
        if attempt.bytes.len() > MAX_WORKER_ARCHIVE_EVENT_BYTES
            || attempt.claim.0.hex.len() > MAX_CLAIM_HEX_BYTES
        {
            return Err(ArchiveError::Limit);
        }
        size = size
            .checked_add(ATTEMPT_FIXED_BYTES)
            .and_then(|sum| sum.checked_add(attempt.bytes.len()))
            .and_then(|sum| sum.checked_add(attempt.claim.0.hex.len()))
            .ok_or(ArchiveError::Limit)?;
        if size > MAX_WORKER_ARCHIVE_BYTES {
            return Err(ArchiveError::Limit);
        }
    }
    Ok(size)
}

fn physical_digest(bytes: &[u8]) -> PhysicalFileDigest {
    PhysicalFileDigest(DigestClaim {
        algorithm: DigestAlgorithm::Sha256,
        representation: DigestRepresentation::ExactBytes,
        hex: crate::olap::sha256_bytes_hex(bytes),
    })
}

fn encode_algorithm(value: DigestAlgorithm) -> u8 {
    match value {
        DigestAlgorithm::Sha256 => 1,
    }
}

fn encode_representation(value: DigestRepresentation) -> u8 {
    match value {
        DigestRepresentation::ExactBytes => 1,
        DigestRepresentation::CanonicalJsonV1 => 2,
    }
}

fn decode_algorithm(value: u8) -> Result<DigestAlgorithm, ArchiveError> {
    match value {
        1 => Ok(DigestAlgorithm::Sha256),
        _ => Err(ArchiveError::Malformed),
    }
}

fn decode_representation(value: u8) -> Result<DigestRepresentation, ArchiveError> {
    match value {
        1 => Ok(DigestRepresentation::ExactBytes),
        2 => Ok(DigestRepresentation::CanonicalJsonV1),
        _ => Err(ArchiveError::Malformed),
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, size: usize) -> Result<&'a [u8], ArchiveError> {
        let end = self.at.checked_add(size).ok_or(ArchiveError::Malformed)?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or(ArchiveError::Malformed)?;
        self.at = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ArchiveError> {
        self.take(N)?
            .try_into()
            .map_err(|_| ArchiveError::Malformed)
    }

    fn u8(&mut self) -> Result<u8, ArchiveError> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, ArchiveError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, ArchiveError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn header(&mut self) -> Result<(WorkerSegmentScope, u32), ArchiveError> {
        if self.take(ARCHIVE_MAGIC.len())? != ARCHIVE_MAGIC {
            return Err(ArchiveError::Malformed);
        }
        if self.u16()? != ARCHIVE_VERSION {
            return Err(ArchiveError::UnsupportedVersion);
        }
        let scope = WorkerSegmentScope {
            job: self.array()?,
            epoch: self.array()?,
        };
        let attempts = self.u32()?;
        if attempts as usize > MAX_WORKER_ARCHIVE_ATTEMPTS {
            return Err(ArchiveError::Limit);
        }
        Ok((scope, attempts))
    }

    fn kind(&mut self) -> Result<AttemptKind, ArchiveError> {
        match self.u8()? {
            1 => Ok(AttemptKind::Segment),
            2 => Ok(AttemptKind::Terminal),
            _ => Err(ArchiveError::Malformed),
        }
    }

    fn claim(&mut self) -> Result<ArtifactContentDigest, ArchiveError> {
        let algorithm = decode_algorithm(self.u8()?)?;
        let representation = decode_representation(self.u8()?)?;
        let hex_len = self.u8()? as usize;
        if hex_len > MAX_CLAIM_HEX_BYTES {
            return Err(ArchiveError::Limit);
        }
        let hex = std::str::from_utf8(self.take(hex_len)?)
            .map_err(|_| ArchiveError::Malformed)?
            .to_owned();
        Ok(ArtifactContentDigest(DigestClaim {
            algorithm,
            representation,
            hex,
        }))
    }

    fn is_done(&self) -> bool {
        self.at == self.bytes.len()
    }
}

fn validate_archive(bytes: &[u8]) -> Result<(WorkerSegmentScope, u32), ArchiveError> {
    if bytes.len() > MAX_WORKER_ARCHIVE_BYTES {
        return Err(ArchiveError::Limit);
    }
    let mut reader = Reader::new(bytes);
    let (scope, attempts) = reader.header()?;
    for _ in 0..attempts {
        let _kind = reader.kind()?;
        let event_bytes = reader.u32()? as usize;
        if event_bytes > MAX_WORKER_ARCHIVE_EVENT_BYTES {
            return Err(ArchiveError::Limit);
        }
        reader.take(event_bytes)?;
        let _claim = reader.claim()?;
    }
    if !reader.is_done() {
        return Err(ArchiveError::Malformed);
    }
    Ok((scope, attempts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_core::{WorkerSegmentScope, worker_segment};
    use std::{fs, path::Path};

    fn scope() -> WorkerSegmentScope {
        WorkerSegmentScope {
            job: [0x11; 16],
            epoch: [0x22; 16],
        }
    }

    fn claim(bytes: &[u8]) -> ArtifactContentDigest {
        ArtifactContentDigest(DigestClaim {
            algorithm: DigestAlgorithm::Sha256,
            representation: DigestRepresentation::ExactBytes,
            hex: crate::olap::sha256_bytes_hex(bytes),
        })
    }

    fn event<'a>(
        kind: AttemptKind,
        bytes: &'a [u8],
        claim: &'a ArtifactContentDigest,
    ) -> AttemptedEvent<'a> {
        AttemptedEvent { kind, bytes, claim }
    }

    fn segment(scope: WorkerSegmentScope, sequence: u64, previous: [u8; 32]) -> Vec<u8> {
        worker_segment::encode_segment(worker_segment::Segment {
            scope,
            sequence,
            previous,
            dropped: 0,
            records: &[b"event"],
        })
        .unwrap()
    }

    fn terminal(scope: WorkerSegmentScope, final_sequence: u64, final_digest: [u8; 32]) -> Vec<u8> {
        worker_segment::encode_terminal(worker_segment::Terminal {
            scope,
            final_sequence,
            final_digest,
            dropped: 0,
            declared_complete: true,
        })
    }

    fn fixture_dir() -> (tempfile::TempDir, File) {
        let temp = tempfile::tempdir_in(
            std::env::var_os("TMPDIR").expect("disk-backed TMPDIR for archive fixture"),
        )
        .unwrap();
        let dir_fd = crate::olap::open_standalone_dir_nofollow(temp.path(), "fixture dir")
            .unwrap()
            .unwrap();
        (temp, dir_fd)
    }

    fn path(temp: &tempfile::TempDir, filename: &str) -> std::path::PathBuf {
        Path::new(temp.path()).join(filename)
    }

    #[test]
    fn finite_attempt_archive_replays_actual_worker_state() {
        let (temp, dir_fd) = fixture_dir();
        let first = segment(scope(), 1, [0; 32]);
        let first_claim = claim(&first);
        let mut verifier = WorkerSegmentVerifier::new(scope());
        verifier.append(&first, &first_claim).unwrap();
        let seal = terminal(scope(), 1, verifier.digest());
        let seal_claim = claim(&seal);
        let attempts = [
            event(AttemptKind::Segment, &first, &first_claim),
            event(AttemptKind::Terminal, &seal, &seal_claim),
        ];
        let receipt = write_archive(&dir_fd, "events.fwca", scope(), &attempts).unwrap();
        assert_eq!(receipt.attempts, 2);
        let replay = replay_archive(&dir_fd, "events.fwca", scope(), &receipt.digest).unwrap();
        assert_eq!(replay.digest, receipt.digest);
        assert_eq!(replay.accepted_attempts, 2);
        assert_eq!(replay.rejected_attempts, 0);
        assert_eq!(replay.completeness, WorkerSegmentCompleteness::Complete);
        assert!(temp.path().is_dir());
    }

    #[test]
    fn missing_seal_mismatched_claim_and_same_scope_conflict_after_terminal_stay_incomplete() {
        let (_temp, dir_fd) = fixture_dir();
        let first = segment(scope(), 1, [0; 32]);
        let first_claim = claim(&first);
        let missing_seal = [event(AttemptKind::Segment, &first, &first_claim)];
        let receipt = write_archive(&dir_fd, "missing-seal.fwca", scope(), &missing_seal).unwrap();
        let replay =
            replay_archive(&dir_fd, "missing-seal.fwca", scope(), &receipt.digest).unwrap();
        assert_eq!(
            replay.completeness,
            WorkerSegmentCompleteness::MissingTerminal
        );

        let wrong_claim = claim(b"different bytes");
        let mut verifier = WorkerSegmentVerifier::new(scope());
        verifier.append(&first, &first_claim).unwrap();
        let seal = terminal(scope(), verifier.sequence(), verifier.digest());
        let seal_claim = claim(&seal);
        let mut changed = segment(scope(), 1, [0; 32]);
        *changed.last_mut().unwrap() ^= 1;
        let changed_claim = claim(&changed);
        let attempts = [
            event(AttemptKind::Segment, &first, &wrong_claim),
            event(AttemptKind::Segment, &first, &first_claim),
            event(AttemptKind::Terminal, &seal, &seal_claim),
            event(AttemptKind::Segment, &changed, &changed_claim),
        ];
        let receipt =
            write_archive(&dir_fd, "conflict-after-seal.fwca", scope(), &attempts).unwrap();
        let replay = replay_archive(
            &dir_fd,
            "conflict-after-seal.fwca",
            scope(),
            &receipt.digest,
        )
        .unwrap();
        assert_eq!(replay.accepted_attempts, 2);
        assert_eq!(replay.rejected_attempts, 2);
        assert_eq!(
            replay.completeness,
            WorkerSegmentCompleteness::RejectedInput
        );
        assert_eq!(replay.outcomes[0].result, Err(WorkerSegmentError::Digest));
        assert_eq!(
            replay.outcomes[1].result,
            Ok(WorkerSegmentAdmission::Inserted)
        );
        assert_eq!(
            replay.outcomes[2].result,
            Ok(WorkerSegmentAdmission::Inserted)
        );
        assert_eq!(replay.outcomes[3].result, Err(WorkerSegmentError::Conflict));
    }

    #[test]
    fn foreign_scope_attempt_is_preserved_without_changing_scoped_completion() {
        let (_temp, dir_fd) = fixture_dir();
        let first = segment(scope(), 1, [0; 32]);
        let first_claim = claim(&first);
        let mut verifier = WorkerSegmentVerifier::new(scope());
        verifier.append(&first, &first_claim).unwrap();
        let seal = terminal(scope(), 1, verifier.digest());
        let seal_claim = claim(&seal);
        let foreign = segment(
            WorkerSegmentScope {
                job: [0x99; 16],
                ..scope()
            },
            1,
            [0; 32],
        );
        let foreign_claim = claim(&foreign);
        let attempts = [
            event(AttemptKind::Segment, &first, &first_claim),
            event(AttemptKind::Terminal, &seal, &seal_claim),
            event(AttemptKind::Segment, &foreign, &foreign_claim),
        ];
        let receipt = write_archive(&dir_fd, "foreign.fwca", scope(), &attempts).unwrap();
        let replay = replay_archive(&dir_fd, "foreign.fwca", scope(), &receipt.digest).unwrap();
        assert_eq!(replay.rejected_attempts, 1);
        assert_eq!(replay.outcomes[2].result, Err(WorkerSegmentError::Scope));
        assert_eq!(replay.completeness, WorkerSegmentCompleteness::Complete);
    }

    #[test]
    fn missing_torn_wrong_digest_and_wrong_scope_never_return_completion() {
        let (temp, dir_fd) = fixture_dir();
        assert_eq!(
            replay_archive(&dir_fd, "absent.fwca", scope(), &physical_digest(b"")),
            Err(ArchiveError::MissingArchive)
        );

        let first = segment(scope(), 1, [0; 32]);
        let first_claim = claim(&first);
        let attempts = [event(AttemptKind::Segment, &first, &first_claim)];
        let receipt = write_archive(&dir_fd, "torn.fwca", scope(), &attempts).unwrap();
        assert_eq!(
            replay_archive(&dir_fd, "torn.fwca", scope(), &physical_digest(b"wrong")),
            Err(ArchiveError::DigestMismatch)
        );
        fs::write(path(&temp, "torn.fwca"), b"FWCA\0").unwrap();
        let torn_digest = physical_digest(b"FWCA\0");
        assert_eq!(
            replay_archive(&dir_fd, "torn.fwca", scope(), &torn_digest),
            Err(ArchiveError::Malformed)
        );
        assert_eq!(
            replay_archive(
                &dir_fd,
                "events.fwca",
                WorkerSegmentScope {
                    job: [3; 16],
                    ..scope()
                },
                &receipt.digest
            ),
            Err(ArchiveError::MissingArchive)
        );
        write_archive(&dir_fd, "scope.fwca", scope(), &attempts).unwrap();
        assert_eq!(
            replay_archive(
                &dir_fd,
                "scope.fwca",
                WorkerSegmentScope {
                    job: [3; 16],
                    ..scope()
                },
                &receipt.digest
            ),
            Err(ArchiveError::ScopeMismatch)
        );
    }

    #[test]
    fn count_event_and_total_budgets_refuse_before_writing() {
        let (_temp, dir_fd) = fixture_dir();
        let bytes = vec![0; MAX_WORKER_ARCHIVE_EVENT_BYTES];
        let digest = claim(&bytes);
        let oversized = vec![0; MAX_WORKER_ARCHIVE_EVENT_BYTES + 1];
        let oversized_claim = claim(&oversized);
        let attempts = [event(AttemptKind::Segment, &oversized, &oversized_claim)];
        assert_eq!(
            write_archive(&dir_fd, "oversized.fwca", scope(), &attempts),
            Err(ArchiveError::Limit)
        );

        let repeated = vec![event(AttemptKind::Segment, &bytes, &digest); 5];
        assert_eq!(
            write_archive(&dir_fd, "quota.fwca", scope(), &repeated),
            Err(ArchiveError::Limit)
        );
        let small_claim = claim(b"x");
        let too_many =
            vec![event(AttemptKind::Segment, b"x", &small_claim); MAX_WORKER_ARCHIVE_ATTEMPTS + 1];
        assert_eq!(encode_archive(scope(), &too_many), Err(ArchiveError::Limit));
        assert!(
            crate::olap::read_non_symlink_bytes_in_dir_bounded_if_present(
                &dir_fd,
                "oversized.fwca",
                "worker event archive",
                MAX_WORKER_ARCHIVE_BYTES as u64,
            )
            .unwrap()
            .is_none()
        );
        assert!(
            crate::olap::read_non_symlink_bytes_in_dir_bounded_if_present(
                &dir_fd,
                "quota.fwca",
                "worker event archive",
                MAX_WORKER_ARCHIVE_BYTES as u64,
            )
            .unwrap()
            .is_none()
        );
    }
}
