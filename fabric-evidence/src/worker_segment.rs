//! Local v1 worker segment claims; independent of transport and observation wire.
//! Hashes check bytes, not sender identity, custody, policy or actual capture.
use crate::evidence_manifest::ArtifactContentDigest;
use sha2::{Digest, Sha256};

pub const MAX_SEGMENT_BYTES: usize = 1024 * 1024;
pub const MAX_RECORDS: usize = 1024;
pub const MAX_RECORD_BYTES: usize = 32 * 1024;
const SEGMENT_MAGIC: &[u8; 4] = b"FWS1";
const TERMINAL_MAGIC: &[u8; 4] = b"FWT1";
const TERMINAL_BYTES: usize = 4 + 16 + 16 + 8 + 32 + 8 + 1;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope {
    pub job: [u8; 16],
    pub epoch: [u8; 16],
}
#[derive(Clone, Copy, Debug)]
pub struct Segment<'a> {
    pub scope: Scope,
    pub sequence: u64,
    pub previous: [u8; 32],
    pub dropped: u64,
    pub records: &'a [&'a [u8]],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Terminal {
    pub scope: Scope,
    pub final_sequence: u64,
    pub final_digest: [u8; 32],
    pub dropped: u64,
    pub declared_complete: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Limit,
    Digest,
    Malformed,
    Scope,
    Sequence,
    Chain,
    Conflict,
    Sealed,
    TerminalMismatch,
    Overflow,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Inserted,
    Duplicate,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completeness {
    MissingTerminal,
    ProducerIncomplete,
    RejectedInput,
    ObservedDrops { count: u64 },
    Complete,
}
/// Immutable append-only verifier state; previous non-head replays are rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verifier {
    scope: Scope,
    sequence: u64,
    digest: [u8; 32],
    records: u64,
    dropped: u64,
    rejected_input: bool,
    terminal: Option<(Terminal, [u8; 32])>,
}
fn put_u64(out: &mut Vec<u8>, n: u64) {
    out.extend_from_slice(&n.to_be_bytes());
}
fn put_u32(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&(n as u32).to_be_bytes());
}
/// Encodes the closed local profile; callers bound their own capture and staging.
pub fn encode_segment(segment: Segment<'_>) -> Result<Vec<u8>, Error> {
    if segment.sequence == 0 || segment.records.len() > MAX_RECORDS {
        return Err(Error::Limit);
    }
    let mut len = 4usize + 16 + 16 + 8 + 32 + 8 + 4;
    for record in segment.records {
        if record.len() > MAX_RECORD_BYTES {
            return Err(Error::Limit);
        }
        len = len
            .checked_add(4)
            .and_then(|v| v.checked_add(record.len()))
            .ok_or(Error::Limit)?;
        if len > MAX_SEGMENT_BYTES {
            return Err(Error::Limit);
        }
    }
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(SEGMENT_MAGIC);
    out.extend_from_slice(&segment.scope.job);
    out.extend_from_slice(&segment.scope.epoch);
    put_u64(&mut out, segment.sequence);
    out.extend_from_slice(&segment.previous);
    put_u64(&mut out, segment.dropped);
    put_u32(&mut out, segment.records.len());
    for record in segment.records {
        put_u32(&mut out, record.len());
        out.extend_from_slice(record);
    }
    Ok(out)
}
pub fn encode_terminal(terminal: Terminal) -> Vec<u8> {
    let mut out = Vec::with_capacity(TERMINAL_BYTES);
    out.extend_from_slice(TERMINAL_MAGIC);
    out.extend_from_slice(&terminal.scope.job);
    out.extend_from_slice(&terminal.scope.epoch);
    put_u64(&mut out, terminal.final_sequence);
    out.extend_from_slice(&terminal.final_digest);
    put_u64(&mut out, terminal.dropped);
    out.push(u8::from(terminal.declared_complete));
    out
}
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(n).ok_or(Error::Malformed)?;
        let bytes = self.bytes.get(self.at..end).ok_or(Error::Malformed)?;
        self.at = end;
        Ok(bytes)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        self.take(N)?.try_into().map_err(|_| Error::Malformed)
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_be_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_be_bytes(self.array()?))
    }
    fn scope(&mut self) -> Result<Scope, Error> {
        Ok(Scope {
            job: self.array()?,
            epoch: self.array()?,
        })
    }
    fn done(&self) -> bool {
        self.at == self.bytes.len()
    }
}
fn checked_digest(bytes: &[u8], claim: &ArtifactContentDigest) -> Result<[u8; 32], Error> {
    claim.verify_exact_bytes(bytes).map_err(|_| Error::Digest)?;
    Ok(Sha256::digest(bytes).into())
}
impl Verifier {
    pub fn new(scope: Scope) -> Self {
        Self {
            scope,
            sequence: 0,
            digest: [0; 32],
            records: 0,
            dropped: 0,
            rejected_input: false,
            terminal: None,
        }
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }
    pub fn record_count(&self) -> u64 {
        self.records
    }
    pub fn completeness(&self) -> Completeness {
        match self.terminal {
            None => Completeness::MissingTerminal,
            Some((terminal, _)) if !terminal.declared_complete => Completeness::ProducerIncomplete,
            Some(_) if self.dropped != 0 => Completeness::ObservedDrops {
                count: self.dropped,
            },
            Some(_) if self.rejected_input => Completeness::RejectedInput,
            Some(_) => Completeness::Complete,
        }
    }
    /// Rejected unscoped/corrupt input is conservatively recorded as uncertainty;
    /// foreign-scope inputs do not change this job. The chain itself is unchanged.
    pub fn append(
        &mut self,
        bytes: &[u8],
        claim: &ArtifactContentDigest,
    ) -> Result<Admission, Error> {
        let result = self.append_inner(bytes, claim);
        if result.is_err() && !matches!(result, Err(Error::Scope)) {
            self.rejected_input = true;
        }
        result
    }
    fn append_inner(
        &mut self,
        bytes: &[u8],
        claim: &ArtifactContentDigest,
    ) -> Result<Admission, Error> {
        if bytes.len() > MAX_SEGMENT_BYTES {
            return Err(Error::Limit);
        }
        let digest = checked_digest(bytes, claim)?;
        let mut r = Reader { bytes, at: 0 };
        if r.take(4)? != SEGMENT_MAGIC {
            return Err(Error::Malformed);
        }
        let scope = r.scope()?;
        let sequence = r.u64()?;
        let previous = r.array::<32>()?;
        let dropped = r.u64()?;
        let count = r.u32()? as usize;
        if count > MAX_RECORDS {
            return Err(Error::Limit);
        }
        for _ in 0..count {
            let n = r.u32()? as usize;
            if n > MAX_RECORD_BYTES {
                return Err(Error::Limit);
            }
            r.take(n)?;
        }
        if !r.done() || sequence == 0 {
            return Err(Error::Malformed);
        }
        if scope != self.scope {
            return Err(Error::Scope);
        }
        if sequence == self.sequence {
            return if digest == self.digest {
                Ok(Admission::Duplicate)
            } else {
                Err(Error::Conflict)
            };
        }
        if self.terminal.is_some() {
            return Err(Error::Sealed);
        }
        if self.sequence.checked_add(1) != Some(sequence) {
            return Err(Error::Sequence);
        }
        if previous != self.digest {
            return Err(Error::Chain);
        }
        let next_records = self
            .records
            .checked_add(count as u64)
            .ok_or(Error::Overflow)?;
        let next_dropped = self.dropped.checked_add(dropped).ok_or(Error::Overflow)?;
        self.sequence = sequence;
        self.digest = digest;
        self.records = next_records;
        self.dropped = next_dropped;
        Ok(Admission::Inserted)
    }
    /// A terminal is checked against this exact scope and committed chain head.
    pub fn seal(
        &mut self,
        bytes: &[u8],
        claim: &ArtifactContentDigest,
    ) -> Result<Admission, Error> {
        let result = self.seal_inner(bytes, claim);
        if result.is_err() && !matches!(result, Err(Error::Scope)) {
            self.rejected_input = true;
        }
        result
    }
    fn seal_inner(
        &mut self,
        bytes: &[u8],
        claim: &ArtifactContentDigest,
    ) -> Result<Admission, Error> {
        if bytes.len() != TERMINAL_BYTES {
            return Err(Error::Malformed);
        }
        let digest = checked_digest(bytes, claim)?;
        let mut r = Reader { bytes, at: 0 };
        if r.take(4)? != TERMINAL_MAGIC {
            return Err(Error::Malformed);
        }
        let scope = r.scope()?;
        let final_sequence = r.u64()?;
        let final_digest = r.array::<32>()?;
        let dropped = r.u64()?;
        let declared_complete = match r.take(1)?[0] {
            0 => false,
            1 => true,
            _ => return Err(Error::Malformed),
        };
        let terminal = Terminal {
            scope,
            final_sequence,
            final_digest,
            dropped,
            declared_complete,
        };
        if scope != self.scope {
            return Err(Error::Scope);
        }
        if let Some((_, old_digest)) = self.terminal {
            return if digest == old_digest {
                Ok(Admission::Duplicate)
            } else {
                Err(Error::Conflict)
            };
        }
        if final_sequence != self.sequence || final_digest != self.digest || dropped != self.dropped
        {
            return Err(Error::TerminalMismatch);
        }
        self.terminal = Some((terminal, digest));
        Ok(Admission::Inserted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence_manifest::{DigestAlgorithm, DigestClaim, DigestRepresentation};
    fn scope() -> Scope {
        Scope {
            job: [1; 16],
            epoch: [2; 16],
        }
    }
    fn claim(bytes: &[u8]) -> ArtifactContentDigest {
        let hex = Sha256::digest(bytes)
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect();
        ArtifactContentDigest(DigestClaim {
            algorithm: DigestAlgorithm::Sha256,
            representation: DigestRepresentation::ExactBytes,
            hex,
        })
    }
    fn segment(
        scope: Scope,
        sequence: u64,
        previous: [u8; 32],
        dropped: u64,
        records: &[&[u8]],
    ) -> Vec<u8> {
        encode_segment(Segment {
            scope,
            sequence,
            previous,
            dropped,
            records,
        })
        .unwrap()
    }
    fn terminal(verifier: &Verifier, complete: bool, dropped: u64) -> Vec<u8> {
        encode_terminal(Terminal {
            scope: verifier.scope,
            final_sequence: verifier.sequence,
            final_digest: verifier.digest,
            dropped,
            declared_complete: complete,
        })
    }
    #[test]
    fn contiguous_chain_and_terminal_are_exactly_replayable() {
        let mut v = Verifier::new(scope());
        assert_eq!(v.completeness(), Completeness::MissingTerminal);
        let a = segment(scope(), 1, [0; 32], 0, &[b"one", b"two"]);
        assert_eq!(v.append(&a, &claim(&a)), Ok(Admission::Inserted));
        assert_eq!(v.append(&a, &claim(&a)), Ok(Admission::Duplicate));
        let b = segment(scope(), 2, v.digest(), 0, &[b"three"]);
        assert_eq!(v.append(&b, &claim(&b)), Ok(Admission::Inserted));
        assert_eq!(v.record_count(), 3);
        let seal = terminal(&v, true, 0);
        assert_eq!(v.seal(&seal, &claim(&seal)), Ok(Admission::Inserted));
        assert_eq!(v.completeness(), Completeness::Complete);
        assert_eq!(v.seal(&seal, &claim(&seal)), Ok(Admission::Duplicate));
        assert_eq!(v.append(&b, &claim(&b)), Ok(Admission::Duplicate));
        assert_eq!(v.completeness(), Completeness::Complete);
    }
    #[test]
    fn malformed_scope_sequence_and_conflicts_are_atomic() {
        let mut v = Verifier::new(scope());
        let a = segment(scope(), 1, [0; 32], 0, &[b"x"]);
        v.append(&a, &claim(&a)).unwrap();
        let conflict = segment(scope(), 1, [0; 32], 0, &[b"y"]);
        let gap = segment(scope(), 3, v.digest(), 0, &[b"z"]);
        let chain = segment(scope(), 2, [3; 32], 0, &[b"z"]);
        let foreign = segment(
            Scope {
                job: [9; 16],
                ..scope()
            },
            2,
            v.digest(),
            0,
            &[b"z"],
        );
        let bad_epoch = segment(
            Scope {
                epoch: [9; 16],
                ..scope()
            },
            2,
            v.digest(),
            0,
            &[b"z"],
        );
        for (bytes, error) in [
            (&conflict, Error::Conflict),
            (&gap, Error::Sequence),
            (&chain, Error::Chain),
            (&foreign, Error::Scope),
            (&bad_epoch, Error::Scope),
        ] {
            let before = v.clone();
            assert_eq!(v.append(bytes, &claim(bytes)), Err(error));
            assert_eq!(
                (v.sequence(), v.digest(), v.record_count()),
                (before.sequence(), before.digest(), before.record_count())
            );
        }
        let mut truncated = segment(scope(), 2, v.digest(), 0, &[b"hello"]);
        truncated.pop();
        let before = v.clone();
        assert_eq!(
            v.append(&truncated, &claim(&truncated)),
            Err(Error::Malformed)
        );
        assert_eq!(
            (v.sequence(), v.digest(), v.record_count()),
            (before.sequence(), before.digest(), before.record_count())
        );
        let mut changed = a.clone();
        *changed.last_mut().unwrap() ^= 1;
        assert_eq!(v.append(&changed, &claim(&a)), Err(Error::Digest));
        assert_eq!(
            (v.sequence(), v.digest(), v.record_count()),
            (before.sequence(), before.digest(), before.record_count())
        );
    }
    #[test]
    fn absence_drop_and_terminal_conflicts_never_show_complete() {
        let mut v = Verifier::new(scope());
        let a = segment(scope(), 1, [0; 32], 2, &[b"x"]);
        v.append(&a, &claim(&a)).unwrap();
        assert_eq!(v.completeness(), Completeness::MissingTerminal);
        let wrong = terminal(&v, true, 0);
        let before = v.clone();
        assert_eq!(v.seal(&wrong, &claim(&wrong)), Err(Error::TerminalMismatch));
        assert_eq!(
            (v.sequence(), v.digest(), v.record_count()),
            (before.sequence(), before.digest(), before.record_count())
        );
        let good = terminal(&v, true, 2);
        v.seal(&good, &claim(&good)).unwrap();
        assert_eq!(v.completeness(), Completeness::ObservedDrops { count: 2 });
        let other = terminal(&v, false, 2);
        assert_eq!(v.seal(&other, &claim(&other)), Err(Error::Conflict));
        let mut incomplete = Verifier::new(scope());
        let t = terminal(&incomplete, false, 0);
        incomplete.seal(&t, &claim(&t)).unwrap();
        assert_eq!(incomplete.completeness(), Completeness::ProducerIncomplete);
    }
    #[test]
    fn terminal_first_then_scoped_segment_taints_without_advancing() {
        let mut v = Verifier::new(scope());
        let t = terminal(&v, true, 0);
        assert_eq!(v.seal(&t, &claim(&t)), Ok(Admission::Inserted));
        assert_eq!(v.seal(&t, &claim(&t)), Ok(Admission::Duplicate));
        assert_eq!(v.completeness(), Completeness::Complete);
        let foreign = segment(
            Scope {
                job: [7; 16],
                ..scope()
            },
            1,
            [0; 32],
            0,
            &[b"x"],
        );
        assert_eq!(v.append(&foreign, &claim(&foreign)), Err(Error::Scope));
        assert_eq!(v.completeness(), Completeness::Complete);
        let scoped = segment(scope(), 1, [0; 32], 0, &[b"x"]);
        assert_eq!(v.append(&scoped, &claim(&scoped)), Err(Error::Sealed));
        assert_eq!(v.completeness(), Completeness::RejectedInput);
        assert_eq!(
            (v.sequence(), v.digest(), v.record_count()),
            (0, [0; 32], 0)
        );
        assert_eq!(v.seal(&t, &claim(&t)), Ok(Admission::Duplicate));
        assert_eq!(v.completeness(), Completeness::RejectedInput);
    }
    #[test]
    fn conflicting_terminal_and_unverified_bytes_taint_completion() {
        let mut v = Verifier::new(scope());
        let t = terminal(&v, true, 0);
        v.seal(&t, &claim(&t)).unwrap();
        let foreign = encode_terminal(Terminal {
            scope: Scope {
                epoch: [7; 16],
                ..scope()
            },
            final_sequence: 0,
            final_digest: [0; 32],
            dropped: 0,
            declared_complete: false,
        });
        assert_eq!(v.seal(&foreign, &claim(&foreign)), Err(Error::Scope));
        assert_eq!(v.completeness(), Completeness::Complete);
        let conflict = terminal(&v, false, 0);
        assert_eq!(v.seal(&conflict, &claim(&conflict)), Err(Error::Conflict));
        assert_eq!(v.completeness(), Completeness::RejectedInput);
        assert_eq!(v.seal(&t, &claim(&t)), Ok(Admission::Duplicate));
        let mut fresh = Verifier::new(scope());
        fresh.seal(&t, &claim(&t)).unwrap();
        assert_eq!(
            fresh.append(b"truncated", &claim(b"truncated")),
            Err(Error::Malformed)
        );
        assert_eq!(fresh.completeness(), Completeness::RejectedInput);
    }
    #[test]
    fn rejected_gap_cannot_be_hidden_by_a_terminal() {
        let mut v = Verifier::new(scope());
        let gap = segment(scope(), 2, [0; 32], 0, &[b"x"]);
        assert_eq!(v.append(&gap, &claim(&gap)), Err(Error::Sequence));
        let t = terminal(&v, true, 0);
        v.seal(&t, &claim(&t)).unwrap();
        assert_eq!(v.completeness(), Completeness::RejectedInput);
    }
    #[test]
    fn bounds_truncation_version_and_wrong_terminal_are_refused() {
        let long = vec![0; MAX_RECORD_BYTES + 1];
        assert_eq!(
            encode_segment(Segment {
                scope: scope(),
                sequence: 1,
                previous: [0; 32],
                dropped: 0,
                records: &[&long]
            }),
            Err(Error::Limit)
        );
        let too_many = vec![&b""[..]; MAX_RECORDS + 1];
        assert_eq!(
            encode_segment(Segment {
                scope: scope(),
                sequence: 1,
                previous: [0; 32],
                dropped: 0,
                records: &too_many
            }),
            Err(Error::Limit)
        );
        let mut v = Verifier::new(scope());
        let big = vec![0; MAX_SEGMENT_BYTES + 1];
        assert_eq!(v.append(&big, &claim(&big)), Err(Error::Limit));
        let mut s = segment(scope(), 1, [0; 32], 0, &[b"x"]);
        s[3] = b'2';
        assert_eq!(v.append(&s, &claim(&s)), Err(Error::Malformed));
        let mut t = terminal(&v, true, 0);
        t.pop();
        assert_eq!(v.seal(&t, &claim(&t)), Err(Error::Malformed));
        let t = encode_terminal(Terminal {
            scope: Scope {
                epoch: [7; 16],
                ..scope()
            },
            final_sequence: 0,
            final_digest: [0; 32],
            dropped: 0,
            declared_complete: true,
        });
        assert_eq!(v.seal(&t, &claim(&t)), Err(Error::Scope));
        assert_eq!(v.completeness(), Completeness::MissingTerminal);
    }
}
