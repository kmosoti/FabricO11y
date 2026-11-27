//! Revision-bound byte passages in caller-supplied captured source bytes.
//!
//! A source claim is not proof of custody, current availability, parser
//! correctness, authorization, or truth. The caller obtains and authorizes the
//! bytes; this module performs no I/O and never follows a locator. V1 limits the
//! entire supplied artifact to the existing corpus canonical byte budget.
use crate::{Corpus, MAX_CANONICAL_BYTES, RevisionDigest, SourceClaim};
use sha2::{Digest, Sha256};

/// V1 is an exact byte range, not a character or parser-derived offset.
/// The revision and index bind the claim from which artifact digest and parser
/// identity must be derived; neither is independently supplied by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CitationV1 {
    pub revision: RevisionDigest,
    pub source_index: u32,
    pub start: u64,
    pub end: u64,
    pub passage_sha256: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CitationError {
    MissingRevision,
    MissingSource,
    ExternalOnly,
    Unavailable,
    OversizedArtifact,
    InvalidRange,
    ArtifactMismatch,
    PassageMismatch,
}

/// Bytes borrow the caller's artifact; parser identity borrows the corpus.
#[derive(Debug, PartialEq, Eq)]
pub struct ResolvedPassage<'artifact, 'corpus> {
    pub bytes: &'artifact [u8],
    pub parser: &'corpus str,
    pub artifact_sha256: [u8; 32],
}

fn artifact<'artifact, 'corpus>(
    corpus: &'corpus Corpus,
    revision: &RevisionDigest,
    source_index: u32,
    bytes: Option<&'artifact [u8]>,
) -> Result<(&'artifact [u8], &'corpus str, [u8; 32]), CitationError> {
    let record = corpus
        .revision(revision)
        .ok_or(CitationError::MissingRevision)?;
    let index = usize::try_from(source_index).map_err(|_| CitationError::MissingSource)?;
    let source = record
        .sources
        .get(index)
        .ok_or(CitationError::MissingSource)?;
    let SourceClaim::Captured {
        artifact_sha256,
        parser,
        ..
    } = source
    else {
        return Err(CitationError::ExternalOnly);
    };
    let bytes = bytes.ok_or(CitationError::Unavailable)?;
    if bytes.len() > MAX_CANONICAL_BYTES {
        return Err(CitationError::OversizedArtifact);
    }
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    if digest != *artifact_sha256 {
        return Err(CitationError::ArtifactMismatch);
    }
    Ok((bytes, parser, digest))
}

fn passage(bytes: &[u8], start: u64, end: u64) -> Result<&[u8], CitationError> {
    let start = usize::try_from(start).map_err(|_| CitationError::InvalidRange)?;
    let end = usize::try_from(end).map_err(|_| CitationError::InvalidRange)?;
    if start >= end {
        return Err(CitationError::InvalidRange);
    }
    bytes.get(start..end).ok_or(CitationError::InvalidRange)
}

impl CitationV1 {
    /// Construct a citation only after verifying the selected captured artifact.
    pub fn cite(
        corpus: &Corpus,
        revision: RevisionDigest,
        source_index: u32,
        start: u64,
        end: u64,
        bytes: Option<&[u8]>,
    ) -> Result<Self, CitationError> {
        let (bytes, _, _) = artifact(corpus, &revision, source_index, bytes)?;
        let selected = passage(bytes, start, end)?;
        Ok(Self {
            revision,
            source_index,
            start,
            end,
            passage_sha256: Sha256::digest(selected).into(),
        })
    }

    /// Resolve only when both the artifact and exact selected bytes match.
    pub fn resolve<'artifact, 'corpus>(
        &self,
        corpus: &'corpus Corpus,
        bytes: Option<&'artifact [u8]>,
    ) -> Result<ResolvedPassage<'artifact, 'corpus>, CitationError> {
        let (bytes, parser, artifact_sha256) =
            artifact(corpus, &self.revision, self.source_index, bytes)?;
        let selected = passage(bytes, self.start, self.end)?;
        let digest: [u8; 32] = Sha256::digest(selected).into();
        if digest != self.passage_sha256 {
            return Err(CitationError::PassageMismatch);
        }
        Ok(ResolvedPassage {
            bytes: selected,
            parser,
            artifact_sha256,
        })
    }
}
