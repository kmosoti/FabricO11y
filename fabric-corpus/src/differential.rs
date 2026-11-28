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
fn invalid_group_selector(g: &GroupSelector) -> bool {
    g.attributed_to.is_empty()
        || g.attributed_to.len() > MAX_ATTRIBUTION_BYTES
        || g.attributed_to.chars().any(char::is_control)
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
    if dependencies.groups.iter().any(invalid_group_selector) {
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

/// Explicit caller-declared AND inputs for one alternative; support relations
/// are not automatically interpreted as dependencies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alternative {
    pub direct: Dependencies,
    pub derived: BTreeSet<RevisionDigest>,
}
pub const MAX_PROPAGATION_NODES: usize = 64;
pub const MAX_PROPAGATION_ALTERNATIVES: usize = 256;
pub const MAX_PROPAGATION_EDGES: usize = 2048;
pub const MAX_PROPAGATION_WORK: usize = 131_072;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropagatedNode {
    pub applicability: Applicability,
    pub alternatives: Vec<Applicability>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Propagation {
    pub differential: Differential,
    pub nodes: BTreeMap<RevisionDigest, PropagatedNode>,
    pub work: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropagationError {
    Limit,
    InvalidGraph,
    MissingHistoricalOutput,
    Differential(DifferentialError),
}
fn direct_state(
    alt: &Alternative,
    d: &Differential,
    work: &mut usize,
) -> Result<Applicability, PropagationError> {
    let mut stale = false;
    let mut unknown = false;
    for id in &alt.direct.revisions {
        *work = work.checked_add(1).ok_or(PropagationError::Limit)?;
        let p = &d.revisions[id];
        match (&p.from, &p.to) {
            (RevisionState::Present(a), RevisionState::Present(b)) => stale |= a != b,
            _ => unknown = true,
        }
    }
    for id in &alt.direct.relations {
        *work = work.checked_add(1).ok_or(PropagationError::Limit)?;
        let p = &d.relations[id];
        unknown |= p.from != RelationState::Present || p.to != RelationState::Present;
    }
    for id in &alt.direct.groups {
        *work = work.checked_add(1).ok_or(PropagationError::Limit)?;
        let p = &d.groups[id];
        match (&p.from, &p.to) {
            (GroupState::Selected(a), GroupState::Selected(b)) => {
                stale |= a != b;
                unknown |= a.values().chain(b.values()).any(|v| !v.is_resolved());
            }
            _ => unknown = true,
        }
    }
    if *work > MAX_PROPAGATION_WORK {
        return Err(PropagationError::Limit);
    }
    Ok(if stale {
        Applicability::Stale
    } else if unknown {
        Applicability::Unknown
    } else {
        Applicability::Unchanged
    })
}
fn evaluate_derived(
    base: Applicability,
    alt: &Alternative,
    states: &BTreeMap<RevisionDigest, Applicability>,
    work: &mut usize,
) -> Result<Applicability, PropagationError> {
    let mut state = base;
    for dependency in &alt.derived {
        *work = work.checked_add(1).ok_or(PropagationError::Limit)?;
        if *work > MAX_PROPAGATION_WORK {
            return Err(PropagationError::Limit);
        }
        match states[dependency] {
            Applicability::Stale => state = Applicability::Stale,
            Applicability::Unknown if state != Applicability::Stale => {
                state = Applicability::Unknown
            }
            _ => (),
        }
    }
    Ok(state)
}
fn alternatives_state(alternatives: &[Applicability]) -> Applicability {
    if alternatives.contains(&Applicability::Unchanged) {
        Applicability::Unchanged
    } else if alternatives.iter().all(|s| *s == Applicability::Stale) {
        Applicability::Stale
    } else {
        Applicability::Unknown
    }
}
/// Compares a caller-declared finite graph across two portable byte sequences.
/// A historical output must be selected and present in the first view. No
/// authority, chronology, truth or completeness beyond the declaration is implied.
pub fn propagate(
    from: &[u8],
    expected_from: [u8; 32],
    to: &[u8],
    expected_to: [u8; 32],
    graph: &BTreeMap<RevisionDigest, Vec<Alternative>>,
) -> Result<Propagation, PropagationError> {
    if graph.is_empty() || graph.len() > MAX_PROPAGATION_NODES {
        return Err(PropagationError::Limit);
    }
    let mut union = Dependencies::default();
    let mut count = 0usize;
    let mut edges = 0usize;
    for (id, alternatives) in graph {
        if alternatives.is_empty() {
            return Err(PropagationError::InvalidGraph);
        }
        union.revisions.insert(*id);
        count = count
            .checked_add(alternatives.len())
            .ok_or(PropagationError::Limit)?;
        if count > MAX_PROPAGATION_ALTERNATIVES {
            return Err(PropagationError::Limit);
        }
        for alt in alternatives {
            let n = alt
                .direct
                .revisions
                .len()
                .checked_add(alt.direct.relations.len())
                .and_then(|v| v.checked_add(alt.direct.groups.len()))
                .and_then(|v| v.checked_add(alt.derived.len()))
                .ok_or(PropagationError::Limit)?;
            edges = edges.checked_add(n).ok_or(PropagationError::Limit)?;
            if edges > MAX_PROPAGATION_EDGES {
                return Err(PropagationError::Limit);
            }
            // Output dependencies must flow through the derived graph. Direct
            // relation/group selectors remain explicit metadata, without inferred edges.
            if n == 0
                || alt.derived.iter().any(|node| !graph.contains_key(node))
                || alt
                    .direct
                    .revisions
                    .iter()
                    .any(|node| graph.contains_key(node))
            {
                return Err(PropagationError::InvalidGraph);
            }
            if alt.direct.groups.iter().any(invalid_group_selector) {
                return Err(PropagationError::Differential(
                    DifferentialError::InvalidSelector,
                ));
            }
            union.revisions.extend(&alt.direct.revisions);
            union.relations.extend(&alt.direct.relations);
            union.groups.extend(alt.direct.groups.iter().cloned());
        }
    }
    let differential = compare(from, expected_from, to, expected_to, &union)
        .map_err(PropagationError::Differential)?;
    if graph
        .keys()
        .any(|id| !matches!(differential.revisions[id].from, RevisionState::Present(_)))
    {
        return Err(PropagationError::MissingHistoricalOutput);
    }
    let mut work = 0usize;
    let direct: BTreeMap<_, Vec<_>> = graph
        .iter()
        .map(|(id, alts)| {
            Ok((
                *id,
                alts.iter()
                    .map(|a| direct_state(a, &differential, &mut work))
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        })
        .collect::<Result<_, PropagationError>>()?;
    let mut states: BTreeMap<_, _> = graph
        .keys()
        .map(|id| (*id, Applicability::Unknown))
        .collect();
    loop {
        let mut changed = false;
        for (id, alts) in graph {
            let mut evaluated = Vec::with_capacity(alts.len());
            for (alt, base) in alts.iter().zip(&direct[id]) {
                evaluated.push(evaluate_derived(*base, alt, &states, &mut work)?);
            }
            let next = alternatives_state(&evaluated);
            if states[id] != next {
                states.insert(*id, next);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut nodes = BTreeMap::new();
    for (id, alts) in graph {
        let mut results = Vec::with_capacity(alts.len());
        for (alt, base) in alts.iter().zip(&direct[id]) {
            results.push(evaluate_derived(*base, alt, &states, &mut work)?);
        }
        nodes.insert(
            *id,
            PropagatedNode {
                applicability: states[id],
                alternatives: results,
            },
        );
    }
    Ok(Propagation {
        differential,
        nodes,
        work,
    })
}
