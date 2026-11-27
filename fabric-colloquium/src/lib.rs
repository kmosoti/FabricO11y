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
}
