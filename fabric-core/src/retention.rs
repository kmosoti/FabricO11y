//! Pure pressure decisions. Inputs are snapshots supplied by an adapter; a plan
//! neither reserves space nor verifies archive evidence or performs I/O.

/// Maximum number of candidates accepted in one snapshot.
pub const MAX_CANDIDATES: usize = 256;

/// Fidelity is independent of whether an object is protected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fidelity {
    /// Exact source evidence.
    Exact,
    /// A separately identified derivative of an exact parent.
    Derived { parent: [u8; 32] },
}

/// An adapter-verified archive and restore assertion, bound to exact bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifiedArchive {
    /// Identity of the archived candidate.
    pub id: u64,
    /// Digest of the exact bytes independently verified on restore.
    pub digest: [u8; 32],
}

/// One immutable candidate. `priority` is supplied by policy; lower sorts first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub id: u64,
    pub digest: [u8; 32],
    pub fidelity: Fidelity,
    pub protected: bool,
    /// Active ledger closure is never selected, including with archive evidence.
    pub active_dependency: bool,
    pub bytes: u64,
    pub priority: u16,
    /// Space needed for a lossless rewrite, if a rewrite is useful.
    pub rewrite_scratch: Option<u64>,
    pub archive: Option<VerifiedArchive>,
}

/// Bytes and watermarks must describe the same storage snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Budget {
    pub used: u64,
    pub free: u64,
    pub quota: u64,
    pub low: u64,
    pub high: u64,
    /// Whether the preceding snapshot was already under pressure.
    pub pressure: bool,
    /// Space reserved for a structured eviction audit record.
    pub audit: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Invalid {
    Budget,
    Overflow,
    TooManyCandidates,
    DuplicateId,
    InvalidCandidate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    /// The applicable watermark does not require maintenance.
    Idle,
    /// Reserve both quantities before beginning a lossless rewrite.
    Rewrite { id: u64, scratch: u64, audit: u64 },
    /// Reserve audit space before beginning an authorized eviction.
    Evict { id: u64, audit: u64 },
    /// No safe action is currently possible; admission must be held.
    Backpressure,
}

/// Select one action without changing the input. The executor must revalidate
/// the snapshot and acquire reservations before acting. A caller must only pass
/// archive assertions whose restore evidence it has independently verified.
pub fn plan(b: Budget, candidates: &[Candidate]) -> Result<Decision, Invalid> {
    if b.quota == 0 || b.low > b.high || b.high > b.quota {
        return Err(Invalid::Budget);
    }
    b.used.checked_add(b.free).ok_or(Invalid::Overflow)?;
    if candidates.len() > MAX_CANDIDATES {
        return Err(Invalid::TooManyCandidates);
    }
    for (i, c) in candidates.iter().enumerate() {
        if c.bytes == 0 || matches!(c.fidelity, Fidelity::Derived { parent } if parent == c.digest)
        {
            return Err(Invalid::InvalidCandidate);
        }
        if candidates[..i].iter().any(|other| other.id == c.id) {
            return Err(Invalid::DuplicateId);
        }
        if let Some(scratch) = c.rewrite_scratch {
            scratch.checked_add(b.audit).ok_or(Invalid::Overflow)?;
        }
    }
    if b.used <= if b.pressure { b.low } else { b.high } {
        return Ok(Decision::Idle);
    }
    let eligible = |c: &&Candidate| !c.active_dependency;
    if let Some(c) = candidates
        .iter()
        .filter(eligible)
        .filter(|c| {
            !c.protected
                && c.rewrite_scratch
                    .is_some_and(|scratch| scratch + b.audit <= b.free)
        })
        .min_by_key(|c| (c.priority, c.id))
    {
        return Ok(Decision::Rewrite {
            id: c.id,
            scratch: c.rewrite_scratch.unwrap(),
            audit: b.audit,
        });
    }
    if b.audit <= b.free
        && let Some(c) = candidates
            .iter()
            .filter(eligible)
            .filter(|c| {
                !c.protected
                    || c.archive
                        == Some(VerifiedArchive {
                            id: c.id,
                            digest: c.digest,
                        })
            })
            .min_by_key(|c| (c.priority, c.id))
    {
        return Ok(Decision::Evict {
            id: c.id,
            audit: b.audit,
        });
    }
    Ok(Decision::Backpressure)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> Budget {
        Budget {
            used: 91,
            free: 30,
            quota: 100,
            low: 60,
            high: 90,
            pressure: false,
            audit: 5,
        }
    }
    fn candidate(id: u64) -> Candidate {
        Candidate {
            id,
            digest: [id as u8; 32],
            fidelity: Fidelity::Exact,
            protected: true,
            active_dependency: false,
            bytes: 10,
            priority: 1,
            rewrite_scratch: None,
            archive: None,
        }
    }
    #[test]
    fn watermark_and_hysteresis() {
        let c = candidate(1);
        let mut b = budget();
        b.used = 90;
        assert_eq!(plan(b, &[c]), Ok(Decision::Idle));
        b.pressure = true;
        assert_eq!(plan(b, &[c]), Ok(Decision::Backpressure));
        b.used = 60;
        assert_eq!(plan(b, &[c]), Ok(Decision::Idle));
    }
    #[test]
    fn reservation_archive_and_active_closure() {
        let mut c = candidate(1);
        let mut b = budget();
        b.free = 4;
        c.protected = false;
        c.rewrite_scratch = Some(26);
        assert_eq!(plan(b, &[c]), Ok(Decision::Backpressure));
        b.free = 30;
        c.rewrite_scratch = Some(25);
        assert_eq!(
            plan(b, &[c]),
            Ok(Decision::Rewrite {
                id: 1,
                scratch: 25,
                audit: 5
            })
        );
        c.rewrite_scratch = None;
        c.protected = true;
        c.archive = Some(VerifiedArchive {
            id: 2,
            digest: c.digest,
        });
        assert_eq!(plan(b, &[c]), Ok(Decision::Backpressure));
        c.archive = Some(VerifiedArchive {
            id: 1,
            digest: [2; 32],
        });
        assert_eq!(plan(b, &[c]), Ok(Decision::Backpressure));
        c.archive = Some(VerifiedArchive {
            id: 1,
            digest: c.digest,
        });
        assert_eq!(plan(b, &[c]), Ok(Decision::Evict { id: 1, audit: 5 }));
        c.active_dependency = true;
        assert_eq!(plan(b, &[c]), Ok(Decision::Backpressure));
        c.active_dependency = false;
        let mut no_audit = b;
        no_audit.free = 4;
        assert_eq!(plan(no_audit, &[c]), Ok(Decision::Backpressure));
    }
    #[test]
    fn validates_and_is_deterministic_without_mutation() {
        let mut a = candidate(1);
        a.protected = false;
        let mut z = candidate(2);
        z.protected = false;
        let input = [z, a];
        assert_eq!(
            plan(budget(), &input),
            Ok(Decision::Evict { id: 1, audit: 5 })
        );
        assert_eq!(plan(budget(), &[a, z]), plan(budget(), &input));
        assert_eq!(input, [z, a]);
        assert_eq!(plan(budget(), &[a, a]), Err(Invalid::DuplicateId));
        assert_eq!(
            plan(budget(), &vec![a; MAX_CANDIDATES + 1]),
            Err(Invalid::TooManyCandidates)
        );
        let mut b = budget();
        b.free = u64::MAX;
        assert_eq!(plan(b, &[]), Err(Invalid::Overflow));
        b = budget();
        a.rewrite_scratch = Some(u64::MAX);
        assert_eq!(plan(b, &[a]), Err(Invalid::Overflow));
        a.rewrite_scratch = None;
        a.fidelity = Fidelity::Derived { parent: a.digest };
        assert_eq!(plan(b, &[a]), Err(Invalid::InvalidCandidate));
    }
}
