//! Pure one-work-item claim fencing. No authentication, clock, durable commit,
//! accepted-plan validation, or external side-effect enforcement is provided.
use crate::{ActorId, ContributionId};

/// An opaque caller-provided claim identifier, not a credential.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClaimToken(pub [u8; 32]);

/// The caller must independently establish that `work` names accepted work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Claim {
    pub work: ContributionId,
    pub owner: ActorId,
    pub token: ClaimToken,
    pub fence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimError {
    VersionMismatch,
    Stale,
    TokenReuse,
    Exhausted,
    Finalized,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Finalize {
    Applied,
    AlreadyFinalized,
}

/// Bounded state for one work item. Generations never wrap.
#[derive(Clone, Debug)]
pub struct WorkClaim {
    work: ContributionId,
    generation: u64,
    current: Option<Claim>,
    finalized: bool,
    last_revoked: Option<Claim>,
}
impl WorkClaim {
    pub fn new(work: ContributionId) -> Self {
        Self {
            work,
            generation: 0,
            current: None,
            finalized: false,
            last_revoked: None,
        }
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn current(&self) -> Option<Claim> {
        self.current
    }

    /// Acquire or replace using compare-and-set. Repeating the exact active
    /// acquisition with its original expected generation is idempotent.
    pub fn acquire(
        &mut self,
        expected: u64,
        owner: ActorId,
        token: ClaimToken,
    ) -> Result<Claim, ClaimError> {
        if self.finalized {
            return Err(ClaimError::Finalized);
        }
        if let Some(current) = self.current {
            if current.owner == owner
                && current.token == token
                && expected.checked_add(1) == Some(current.fence)
            {
                return Ok(current);
            }
            if current.token == token {
                return Err(ClaimError::TokenReuse);
            }
        }
        if expected != self.generation {
            return Err(ClaimError::VersionMismatch);
        }
        let fence = self
            .generation
            .checked_add(1)
            .ok_or(ClaimError::Exhausted)?;
        let claim = Claim {
            work: self.work,
            owner,
            token,
            fence,
        };
        self.generation = fence;
        self.current = Some(claim);
        self.last_revoked = None;
        Ok(claim)
    }

    /// Revoke only the current claim. The immediately repeated revoke is idempotent.
    pub fn revoke(&mut self, claim: Claim) -> Result<u64, ClaimError> {
        if self.finalized {
            return Err(ClaimError::Finalized);
        }
        if self.current.is_none() && self.last_revoked == Some(claim) {
            return Ok(self.generation);
        }
        if self.current != Some(claim) {
            return Err(ClaimError::Stale);
        }
        let next = self
            .generation
            .checked_add(1)
            .ok_or(ClaimError::Exhausted)?;
        self.generation = next;
        self.current = None;
        self.last_revoked = Some(claim);
        Ok(next)
    }

    /// Finalization is a model transition, not execution authorization.
    pub fn finalize(&mut self, claim: Claim) -> Result<Finalize, ClaimError> {
        if self.current != Some(claim) {
            return Err(ClaimError::Stale);
        }
        if self.finalized {
            return Ok(Finalize::AlreadyFinalized);
        }
        self.finalized = true;
        Ok(Finalize::Applied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Contribution, Kind, PrincipalId, Protocol};
    use std::collections::BTreeSet;
    fn actor(n: u8) -> ActorId {
        ActorId([n; 32])
    }
    fn token(n: u8) -> ClaimToken {
        ClaimToken([n; 32])
    }
    #[test]
    fn replaced_owner_cannot_finalize_but_late_evidence_can_append() {
        let work = ContributionId([1; 32]);
        let mut claims = WorkClaim::new(work);
        let a = claims.acquire(0, actor(1), token(1)).unwrap();
        assert_eq!(claims.acquire(0, actor(1), token(1)), Ok(a));
        let b = claims.acquire(1, actor(2), token(2)).unwrap();
        assert_eq!(claims.finalize(a), Err(ClaimError::Stale));
        let mut protocol = Protocol::new(4, 2).unwrap();
        protocol
            .append(Contribution {
                id: work,
                actor: actor(1),
                principal: PrincipalId([1; 32]),
                team: None,
                kind: Kind::Proposal,
                parents: BTreeSet::new(),
            })
            .unwrap();
        let late = ContributionId([2; 32]);
        protocol
            .append(Contribution {
                id: late,
                actor: actor(1),
                principal: PrincipalId([1; 32]),
                team: None,
                kind: Kind::Discussion,
                parents: BTreeSet::from([work]),
            })
            .unwrap();
        assert!(protocol.contribution(&late).is_some());
        assert_eq!(claims.finalize(b), Ok(Finalize::Applied));
        assert_eq!(claims.finalize(b), Ok(Finalize::AlreadyFinalized));
        assert_eq!(
            claims.acquire(2, actor(1), token(3)),
            Err(ClaimError::Finalized)
        );
    }
    #[test]
    fn compare_revoke_identity_and_exhaustion_fail_closed() {
        let work = ContributionId([3; 32]);
        let mut claims = WorkClaim::new(work);
        assert_eq!(
            claims.acquire(1, actor(1), token(1)),
            Err(ClaimError::VersionMismatch)
        );
        let a = claims.acquire(0, actor(1), token(1)).unwrap();
        assert_eq!(
            claims.acquire(1, actor(2), token(1)),
            Err(ClaimError::TokenReuse)
        );
        let mut wrong = a;
        wrong.work = ContributionId([4; 32]);
        assert_eq!(claims.finalize(wrong), Err(ClaimError::Stale));
        assert_eq!(claims.revoke(wrong), Err(ClaimError::Stale));
        let mut wrong_owner = a;
        wrong_owner.owner = actor(2);
        assert_eq!(claims.finalize(wrong_owner), Err(ClaimError::Stale));
        let mut wrong_fence = a;
        wrong_fence.fence += 1;
        assert_eq!(claims.finalize(wrong_fence), Err(ClaimError::Stale));
        assert_eq!(claims.revoke(a), Ok(2));
        assert_eq!(claims.revoke(a), Ok(2));
        let b = claims.acquire(2, actor(2), token(2)).unwrap();
        assert_eq!(claims.revoke(a), Err(ClaimError::Stale));
        assert_eq!(claims.finalize(b), Ok(Finalize::Applied));
        let mut exhausted = WorkClaim::new(work);
        exhausted.generation = u64::MAX;
        assert_eq!(
            exhausted.acquire(u64::MAX, actor(1), token(1)),
            Err(ClaimError::Exhausted)
        );
        assert_eq!(exhausted.generation(), u64::MAX);
        let mut no_revoke = WorkClaim::new(work);
        no_revoke.generation = u64::MAX - 1;
        let last = no_revoke.acquire(u64::MAX - 1, actor(1), token(1)).unwrap();
        assert_eq!(no_revoke.revoke(last), Err(ClaimError::Exhausted));
        assert_eq!(no_revoke.current(), Some(last));
    }
}
