//! Compare explicitly selected dependencies in two caller-bound portable views.
//! From/To are labels, not verified chronology, authority or event order.
use crate::{
    RelationDigest, RevisionDigest,
    as_of::{
        GroupProjection, MAX_ATTRIBUTION_BYTES, MAX_VIEW_GROUPS, MAX_VIEW_RELATIONS,
        MAX_VIEW_REVISIONS, SupportGroup,
    },
    portable::{AvailabilityClaim, PortableError, PortableView},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GroupSelector {
    pub conclusion: RevisionDigest,
    pub attributed_to: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dependencies {
    pub revisions: BTreeSet<RevisionDigest>,
    pub relations: BTreeSet<RelationDigest>,
    pub groups: BTreeSet<GroupSelector>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevisionState {
    Omitted,
    Missing(AvailabilityClaim),
    Present(AvailabilityClaim),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationState {
    Omitted,
    Missing,
    Present,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroupState {
    Omitted,
    Selected(BTreeMap<SupportGroup, GroupProjection>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair<T> {
    pub from: T,
    pub to: T,
}
/// Stale indicates an observed selected dependency change; Unknown means no
/// known change but at least one dependency was omitted or selected missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applicability {
    Unchanged,
    Stale,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Differential {
    from_digest: [u8; 32],
    to_digest: [u8; 32],
    revisions: BTreeMap<RevisionDigest, Pair<RevisionState>>,
    relations: BTreeMap<RelationDigest, Pair<RelationState>>,
    groups: BTreeMap<GroupSelector, Pair<GroupState>>,
    applicability: Applicability,
}
impl Differential {
    pub fn from_digest(&self) -> [u8; 32] {
        self.from_digest
    }
    pub fn to_digest(&self) -> [u8; 32] {
        self.to_digest
    }
    pub fn revisions(&self) -> &BTreeMap<RevisionDigest, Pair<RevisionState>> {
        &self.revisions
    }
    pub fn relations(&self) -> &BTreeMap<RelationDigest, Pair<RelationState>> {
        &self.relations
    }
    pub fn groups(&self) -> &BTreeMap<GroupSelector, Pair<GroupState>> {
        &self.groups
    }
    pub fn applicability(&self) -> Applicability {
        self.applicability
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DifferentialError {
    Limit,
    InvalidSelector,
    From(PortableError),
    To(PortableError),
}
fn revision(view: &PortableView, id: &RevisionDigest) -> RevisionState {
    match view.view().revision(id) {
        None => RevisionState::Omitted,
        Some(None) => RevisionState::Missing(view.availability()[id]),
        Some(Some(_)) => RevisionState::Present(view.availability()[id]),
    }
}
fn relation(view: &PortableView, id: &RelationDigest) -> RelationState {
    match view.view().relation(id) {
        None => RelationState::Omitted,
        Some(None) => RelationState::Missing,
        Some(Some(_)) => RelationState::Present,
    }
}
fn group(view: &PortableView, selector: &GroupSelector) -> GroupState {
    let matches: BTreeMap<_, _> = view
        .view()
        .groups()
        .iter()
        .filter(|(g, _)| {
            g.conclusion == selector.conclusion && g.attributed_to == selector.attributed_to
        })
        .map(|(g, status)| (g.clone(), status.clone()))
        .collect();
    if matches.is_empty() {
        GroupState::Omitted
    } else {
        GroupState::Selected(matches)
    }
}
/// Imports and verifies both exact portable byte sequences against caller-supplied
/// expected digests. Those digests must be bound by the caller to trusted sources.
pub fn compare(
    from: &[u8],
    expected_from: [u8; 32],
    to: &[u8],
    expected_to: [u8; 32],
    dependencies: &Dependencies,
) -> Result<Differential, DifferentialError> {
    if dependencies.revisions.len() > MAX_VIEW_REVISIONS
        || dependencies.relations.len() > MAX_VIEW_RELATIONS
        || dependencies.groups.len() > MAX_VIEW_GROUPS
    {
        return Err(DifferentialError::Limit);
    }
    if dependencies.revisions.is_empty()
        && dependencies.relations.is_empty()
        && dependencies.groups.is_empty()
    {
        return Err(DifferentialError::InvalidSelector);
    }
    if dependencies.groups.iter().any(|g| {
        g.attributed_to.is_empty()
            || g.attributed_to.len() > MAX_ATTRIBUTION_BYTES
            || g.attributed_to.chars().any(char::is_control)
    }) {
        return Err(DifferentialError::InvalidSelector);
    }
    let before = PortableView::import(from, expected_from).map_err(DifferentialError::From)?;
    let after = PortableView::import(to, expected_to).map_err(DifferentialError::To)?;
    let mut stale = false;
    let mut unknown = false;
    let mut revisions = BTreeMap::new();
    for id in &dependencies.revisions {
        let pair = Pair {
            from: revision(&before, id),
            to: revision(&after, id),
        };
        match (&pair.from, &pair.to) {
            (RevisionState::Present(a), RevisionState::Present(b)) => stale |= a != b,
            _ => unknown = true,
        }
        revisions.insert(*id, pair);
    }
    let mut relations = BTreeMap::new();
    for id in &dependencies.relations {
        let pair = Pair {
            from: relation(&before, id),
            to: relation(&after, id),
        };
        if pair.from != RelationState::Present || pair.to != RelationState::Present {
            unknown = true;
        }
        relations.insert(*id, pair);
    }
    let mut groups = BTreeMap::new();
    for selector in &dependencies.groups {
        let pair = Pair {
            from: group(&before, selector),
            to: group(&after, selector),
        };
        match (&pair.from, &pair.to) {
            (GroupState::Selected(a), GroupState::Selected(b)) => {
                stale |= a != b;
                unknown |= a
                    .values()
                    .chain(b.values())
                    .any(|projection| !projection.is_resolved());
            }
            _ => unknown = true,
        }
        groups.insert(selector.clone(), pair);
    }
    let applicability = if stale {
        Applicability::Stale
    } else if unknown {
        Applicability::Unknown
    } else {
        Applicability::Unchanged
    };
    Ok(Differential {
        from_digest: expected_from,
        to_digest: expected_to,
        revisions,
        relations,
        groups,
        applicability,
    })
}
