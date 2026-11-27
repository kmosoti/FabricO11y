//! In-memory, runtime-neutral contribution and causal proposal state.
//! Caller-supplied identities are opaque claims, not authentication or receipts.

use std::collections::{BTreeMap, BTreeSet};

pub const MAX_CONTRIBUTIONS: usize = 4096;
pub const MAX_HEADS: usize = 256;
pub const MAX_PARENTS: usize = 64;
pub const MAX_KIND_COMPONENT_BYTES: usize = 128;

macro_rules! identity {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub [u8; 32]);
    };
}
identity!(ContributionId);
identity!(ActorId);
identity!(PrincipalId);
identity!(TeamId);

/// Custom kinds carry no protocol behavior, including when named "proposal".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Discussion,
    Proposal,
    Custom { namespace: String, name: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contribution {
    pub id: ContributionId,
    pub actor: ActorId,
    pub principal: PrincipalId,
    pub team: Option<TeamId>,
    pub kind: Kind,
    /// Direct causal predecessors. All must already be present.
    pub parents: BTreeSet<ContributionId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidLimits,
    InvalidKind,
    TooManyParents,
    TooManyContributions,
    TooManyHeads,
    DuplicateId(ContributionId),
    MissingParent(ContributionId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Recorded {
    contribution: Contribution,
    turn: usize,
}

/// Bounded projection of a causally ordered contribution stream.
/// A proposal supersedes only its direct proposal parents. Other contributions
/// never alter heads. Siblings of an already superseded proposal remain heads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Protocol {
    max_contributions: usize,
    max_heads: usize,
    records: BTreeMap<ContributionId, Recorded>,
    heads: BTreeSet<ContributionId>,
}

impl Protocol {
    pub fn new(max_contributions: usize, max_heads: usize) -> Result<Self, Error> {
        if max_contributions == 0
            || max_contributions > MAX_CONTRIBUTIONS
            || max_heads == 0
            || max_heads > MAX_HEADS
            || max_heads > max_contributions
        {
            return Err(Error::InvalidLimits);
        }
        Ok(Self {
            max_contributions,
            max_heads,
            records: BTreeMap::new(),
            heads: BTreeSet::new(),
        })
    }

    /// Rejects invalid input without changing the projection. Admission order
    /// must respect causality; concurrent entries may arrive in either order.
    pub fn append(&mut self, contribution: Contribution) -> Result<(), Error> {
        if self.records.contains_key(&contribution.id) {
            return Err(Error::DuplicateId(contribution.id));
        }
        if self.records.len() >= self.max_contributions {
            return Err(Error::TooManyContributions);
        }
        if contribution.parents.len() > MAX_PARENTS {
            return Err(Error::TooManyParents);
        }
        if let Kind::Custom { namespace, name } = &contribution.kind
            && (namespace.is_empty()
                || name.is_empty()
                || namespace.len() > MAX_KIND_COMPONENT_BYTES
                || name.len() > MAX_KIND_COMPONENT_BYTES)
        {
            return Err(Error::InvalidKind);
        }
        let mut turn = 0;
        for parent in &contribution.parents {
            let prior = self
                .records
                .get(parent)
                .ok_or(Error::MissingParent(*parent))?;
            turn = turn.max(prior.turn + 1);
        }
        if contribution.kind == Kind::Proposal {
            let removed = contribution
                .parents
                .iter()
                .filter(|id| self.heads.contains(*id))
                .count();
            if self.heads.len() - removed + 1 > self.max_heads {
                return Err(Error::TooManyHeads);
            }
            for parent in &contribution.parents {
                self.heads.remove(parent);
            }
            self.heads.insert(contribution.id);
        }
        self.records
            .insert(contribution.id, Recorded { contribution, turn });
        Ok(())
    }

    pub fn proposal_heads(&self) -> &BTreeSet<ContributionId> {
        &self.heads
    }
    pub fn turn(&self, id: &ContributionId) -> Option<usize> {
        self.records.get(id).map(|r| r.turn)
    }
    pub fn contribution(&self, id: &ContributionId) -> Option<&Contribution> {
        self.records.get(id).map(|r| &r.contribution)
    }

    fn extends_history(&self, earlier: &Self) -> bool {
        self.max_contributions == earlier.max_contributions
            && self.max_heads == earlier.max_heads
            && earlier
                .records
                .iter()
                .all(|(id, record)| self.records.get(id) == Some(record))
    }
}

/// Hard bound on total contribution IDs retained across review boundaries.
pub const MAX_REVIEW_INPUTS: usize = MAX_CONTRIBUTIONS;
/// Hard bound on explicit reopen operations for one review session.
pub const MAX_REVIEW_REOPENINGS: usize = 16;

/// Opt-in limits for an explicitly scoped review session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReviewProfile {
    max_total_inputs: usize,
    max_reopenings: usize,
}

impl ReviewProfile {
    pub fn new(max_total_inputs: usize, max_reopenings: usize) -> Result<Self, ReviewError> {
        if max_total_inputs == 0
            || max_total_inputs > MAX_REVIEW_INPUTS
            || max_reopenings > MAX_REVIEW_REOPENINGS
        {
            return Err(ReviewError::InvalidLimits);
        }
        Ok(Self {
            max_total_inputs,
            max_reopenings,
        })
    }

    pub fn max_total_inputs(&self) -> usize {
        self.max_total_inputs
    }

    pub fn max_reopenings(&self) -> usize {
        self.max_reopenings
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewUnresolvedReason {
    InputBudget,
    ReopenBudget,
    HistoryMismatch,
}

/// Exhaustion or a changed history remains unresolved. This has no decision state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewStatus {
    Open,
    Sealed,
    Unresolved(ReviewUnresolvedReason),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewError {
    InvalidLimits,
    AlreadyStarted,
    NotSealed,
    MissingContribution(ContributionId),
    HistoryMismatch,
    InputBudgetExceeded,
    ReopenBudgetExceeded,
}

/// Immutable snapshot of the exact admitted contribution IDs selected for one boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewBoundary {
    sequence: usize,
    inputs: BTreeSet<ContributionId>,
}

impl ReviewBoundary {
    pub fn sequence(&self) -> usize {
        self.sequence
    }

    pub fn inputs(&self) -> &BTreeSet<ContributionId> {
        &self.inputs
    }
}

/// Opt-in bounded review history. It does not accept proposals or grant authority.
/// Each boundary snapshots caller-selected admitted IDs; later appends cannot change it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewSession {
    profile: ReviewProfile,
    status: ReviewStatus,
    boundaries: Vec<ReviewBoundary>,
    history: Option<Protocol>,
    total_inputs: usize,
    reopenings: usize,
}

impl ReviewSession {
    pub fn new(profile: ReviewProfile) -> Self {
        Self {
            profile,
            status: ReviewStatus::Open,
            boundaries: Vec::new(),
            history: None,
            total_inputs: 0,
            reopenings: 0,
        }
    }

    /// Seal one explicit input set. Free-form contributions remain unaffected.
    pub fn seal(
        &mut self,
        protocol: &Protocol,
        inputs: &BTreeSet<ContributionId>,
    ) -> Result<(), ReviewError> {
        if self.status != ReviewStatus::Open {
            return Err(ReviewError::AlreadyStarted);
        }
        self.capture(protocol, inputs, false)
    }

    /// Explicitly open a new boundary while retaining every prior boundary.
    pub fn reopen(
        &mut self,
        protocol: &Protocol,
        inputs: &BTreeSet<ContributionId>,
    ) -> Result<(), ReviewError> {
        if self.status != ReviewStatus::Sealed {
            return Err(ReviewError::NotSealed);
        }
        if self.reopenings >= self.profile.max_reopenings {
            self.status = ReviewStatus::Unresolved(ReviewUnresolvedReason::ReopenBudget);
            return Err(ReviewError::ReopenBudgetExceeded);
        }
        self.capture(protocol, inputs, true)
    }

    fn capture(
        &mut self,
        protocol: &Protocol,
        inputs: &BTreeSet<ContributionId>,
        reopening: bool,
    ) -> Result<(), ReviewError> {
        if inputs.len() > self.profile.max_total_inputs - self.total_inputs {
            self.status = ReviewStatus::Unresolved(ReviewUnresolvedReason::InputBudget);
            return Err(ReviewError::InputBudgetExceeded);
        }
        if self
            .history
            .as_ref()
            .is_some_and(|earlier| !protocol.extends_history(earlier))
        {
            self.status = ReviewStatus::Unresolved(ReviewUnresolvedReason::HistoryMismatch);
            return Err(ReviewError::HistoryMismatch);
        }
        for id in inputs {
            if protocol.contribution(id).is_none() {
                return Err(ReviewError::MissingContribution(*id));
            }
        }
        self.boundaries.push(ReviewBoundary {
            sequence: self.boundaries.len(),
            inputs: inputs.clone(),
        });
        self.total_inputs += inputs.len();
        self.history = Some(protocol.clone());
        if reopening {
            self.reopenings += 1;
        }
        self.status = ReviewStatus::Sealed;
        Ok(())
    }

    pub fn status(&self) -> ReviewStatus {
        self.status
    }

    pub fn boundaries(&self) -> &[ReviewBoundary] {
        &self.boundaries
    }

    pub fn current_boundary(&self) -> Option<&ReviewBoundary> {
        self.boundaries.last()
    }

    pub fn total_inputs(&self) -> usize {
        self.total_inputs
    }

    pub fn reopenings(&self) -> usize {
        self.reopenings
    }
}
