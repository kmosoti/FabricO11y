//! Bounded structural investigation over a verified portable selection.
//! No authority, independence, completeness, entailment or truth is inferred.
use crate::{
    RelationDigest, RevisionDigest, SourceClaim,
    as_of::{PremiseStatus, SupportGroup},
    portable::{AvailabilityClaim, PortableError, PortableView},
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const MAX_ANCHORS: usize = 64;
pub const MAX_NODES: usize = 256;
pub const MAX_GROUPS: usize = 128;
pub const MAX_EDGES: usize = 1024;
pub const MAX_WORK: usize = 131_072;
/// Maximum source claims inspected for explicitly selected terminal revisions.
pub const MAX_TERMINAL_SOURCE_CLAIMS: usize = 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub nodes: usize,
    pub groups: usize,
    pub edges: usize,
    pub work: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            nodes: MAX_NODES,
            groups: MAX_GROUPS,
            edges: MAX_EDGES,
            work: MAX_WORK,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Omitted,
    Missing,
    Present,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Premise {
    pub revision: RevisionDigest,
    pub relation: RelationDigest,
    pub revision_selection: Selection,
    pub relation_selection: Selection,
    pub status: PremiseStatus,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub attributed_to: String,
    pub premises: Vec<Premise>,
    /// All selected premises were enumerated; this does not mean resolved.
    pub enumeration_complete: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub selection: Selection,
    pub groups: Vec<Group>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frontier {
    pub basis_digest: [u8; 32],
    pub anchors: BTreeSet<RevisionDigest>,
    pub nodes: BTreeMap<RevisionDigest, Node>,
    /// Structural terminals with no selected support group, keyed by terminal.
    pub terminal_anchors: BTreeMap<RevisionDigest, BTreeSet<RevisionDigest>>,
    /// DFS back-edges in the selected support graph; not a list of all cycles.
    pub cycle_edges: BTreeSet<(RevisionDigest, RevisionDigest)>,
    pub work: usize,
    /// Any cap reached means reported nodes, roots and cycles may be incomplete.
    pub truncated: bool,
}

/// A source claim is identified by its revision and index, not independently
/// authenticated by this projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceReference {
    pub revision: RevisionDigest,
    pub source_index: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Captured([u8; 32]),
    ExternalOnly,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeclaredSource {
    pub reference: SourceReference,
    pub kind: SourceKind,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalState {
    ObservedTerminal,
    Missing,
    Omitted,
    /// Includes nonterminals and nodes not visited because traversal was capped.
    NotObservedTerminal,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSources {
    pub state: TerminalState,
    pub availability: Option<AvailabilityClaim>,
    pub claims: Vec<DeclaredSource>,
}
/// Grouping is equality of declared digests, not independent corroboration,
/// verified source bytes, custody, or present availability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFrontier {
    pub frontier: Frontier,
    pub terminals: BTreeMap<RevisionDigest, TerminalSources>,
    pub captured: BTreeMap<[u8; 32], Vec<SourceReference>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Limits,
    Portable(PortableError),
}
struct Budget {
    used: usize,
    max: usize,
    truncated: bool,
}
impl Budget {
    fn tick(&mut self) -> bool {
        if self.used >= self.max {
            self.truncated = true;
            false
        } else {
            self.used += 1;
            true
        }
    }
}
fn selection<T>(value: Option<&Option<T>>) -> Selection {
    match value {
        None => Selection::Omitted,
        Some(None) => Selection::Missing,
        Some(Some(_)) => Selection::Present,
    }
}
/// Imports and digest-checks the supplied portable bytes before traversal.
/// Caller must bound decoding before constructing the anchor set.
pub fn project(
    bytes: &[u8],
    digest: [u8; 32],
    anchors: BTreeSet<RevisionDigest>,
    limits: Limits,
) -> Result<Frontier, Error> {
    if anchors.len() > MAX_ANCHORS
        || limits.nodes > MAX_NODES
        || limits.groups > MAX_GROUPS
        || limits.edges > MAX_EDGES
        || limits.work > MAX_WORK
    {
        return Err(Error::Limits);
    }
    let portable = PortableView::import(bytes, digest).map_err(Error::Portable)?;
    project_imported(&portable, digest, anchors, limits)
}

/// Verify one portable selection and group the claims of explicitly selected
/// observed terminals. Missing and unvisited selections remain explicit.
pub fn project_with_sources(
    bytes: &[u8],
    digest: [u8; 32],
    anchors: BTreeSet<RevisionDigest>,
    selected_terminals: BTreeSet<RevisionDigest>,
    limits: Limits,
) -> Result<SourceFrontier, Error> {
    if anchors.len() > MAX_ANCHORS
        || selected_terminals.len() > MAX_NODES
        || limits.nodes > MAX_NODES
        || limits.groups > MAX_GROUPS
        || limits.edges > MAX_EDGES
        || limits.work > MAX_WORK
    {
        return Err(Error::Limits);
    }
    let portable = PortableView::import(bytes, digest).map_err(Error::Portable)?;
    let frontier = project_imported(&portable, digest, anchors, limits)?;
    let view = portable.view();
    let mut total = 0usize;
    for id in &selected_terminals {
        if frontier.terminal_anchors.contains_key(id)
            && let Some(Some(revision)) = view.revision(id)
        {
            total = total
                .checked_add(revision.sources.len())
                .ok_or(Error::Limits)?;
            if total > MAX_TERMINAL_SOURCE_CLAIMS {
                return Err(Error::Limits);
            }
        }
    }
    let mut terminals = BTreeMap::new();
    let mut captured: BTreeMap<[u8; 32], Vec<SourceReference>> = BTreeMap::new();
    for id in selected_terminals {
        let revision = view.revision(&id);
        let state = match revision {
            None => TerminalState::Omitted,
            Some(None) => TerminalState::Missing,
            Some(Some(_)) if frontier.terminal_anchors.contains_key(&id) => {
                TerminalState::ObservedTerminal
            }
            Some(Some(_)) => TerminalState::NotObservedTerminal,
        };
        let mut claims = Vec::new();
        if state == TerminalState::ObservedTerminal
            && let Some(Some(revision)) = revision
        {
            for (source_index, source) in revision.sources.iter().enumerate() {
                let reference = SourceReference {
                    revision: id,
                    source_index,
                };
                let kind = match source {
                    SourceClaim::Captured {
                        artifact_sha256, ..
                    } => {
                        captured
                            .entry(*artifact_sha256)
                            .or_default()
                            .push(reference);
                        SourceKind::Captured(*artifact_sha256)
                    }
                    SourceClaim::ExternalOnly { .. } => SourceKind::ExternalOnly,
                };
                claims.push(DeclaredSource { reference, kind });
            }
        }
        terminals.insert(
            id,
            TerminalSources {
                state,
                availability: portable.availability().get(&id).copied(),
                claims,
            },
        );
    }
    Ok(SourceFrontier {
        frontier,
        terminals,
        captured,
    })
}

fn project_imported(
    portable: &PortableView,
    digest: [u8; 32],
    anchors: BTreeSet<RevisionDigest>,
    limits: Limits,
) -> Result<Frontier, Error> {
    let view = portable.view();
    let mut by_conclusion: BTreeMap<RevisionDigest, Vec<&SupportGroup>> = BTreeMap::new();
    for group in view.groups().keys() {
        by_conclusion
            .entry(group.conclusion)
            .or_default()
            .push(group);
    }
    let mut budget = Budget {
        used: 0,
        max: limits.work,
        truncated: false,
    };
    let mut nodes = BTreeMap::new();
    let mut queue: BTreeSet<RevisionDigest> = anchors.clone();
    let mut groups_used = 0usize;
    let mut edges_used = 0usize;
    while let Some(id) = queue.pop_first() {
        if nodes.contains_key(&id) {
            continue;
        }
        if nodes.len() >= limits.nodes || !budget.tick() {
            budget.truncated = true;
            break;
        }
        let node_selection = selection(view.revision(&id));
        let mut groups = Vec::new();
        if node_selection == Selection::Present
            && let Some(selected_groups) = by_conclusion.get(&id)
        {
            for group in selected_groups {
                if groups_used >= limits.groups || !budget.tick() {
                    budget.truncated = true;
                    break;
                }
                groups_used += 1;
                let mut entry = Group {
                    attributed_to: group.attributed_to.clone(),
                    premises: Vec::new(),
                    enumeration_complete: true,
                };
                let projection = &view.groups()[*group];
                for (premise, relation) in &group.premises {
                    if edges_used >= limits.edges || !budget.tick() {
                        budget.truncated = true;
                        entry.enumeration_complete = false;
                        break;
                    }
                    edges_used += 1;
                    let status = projection.premises()[premise];
                    entry.premises.push(Premise {
                        revision: *premise,
                        relation: *relation,
                        revision_selection: selection(view.revision(premise)),
                        relation_selection: selection(view.relation(relation)),
                        status,
                    });
                    if status == PremiseStatus::Present && !nodes.contains_key(premise) {
                        queue.insert(*premise);
                    }
                }
                groups.push(entry);
                if budget.truncated {
                    break;
                }
            }
        }
        nodes.insert(
            id,
            Node {
                selection: node_selection,
                groups,
            },
        );
        if budget.truncated {
            break;
        }
    }
    // Propagation is over visited graph pairs, not derivation paths. A cap
    // explicitly makes the resulting root-to-anchor mapping incomplete.
    let mut terminal_anchors: BTreeMap<RevisionDigest, BTreeSet<RevisionDigest>> = BTreeMap::new();
    let mut visited = BTreeSet::new();
    let mut pairs = VecDeque::new();
    for anchor in &anchors {
        pairs.push_back((*anchor, *anchor));
    }
    while let Some((anchor, id)) = pairs.pop_front() {
        if visited.contains(&(anchor, id)) {
            continue;
        }
        if !budget.tick() {
            break;
        }
        visited.insert((anchor, id));
        let Some(node) = nodes.get(&id) else {
            continue;
        };
        if node.selection == Selection::Present && !by_conclusion.contains_key(&id) {
            terminal_anchors.entry(id).or_default().insert(anchor);
        }
        for group in &node.groups {
            for premise in &group.premises {
                if !budget.tick() {
                    break;
                }
                if premise.status == PremiseStatus::Present
                    && !visited.contains(&(anchor, premise.revision))
                {
                    pairs.push_back((anchor, premise.revision));
                }
            }
            if budget.truncated {
                break;
            }
        }
        if budget.truncated {
            break;
        }
    }
    // A bounded iterative DFS reports back-edges, including self-edges.
    let mut color = BTreeMap::<RevisionDigest, u8>::new();
    let mut cycle_edges = BTreeSet::new();
    for start in nodes.keys().copied() {
        if color.contains_key(&start) {
            continue;
        }
        let mut stack = vec![(start, false)];
        while let Some((id, exiting)) = stack.pop() {
            if !budget.tick() {
                break;
            }
            if exiting {
                color.insert(id, 2);
                continue;
            }
            if color.contains_key(&id) {
                continue;
            }
            color.insert(id, 1);
            stack.push((id, true));
            if let Some(node) = nodes.get(&id) {
                for premise in node.groups.iter().flat_map(|g| &g.premises).rev() {
                    if !budget.tick() {
                        break;
                    }
                    if premise.status != PremiseStatus::Present
                        || !nodes.contains_key(&premise.revision)
                    {
                        continue;
                    }
                    match color.get(&premise.revision) {
                        Some(1) => {
                            cycle_edges.insert((id, premise.revision));
                        }
                        Some(_) => {}
                        None => stack.push((premise.revision, false)),
                    }
                }
            }
        }
        if budget.truncated {
            break;
        }
    }
    Ok(Frontier {
        basis_digest: digest,
        anchors,
        nodes,
        terminal_anchors,
        cycle_edges,
        work: budget.used,
        truncated: budget.truncated,
    })
}
