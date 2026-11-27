//! Bounded immutable membership views over the in-memory corpus.
//!
//! Selection is explicit: no timestamp or iteration order is treated as a
//! cutoff. A support group is an attributed claim about a conjunction, not an
//! assertion that its conclusion is true or entailed.
use crate::{Corpus, Error, Predicate, Relation, RelationDigest, RevisionBody, RevisionDigest};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_VIEW_REVISIONS: usize = 256;
pub const MAX_VIEW_RELATIONS: usize = 512;
pub const MAX_VIEW_GROUPS: usize = 128;
pub const MAX_GROUP_PREMISES: usize = 64;
pub const MAX_TOTAL_PREMISES: usize = 1024;
pub const MAX_VIEW_RECORD_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_ATTRIBUTION_BYTES: usize = 256;

/// Each selected edge claims that its premise supports this conclusion. The
/// attribution records the group claim, independently of each edge's author.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SupportGroup {
    pub conclusion: RevisionDigest,
    pub attributed_to: String,
    pub premises: BTreeMap<RevisionDigest, RelationDigest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewError {
    Limit,
    InvalidGroup,
    InvalidRecord(Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PremiseStatus {
    Present,
    MissingRevision,
    MissingRelation,
    MismatchedRelation,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupProjection {
    conclusion_present: bool,
    premises: BTreeMap<RevisionDigest, PremiseStatus>,
}
impl GroupProjection {
    pub fn conclusion_present(&self) -> bool {
        self.conclusion_present
    }
    pub fn premises(&self) -> &BTreeMap<RevisionDigest, PremiseStatus> {
        &self.premises
    }
    /// All selected records are present and the claimed edges match. This does
    /// not establish the truth, relevance, or independent custody of evidence.
    pub fn is_resolved(&self) -> bool {
        self.conclusion_present && self.premises.values().all(|p| *p == PremiseStatus::Present)
    }
}

/// An owned snapshot, including missing selections and frozen group claims.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AsOfView {
    revisions: BTreeMap<RevisionDigest, Option<RevisionBody>>,
    relations: BTreeMap<RelationDigest, Option<Relation>>,
    groups: BTreeMap<SupportGroup, GroupProjection>,
}

impl AsOfView {
    /// Rejects oversized selections before copying records. Callers must bound
    /// decoding/staging and reject duplicate set entries before construction.
    pub fn capture(
        corpus: &Corpus,
        revision_ids: BTreeSet<RevisionDigest>,
        relation_ids: BTreeSet<RelationDigest>,
        groups: BTreeSet<SupportGroup>,
    ) -> Result<Self, ViewError> {
        if revision_ids.len() > MAX_VIEW_REVISIONS
            || relation_ids.len() > MAX_VIEW_RELATIONS
            || groups.len() > MAX_VIEW_GROUPS
        {
            return Err(ViewError::Limit);
        }
        let mut total_premises = 0usize;
        for group in &groups {
            if group.attributed_to.is_empty()
                || group.attributed_to.len() > MAX_ATTRIBUTION_BYTES
                || group.attributed_to.chars().any(char::is_control)
                || group.premises.is_empty()
            {
                return Err(ViewError::InvalidGroup);
            }
            if group.premises.len() > MAX_GROUP_PREMISES {
                return Err(ViewError::Limit);
            }
            total_premises = total_premises
                .checked_add(group.premises.len())
                .ok_or(ViewError::Limit)?;
            if total_premises > MAX_TOTAL_PREMISES {
                return Err(ViewError::Limit);
            }
        }
        let mut total_bytes = 0usize;
        for id in &revision_ids {
            if let Some(record) = corpus.revision(id) {
                let size = record
                    .canonical_bytes()
                    .map_err(ViewError::InvalidRecord)?
                    .len();
                total_bytes = total_bytes.checked_add(size).ok_or(ViewError::Limit)?;
                if total_bytes > MAX_VIEW_RECORD_BYTES {
                    return Err(ViewError::Limit);
                }
            }
        }
        for id in &relation_ids {
            if let Some(record) = corpus.relations().get(id) {
                let size = record
                    .canonical_bytes()
                    .map_err(ViewError::InvalidRecord)?
                    .len();
                total_bytes = total_bytes.checked_add(size).ok_or(ViewError::Limit)?;
                if total_bytes > MAX_VIEW_RECORD_BYTES {
                    return Err(ViewError::Limit);
                }
            }
        }
        let revisions: BTreeMap<_, _> = revision_ids
            .into_iter()
            .map(|id| (id, corpus.revision(&id).cloned()))
            .collect();
        let relations: BTreeMap<_, _> = relation_ids
            .into_iter()
            .map(|id| (id, corpus.relations().get(&id).cloned()))
            .collect();
        let groups = groups
            .into_iter()
            .map(|group| {
                let conclusion_present = revisions
                    .get(&group.conclusion)
                    .is_some_and(Option::is_some);
                let premises = group
                    .premises
                    .iter()
                    .map(|(premise, relation)| {
                        let status = if !revisions.get(premise).is_some_and(Option::is_some) {
                            PremiseStatus::MissingRevision
                        } else {
                            match relations.get(relation).and_then(Option::as_ref) {
                                None => PremiseStatus::MissingRelation,
                                Some(edge)
                                    if edge.subject == *premise
                                        && edge.object == group.conclusion
                                        && edge.predicate == Predicate::Supports =>
                                {
                                    PremiseStatus::Present
                                }
                                Some(_) => PremiseStatus::MismatchedRelation,
                            }
                        };
                        (*premise, status)
                    })
                    .collect();
                (
                    group,
                    GroupProjection {
                        conclusion_present,
                        premises,
                    },
                )
            })
            .collect();
        Ok(Self {
            revisions,
            relations,
            groups,
        })
    }
    /// Ordered explicit membership, including missing selected records.
    pub fn revisions(&self) -> &BTreeMap<RevisionDigest, Option<RevisionBody>> {
        &self.revisions
    }
    pub fn relations(&self) -> &BTreeMap<RelationDigest, Option<Relation>> {
        &self.relations
    }
    /// Outer None means not selected; inner None means selected but missing.
    pub fn revision(&self, id: &RevisionDigest) -> Option<&Option<RevisionBody>> {
        self.revisions.get(id)
    }
    /// Relation bodies retain their original predicate, attribution and extensions.
    pub fn relation(&self, id: &RelationDigest) -> Option<&Option<Relation>> {
        self.relations.get(id)
    }
    pub fn groups(&self) -> &BTreeMap<SupportGroup, GroupProjection> {
        &self.groups
    }
}
