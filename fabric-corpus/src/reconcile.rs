//! Bounded analysis and explicit reconciliation of a frozen selected revision DAG.
//! A selected view is not a global head query or conditional-write authority.
use crate::{Corpus, Error, RevisionBody, RevisionDigest, as_of::AsOfView};
use std::collections::BTreeSet;

pub const MAX_ANCESTRY_LINKS: usize = 4096;
pub const MAX_UNSELECTED_ANCESTORS: usize = 256;
pub const MAX_OBJECT_ID_BYTES: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadAnalysis {
    pub heads: BTreeSet<RevisionDigest>,
    /// Every selected missing revision is unknown, including its object identity.
    pub missing_selected: BTreeSet<RevisionDigest>,
    /// Direct ancestors referenced by selected same-object revisions but not selected.
    pub unselected_ancestry: BTreeSet<RevisionDigest>,
    pub ancestry_links: usize,
}
impl HeadAnalysis {
    pub fn is_complete(&self) -> bool {
        self.missing_selected.is_empty() && self.unselected_ancestry.is_empty()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcileError {
    InvalidObject,
    Limit,
    CrossObjectAncestry,
    ViewMismatch,
    Incomplete(HeadAnalysis),
    InsufficientHeads,
    ParentMismatch,
    Admission(Error),
}

/// Finds maximal present same-object revisions. The result is complete only if
/// all selections are present and every same-object ancestry link is selected.
pub fn analyze(view: &AsOfView, object_id: &str) -> Result<HeadAnalysis, ReconcileError> {
    if object_id.is_empty() || object_id.len() > MAX_OBJECT_ID_BYTES {
        return Err(ReconcileError::InvalidObject);
    }
    let mut result = HeadAnalysis {
        heads: BTreeSet::new(),
        missing_selected: BTreeSet::new(),
        unselected_ancestry: BTreeSet::new(),
        ancestry_links: 0,
    };
    for (id, body) in view.revisions() {
        match body {
            None => {
                result.missing_selected.insert(*id);
            }
            Some(body) if body.object_id == object_id => {
                result.ancestry_links = result
                    .ancestry_links
                    .checked_add(body.parents.len())
                    .ok_or(ReconcileError::Limit)?;
                if result.ancestry_links > MAX_ANCESTRY_LINKS {
                    return Err(ReconcileError::Limit);
                }
                result.heads.insert(*id);
            }
            Some(_) => {}
        }
    }
    for body in view
        .revisions()
        .values()
        .flatten()
        .filter(|b| b.object_id == object_id)
    {
        for parent in &body.parents {
            match view.revision(parent) {
                None => {
                    if !result.unselected_ancestry.contains(parent)
                        && result.unselected_ancestry.len() == MAX_UNSELECTED_ANCESTORS
                    {
                        return Err(ReconcileError::Limit);
                    }
                    result.unselected_ancestry.insert(*parent);
                }
                Some(None) => { /* already included in missing_selected */ }
                Some(Some(ancestor)) if ancestor.object_id == object_id => {
                    result.heads.remove(parent);
                }
                Some(Some(_)) => return Err(ReconcileError::CrossObjectAncestry),
            }
        }
    }
    Ok(result)
}

/// Appends a new body whose parents are exactly the competing selected heads.
/// The frozen selection can omit concurrent heads; this operation does not claim
/// global completeness or provide atomic conditional writes against a store.
pub fn reconcile(
    corpus: &mut Corpus,
    view: &AsOfView,
    body: RevisionBody,
) -> Result<RevisionDigest, ReconcileError> {
    let analysis = analyze(view, &body.object_id)?;
    if !analysis.is_complete() {
        return Err(ReconcileError::Incomplete(analysis));
    }
    if analysis.heads.len() < 2 {
        return Err(ReconcileError::InsufficientHeads);
    }
    if body.parents != analysis.heads {
        return Err(ReconcileError::ParentMismatch);
    }
    for (id, selected) in view.revisions() {
        if let Some(selected) = selected
            && corpus.revision(id) != Some(selected)
        {
            return Err(ReconcileError::ViewMismatch);
        }
    }
    corpus.admit(body).map_err(ReconcileError::Admission)
}
