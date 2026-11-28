//! Bounded in-memory representations. Abstracts are unverified derivative claims;
//! this module grants no expansion, authorization, custody, or truth.
use crate::{
    RevisionDigest,
    context::{self, CitationInput, PassageState},
    enrichment::Prepared,
    handoff::{Continuation, Handoff, Marker},
    portable::PortableView,
    source::CitationV1,
};
pub const MAX_ENTRIES: usize = 64;
pub const MAX_SUPPLIED_BYTES: usize = 8 * 1024 * 1024;
#[derive(Clone, Copy, Debug)]
pub enum Choice<'a> {
    Raw(&'a [u8]),
    ProvidedAbstract {
        bytes: &'a [u8],
        digest: [u8; 32],
        expected: &'a Prepared,
        stored: &'a Prepared,
    },
    Omitted,
}
#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    pub revision: RevisionDigest,
    pub citation: &'a CitationV1,
    pub choice: Choice<'a>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selected<'a> {
    Raw(&'a [u8]),
    ProvidedAbstract(&'a [u8]),
    Omitted,
}
#[derive(Debug, PartialEq, Eq)]
pub struct SelectedEntry<'a> {
    pub citation: &'a CitationV1,
    pub representation: Selected<'a>,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Selection<'a> {
    pub entries: Vec<SelectedEntry<'a>>,
    pub supplied_bytes: usize,
    pub dissent: Marker,
    pub evidence_omission: Marker,
    pub continuation: Continuation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Limit,
    Coverage,
    Binding,
    Portable,
    Missing,
    Raw,
    Abstract,
}
#[derive(Clone, Copy, Debug)]
pub struct Bindings<'a> {
    pub view: [u8; 32],
    pub root: RevisionDigest,
    pub policy: [u8; 32],
    pub scope: &'a str,
}
/// Scope and expected identities must be supplied independently by the caller.
/// Byte count is logical supplied payload, not token count or encoded size.
pub fn select<'a>(
    handoff: &Handoff,
    portable_bytes: &[u8],
    bindings: Bindings<'_>,
    entries: &[Entry<'a>],
    max_bytes: usize,
) -> Result<Selection<'a>, Error> {
    let Bindings {
        view: expected_view,
        root: expected_root,
        policy: expected_policy,
        scope,
    } = bindings;
    let d = handoff.declaration();
    if d.basis_digest != expected_view
        || d.root != expected_root
        || d.policy_id != expected_policy
        || scope.is_empty()
        || scope.len() > crate::enrichment::MAX_ID_BYTES
        || scope.chars().any(char::is_control)
    {
        return Err(Error::Binding);
    }
    if entries.len() > MAX_ENTRIES || max_bytes > MAX_SUPPLIED_BYTES {
        return Err(Error::Limit);
    }
    let mut total = 0usize;
    if entries.len() != d.citations.len() {
        return Err(Error::Coverage);
    }
    for (entry, selected_citation) in entries.iter().zip(&d.citations) {
        if entry.citation != selected_citation {
            return Err(Error::Coverage);
        }
        if entry.citation.revision != entry.revision || !d.revisions.contains(&entry.revision) {
            return Err(Error::Binding);
        }
        let n = match entry.choice {
            Choice::Raw(b) => b.len(),
            Choice::ProvidedAbstract { bytes, .. } => bytes.len(),
            Choice::Omitted => 0,
        };
        total = total.checked_add(n).ok_or(Error::Limit)?;
        if total > max_bytes {
            return Err(Error::Limit);
        }
    }
    let portable =
        PortableView::import(portable_bytes, expected_view).map_err(|_| Error::Portable)?;
    let mut output = Vec::with_capacity(entries.len());
    for entry in entries {
        if !matches!(portable.view().revision(&entry.revision), Some(Some(_))) {
            return Err(Error::Missing);
        }
        let selected = match entry.choice {
            Choice::Raw(bytes) => {
                let inputs = [CitationInput {
                    citation: entry.citation,
                    bytes: Some(bytes),
                }];
                let context =
                    context::assemble(portable.view(), &inputs).map_err(|_| Error::Raw)?;
                if !matches!(context.entries()[0].state(), PassageState::Verified(_)) {
                    return Err(Error::Raw);
                }
                Selected::Raw(bytes)
            }
            Choice::ProvidedAbstract {
                bytes,
                digest,
                expected,
                stored,
            } => {
                if !expected.binds(expected_view, expected_root, entry.revision, scope) {
                    return Err(Error::Binding);
                }
                expected
                    .validate_cached(stored, bytes, bytes.len(), digest)
                    .map_err(|_| Error::Abstract)?;
                Selected::ProvidedAbstract(bytes)
            }
            Choice::Omitted => Selected::Omitted,
        };
        output.push(SelectedEntry {
            citation: entry.citation,
            representation: selected,
        });
    }
    Ok(Selection {
        entries: output,
        supplied_bytes: total,
        dissent: d.dissent,
        evidence_omission: d.evidence_omission,
        continuation: d.continuation,
    })
}
