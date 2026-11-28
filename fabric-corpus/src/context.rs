//! Frozen caller-selected context; no authorization, custody or truth assertion.
use crate::{
    SourceClaim,
    as_of::AsOfView,
    source::{CitationError, CitationV1, ResolvedPassage, source_claim},
};

pub const MAX_CONTEXT_CITATIONS: usize = 128;
pub const MAX_CONTEXT_SCAN_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_CONTEXT_PASSAGE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct CitationInput<'a> {
    pub citation: &'a CitationV1,
    pub bytes: Option<&'a [u8]>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum PassageState<'a, 'view> {
    Verified(ResolvedPassage<'a, 'view>),
    Omitted,
    ExternalOnly,
    MissingRevision,
}
#[derive(Debug, PartialEq, Eq)]
pub struct ContextEntry<'a, 'view> {
    citation: &'a CitationV1,
    state: PassageState<'a, 'view>,
}
/// The immutable selected view preserves all frozen dissent and relation claims.
#[derive(Debug)]
pub struct FrozenContext<'a, 'view> {
    view: &'view AsOfView,
    entries: Vec<ContextEntry<'a, 'view>>,
    supplied_bytes: usize,
    requested_passage_bytes: u64,
}
impl<'a, 'view> ContextEntry<'a, 'view> {
    pub fn citation(&self) -> &'a CitationV1 {
        self.citation
    }
    pub fn state(&self) -> &PassageState<'a, 'view> {
        &self.state
    }
}
impl<'a, 'view> FrozenContext<'a, 'view> {
    pub fn view(&self) -> &'view AsOfView {
        self.view
    }
    pub fn entries(&self) -> &[ContextEntry<'a, 'view>] {
        &self.entries
    }
    pub fn supplied_bytes(&self) -> usize {
        self.supplied_bytes
    }
    pub fn requested_passage_bytes(&self) -> u64 {
        self.requested_passage_bytes
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextError {
    Limit,
    UnselectedRevision,
    Citation(CitationError),
}

/// Checks aggregate budgets before hashing; invalid bytes reject the context.
pub fn assemble<'a, 'view>(
    view: &'view AsOfView,
    inputs: &[CitationInput<'a>],
) -> Result<FrozenContext<'a, 'view>, ContextError> {
    if inputs.len() > MAX_CONTEXT_CITATIONS {
        return Err(ContextError::Limit);
    }
    let mut supplied = 0usize;
    let mut requested = 0u64;
    for input in inputs {
        supplied = supplied
            .checked_add(input.bytes.map_or(0, <[u8]>::len))
            .ok_or(ContextError::Limit)?;
        requested = requested
            .checked_add(
                input
                    .citation
                    .end
                    .checked_sub(input.citation.start)
                    .filter(|n| *n > 0)
                    .ok_or(ContextError::Citation(CitationError::InvalidRange))?,
            )
            .ok_or(ContextError::Limit)?;
        if supplied > MAX_CONTEXT_SCAN_BYTES || requested > MAX_CONTEXT_PASSAGE_BYTES {
            return Err(ContextError::Limit);
        }
    }
    let mut entries = Vec::with_capacity(inputs.len());
    for input in inputs {
        let selected = view
            .revision(&input.citation.revision)
            .ok_or(ContextError::UnselectedRevision)?;
        let state = match selected {
            None => PassageState::MissingRevision,
            Some(body) => match source_claim(body, input.citation.source_index)
                .map_err(ContextError::Citation)?
            {
                SourceClaim::ExternalOnly { .. } if input.bytes.is_none() => {
                    PassageState::ExternalOnly
                }
                SourceClaim::ExternalOnly { .. } => {
                    return Err(ContextError::Citation(CitationError::ExternalOnly));
                }
                SourceClaim::Captured { .. } if input.bytes.is_none() => PassageState::Omitted,
                SourceClaim::Captured { .. } => PassageState::Verified(
                    input
                        .citation
                        .resolve_body(body, input.bytes)
                        .map_err(ContextError::Citation)?,
                ),
            },
        };
        entries.push(ContextEntry {
            citation: input.citation,
            state,
        });
    }
    Ok(FrozenContext {
        view,
        entries,
        supplied_bytes: supplied,
        requested_passage_bytes: requested,
    })
}
