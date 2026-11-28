//! Bounded inert handoff declarations over a digest-checked portable view.
//! Policy IDs and availability are claims; citations are not byte verification.
use crate::{
    RelationDigest, RevisionDigest,
    differential::{self, Applicability, Dependencies},
    portable::{AvailabilityClaim, PortableView},
    source::{CitationV1, source_claim},
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
pub const MAX_REVISIONS: usize = 128;
pub const MAX_RELATIONS: usize = 128;
pub const MAX_CITATIONS: usize = 64;
pub const MAX_BYTES: usize = 32 * 1024;
const MAGIC: &[u8; 8] = b"FCHAND\0\x01";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    NoneClaimed,
    Omitted,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Continuation {
    NotRequested,
    Unresolved,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declaration {
    pub basis_digest: [u8; 32],
    pub root: RevisionDigest,
    pub policy_id: [u8; 32],
    pub revisions: BTreeSet<RevisionDigest>,
    pub relations: BTreeSet<RelationDigest>,
    /// Declared identities only; source bytes must be independently verified.
    pub citations: Vec<CitationV1>,
    pub evidence_omission: Marker,
    pub dissent: Marker,
    pub continuation: Continuation,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handoff {
    declaration: Declaration,
    bytes: Vec<u8>,
    digest: [u8; 32],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Present(AvailabilityClaim),
    Missing(AvailabilityClaim),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Limit,
    Invalid,
    Digest,
    PolicyMismatch,
    RootMismatch,
    Portable,
    Omitted,
    MissingRoot,
    BasisMismatch,
    Citation,
    Differential,
}
fn marker(value: Marker) -> u8 {
    match value {
        Marker::NoneClaimed => 0,
        Marker::Omitted => 1,
        Marker::Unknown => 2,
    }
}
fn decode_marker(value: u8) -> Result<Marker, Error> {
    match value {
        0 => Ok(Marker::NoneClaimed),
        1 => Ok(Marker::Omitted),
        2 => Ok(Marker::Unknown),
        _ => Err(Error::Invalid),
    }
}
fn key(c: &CitationV1) -> (RevisionDigest, u32, u64, u64, [u8; 32]) {
    (c.revision, c.source_index, c.start, c.end, c.passage_sha256)
}
fn validate(d: &Declaration, portable: &PortableView) -> Result<(), Error> {
    if d.revisions.is_empty()
        || d.revisions.len() > MAX_REVISIONS
        || d.relations.len() > MAX_RELATIONS
        || d.citations.len() > MAX_CITATIONS
    {
        return Err(Error::Limit);
    }
    if !d.revisions.contains(&d.root) {
        return Err(Error::RootMismatch);
    }
    match portable.view().revision(&d.root) {
        Some(Some(_)) => (),
        Some(None) => return Err(Error::MissingRoot),
        None => return Err(Error::Omitted),
    }
    for id in &d.revisions {
        if portable.view().revision(id).is_none() {
            return Err(Error::Omitted);
        }
    }
    for id in &d.relations {
        if portable.view().relation(id).is_none() {
            return Err(Error::Omitted);
        }
    }
    for c in &d.citations {
        if !d.revisions.contains(&c.revision) || c.start >= c.end {
            return Err(Error::Citation);
        }
        let body = portable
            .view()
            .revision(&c.revision)
            .and_then(Option::as_ref)
            .ok_or(Error::Citation)?;
        source_claim(body, c.source_index).map_err(|_| Error::Citation)?;
    }
    if d.citations.windows(2).any(|p| key(&p[0]) >= key(&p[1])) {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn encode(d: &Declaration) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(MAGIC);
    b.extend_from_slice(&d.basis_digest);
    b.extend_from_slice(&d.root);
    b.extend_from_slice(&d.policy_id);
    b.push(marker(d.evidence_omission));
    b.push(marker(d.dissent));
    b.push(match d.continuation {
        Continuation::NotRequested => 0,
        Continuation::Unresolved => 1,
    });
    b.extend_from_slice(&(d.revisions.len() as u16).to_be_bytes());
    for id in &d.revisions {
        b.extend_from_slice(id);
    }
    b.extend_from_slice(&(d.relations.len() as u16).to_be_bytes());
    for id in &d.relations {
        b.extend_from_slice(id);
    }
    b.extend_from_slice(&(d.citations.len() as u16).to_be_bytes());
    for c in &d.citations {
        b.extend_from_slice(&c.revision);
        b.extend_from_slice(&c.source_index.to_be_bytes());
        b.extend_from_slice(&c.start.to_be_bytes());
        b.extend_from_slice(&c.end.to_be_bytes());
        b.extend_from_slice(&c.passage_sha256);
    }
    b
}
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(n).ok_or(Error::Invalid)?;
        let v = self.bytes.get(self.at..end).ok_or(Error::Invalid)?;
        self.at = end;
        Ok(v)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        self.take(N)?.try_into().map_err(|_| Error::Invalid)
    }
    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn count(&mut self, max: usize) -> Result<usize, Error> {
        let n = u16::from_be_bytes(self.array()?) as usize;
        if n > max { Err(Error::Limit) } else { Ok(n) }
    }
}
impl Handoff {
    /// Caller supplies bounded input and independently expected root and policy.
    pub fn create(
        mut d: Declaration,
        portable_bytes: &[u8],
        expected_root: RevisionDigest,
        expected_policy: [u8; 32],
        expected_basis: [u8; 32],
    ) -> Result<Self, Error> {
        if d.revisions.len() > MAX_REVISIONS
            || d.relations.len() > MAX_RELATIONS
            || d.citations.len() > MAX_CITATIONS
        {
            return Err(Error::Limit);
        }
        if d.basis_digest != expected_basis {
            return Err(Error::BasisMismatch);
        }
        if d.root != expected_root {
            return Err(Error::RootMismatch);
        }
        if d.policy_id != expected_policy {
            return Err(Error::PolicyMismatch);
        }
        let portable =
            PortableView::import(portable_bytes, d.basis_digest).map_err(|_| Error::Portable)?;
        d.citations.sort_by_key(key);
        validate(&d, &portable)?;
        let bytes = encode(&d);
        if bytes.len() > MAX_BYTES {
            return Err(Error::Limit);
        }
        let digest = Sha256::digest(&bytes).into();
        Ok(Self {
            declaration: d,
            bytes,
            digest,
        })
    }
    /// Checks descriptor bytes, explicit identities and portable bytes before use.
    pub fn import(
        bytes: &[u8],
        expected_digest: [u8; 32],
        portable_bytes: &[u8],
        expected_root: RevisionDigest,
        expected_policy: [u8; 32],
        expected_basis: [u8; 32],
    ) -> Result<Self, Error> {
        if bytes.len() > MAX_BYTES {
            return Err(Error::Limit);
        }
        if <[u8; 32]>::from(Sha256::digest(bytes)) != expected_digest {
            return Err(Error::Digest);
        }
        let mut r = Reader { bytes, at: 0 };
        if r.take(8)? != MAGIC {
            return Err(Error::Invalid);
        }
        let basis_digest = r.array()?;
        let root = r.array()?;
        let policy_id = r.array()?;
        let evidence_omission = decode_marker(r.byte()?)?;
        let dissent = decode_marker(r.byte()?)?;
        let continuation = match r.byte()? {
            0 => Continuation::NotRequested,
            1 => Continuation::Unresolved,
            _ => return Err(Error::Invalid),
        };
        let n = r.count(MAX_REVISIONS)?;
        let mut revisions = BTreeSet::new();
        let mut prev = None;
        for _ in 0..n {
            let id = r.array()?;
            if prev.is_some_and(|p| p >= id) {
                return Err(Error::Invalid);
            }
            prev = Some(id);
            revisions.insert(id);
        }
        let n = r.count(MAX_RELATIONS)?;
        let mut relations = BTreeSet::new();
        let mut prev = None;
        for _ in 0..n {
            let id = r.array()?;
            if prev.is_some_and(|p| p >= id) {
                return Err(Error::Invalid);
            }
            prev = Some(id);
            relations.insert(id);
        }
        let n = r.count(MAX_CITATIONS)?;
        let mut citations = Vec::with_capacity(n);
        for _ in 0..n {
            citations.push(CitationV1 {
                revision: r.array()?,
                source_index: u32::from_be_bytes(r.array()?),
                start: u64::from_be_bytes(r.array()?),
                end: u64::from_be_bytes(r.array()?),
                passage_sha256: r.array()?,
            });
        }
        if r.at != bytes.len() {
            return Err(Error::Invalid);
        }
        if citations.windows(2).any(|p| key(&p[0]) >= key(&p[1])) {
            return Err(Error::Invalid);
        }
        let d = Declaration {
            basis_digest,
            root,
            policy_id,
            revisions,
            relations,
            citations,
            evidence_omission,
            dissent,
            continuation,
        };
        let h = Self::create(
            d,
            portable_bytes,
            expected_root,
            expected_policy,
            expected_basis,
        )?;
        if h.bytes != bytes {
            return Err(Error::Invalid);
        }
        Ok(h)
    }
    pub fn declaration(&self) -> &Declaration {
        &self.declaration
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }
    /// Reports captured availability labels, not actual retrieval or custody.
    pub fn revision_selection(
        &self,
        portable_bytes: &[u8],
        id: &RevisionDigest,
    ) -> Result<Selection, Error> {
        if !self.declaration.revisions.contains(id) {
            return Err(Error::Omitted);
        }
        let v = PortableView::import(portable_bytes, self.declaration.basis_digest)
            .map_err(|_| Error::Portable)?;
        let claim = *v.availability().get(id).ok_or(Error::Omitted)?;
        match v.view().revision(id) {
            Some(Some(_)) => Ok(Selection::Present(claim)),
            Some(None) => Ok(Selection::Missing(claim)),
            None => Err(Error::Omitted),
        }
    }
    /// From/to names do not establish chronology; policy mismatch is explicit.
    pub fn compare_current(
        &self,
        original: &[u8],
        current: &[u8],
        current_digest: [u8; 32],
        current_root: RevisionDigest,
        current_policy: [u8; 32],
    ) -> Result<Applicability, Error> {
        if current_root != self.declaration.root {
            return Err(Error::RootMismatch);
        }
        if current_policy != self.declaration.policy_id {
            return Err(Error::PolicyMismatch);
        }
        let deps = Dependencies {
            revisions: self.declaration.revisions.clone(),
            relations: self.declaration.relations.clone(),
            groups: BTreeSet::new(),
        };
        differential::compare(
            original,
            self.declaration.basis_digest,
            current,
            current_digest,
            &deps,
        )
        .map(|d| d.applicability())
        .map_err(|_| Error::Differential)
    }
}
