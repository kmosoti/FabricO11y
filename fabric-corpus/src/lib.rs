//! Transport-free, append-only research record contracts.
//!
//! Canonical profile 1 is a binary format, not JSON. Strings are UTF-8, ordered
//! maps use UTF-8 byte order, and lengths and counts are unsigned 64-bit
//! big-endian integers. Every value has a one-byte tag. No Unicode normalization
//! or numeric coercion is performed. See `docs/corpus-contract.md`.

pub mod as_of;
pub mod context;
pub mod differential;
pub mod failure_claim;
pub mod handoff;
pub mod investigation;
pub mod investigation_proposal;
pub mod lexical;
pub mod portable;
pub mod reconcile;
pub mod source;

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_CANONICAL_BYTES: usize = 1024 * 1024;
pub const MAX_DEPTH: usize = 32;
pub const MAX_ITEMS: usize = 4096;

pub type RevisionDigest = [u8; 32];
pub type RelationDigest = [u8; 32];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationInsertion {
    Inserted(RelationDigest),
    Duplicate(RelationDigest),
}

/// Unknown extensions are data only. Numbers requiring exact representation
/// should be carried as strings with application-defined interpretation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Bool(bool),
    Text(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Hypothesis,
    Source,
    Observation,
    Result,
    Analysis,
    Decision,
    Dissent,
    /// A namespace and local name, both nonempty; no behavior is inferred.
    Custom {
        namespace: String,
        name: String,
    },
}

/// These are claims, not evidence that bytes are presently available or held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceClaim {
    Captured {
        capture_id: String,
        artifact_sha256: [u8; 32],
        locator: String,
        parser: String,
    },
    ExternalOnly {
        locator: String,
        parser: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevisionBody {
    pub object_id: String,
    pub kind: Kind,
    pub text: String,
    /// A set of parent revision digests, encoded in byte order.
    pub parents: BTreeSet<RevisionDigest>,
    pub sources: Vec<SourceClaim>,
    pub extensions: BTreeMap<String, Value>,
}

/// Predicates label recorded claims; custom names have no inferred semantics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Predicate {
    Supports,
    Refines,
    Supersedes,
    Falsifies,
    Waives,
    Custom { namespace: String, name: String },
}

/// A semantic edge is an attributed claim, not an entailment. Cycles are valid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    pub subject: RevisionDigest,
    pub predicate: Predicate,
    pub object: RevisionDigest,
    pub attributed_to: String,
    pub extensions: BTreeMap<String, Value>,
}

impl Relation {
    /// Profile 1 identifies the whole attributed assertion, including extensions.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut e = Encoder::new(b"fabric-corpus/relation\0\x01");
        e.push(&self.subject)?;
        match &self.predicate {
            Predicate::Supports => e.push(&[0])?,
            Predicate::Refines => e.push(&[1])?,
            Predicate::Supersedes => e.push(&[2])?,
            Predicate::Falsifies => e.push(&[3])?,
            Predicate::Waives => e.push(&[4])?,
            Predicate::Custom { namespace, name } => {
                e.push(&[5])?;
                e.required(namespace, "predicate namespace")?;
                e.required(name, "predicate name")?;
            }
        }
        e.push(&self.object)?;
        e.required(&self.attributed_to, "attributed_to")?;
        e.map(&self.extensions, 0)?;
        Ok(e.bytes)
    }
    pub fn digest(&self) -> Result<RelationDigest, Error> {
        Ok(Sha256::digest(self.canonical_bytes()?).into())
    }
}

/// Postcommit metadata, deliberately outside revision identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attestation {
    pub revision: RevisionDigest,
    pub attributed_to: String,
    pub statement: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    EmptyField(&'static str),
    TooLarge,
    TooDeep,
    TooManyItems,
    MissingParent(RevisionDigest),
    ParentObjectMismatch(RevisionDigest),
    MissingRevision(RevisionDigest),
    DuplicateRevision(RevisionDigest),
    InvalidLocator,
    RelationDigestCollision(RelationDigest),
}

struct Encoder {
    bytes: Vec<u8>,
    items: usize,
}
impl Encoder {
    fn new(domain: &[u8]) -> Self {
        Self {
            bytes: domain.to_vec(),
            items: 0,
        }
    }
    fn push(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|n| n > MAX_CANONICAL_BYTES)
        {
            return Err(Error::TooLarge);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    fn count(&mut self, n: usize) -> Result<(), Error> {
        self.items = self.items.checked_add(n).ok_or(Error::TooManyItems)?;
        if self.items > MAX_ITEMS {
            return Err(Error::TooManyItems);
        }
        self.push(&(n as u64).to_be_bytes())
    }
    fn string(&mut self, s: &str) -> Result<(), Error> {
        self.push(&(s.len() as u64).to_be_bytes())?;
        self.push(s.as_bytes())
    }
    fn required(&mut self, s: &str, name: &'static str) -> Result<(), Error> {
        if s.is_empty() {
            return Err(Error::EmptyField(name));
        }
        self.string(s)
    }
    fn locator(&mut self, s: &str) -> Result<(), Error> {
        let Some((scheme, rest)) = s.split_once(':') else {
            return Err(Error::InvalidLocator);
        };
        if scheme.is_empty()
            || rest.is_empty()
            || !scheme.as_bytes()[0].is_ascii_alphabetic()
            || !scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
            || s.chars().any(char::is_whitespace)
        {
            return Err(Error::InvalidLocator);
        }
        self.string(s)
    }
    fn value(&mut self, value: &Value, depth: usize) -> Result<(), Error> {
        if depth > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        match value {
            Value::Null => self.push(&[0]),
            Value::Bool(b) => self.push(&[if *b { 2 } else { 1 }]),
            Value::Text(s) => {
                self.push(&[3])?;
                self.string(s)
            }
            Value::Array(a) => {
                self.push(&[4])?;
                self.count(a.len())?;
                for v in a {
                    self.value(v, depth + 1)?;
                }
                Ok(())
            }
            Value::Object(m) => {
                self.push(&[5])?;
                self.map(m, depth + 1)
            }
        }
    }
    fn map(&mut self, m: &BTreeMap<String, Value>, depth: usize) -> Result<(), Error> {
        self.count(m.len())?;
        for (k, v) in m {
            self.required(k, "extension key")?;
            self.value(v, depth)?;
        }
        Ok(())
    }
}

impl RevisionBody {
    /// Canonical profile v1, domain separated from other record types.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut e = Encoder::new(b"fabric-corpus/revision\0\x01");
        e.required(&self.object_id, "object_id")?;
        match &self.kind {
            Kind::Hypothesis => e.push(&[0])?,
            Kind::Source => e.push(&[1])?,
            Kind::Observation => e.push(&[2])?,
            Kind::Result => e.push(&[3])?,
            Kind::Analysis => e.push(&[4])?,
            Kind::Decision => e.push(&[5])?,
            Kind::Dissent => e.push(&[6])?,
            Kind::Custom { namespace, name } => {
                e.push(&[7])?;
                e.required(namespace, "kind namespace")?;
                e.required(name, "kind name")?;
            }
        }
        e.string(&self.text)?;
        e.count(self.parents.len())?;
        for p in &self.parents {
            e.push(p)?;
        }
        e.count(self.sources.len())?;
        for source in &self.sources {
            match source {
                SourceClaim::Captured {
                    capture_id,
                    artifact_sha256,
                    locator,
                    parser,
                } => {
                    e.push(&[0])?;
                    e.required(capture_id, "capture_id")?;
                    e.push(artifact_sha256)?;
                    e.locator(locator)?;
                    e.required(parser, "parser")?;
                }
                SourceClaim::ExternalOnly { locator, parser } => {
                    e.push(&[1])?;
                    e.locator(locator)?;
                    e.required(parser, "parser")?;
                }
            }
        }
        e.map(&self.extensions, 0)?;
        Ok(e.bytes)
    }
    pub fn digest(&self) -> Result<RevisionDigest, Error> {
        Ok(Sha256::digest(self.canonical_bytes()?).into())
    }
}

/// Owned bodies have no mutable accessor. Admit parents before children; this
/// establishes an acyclic revision graph without trusting caller-supplied order.
#[derive(Default)]
pub struct Corpus {
    revisions: BTreeMap<RevisionDigest, RevisionBody>,
    relations: BTreeMap<RelationDigest, Relation>,
    attestations: Vec<Attestation>,
}
impl Corpus {
    pub fn admit(&mut self, body: RevisionBody) -> Result<RevisionDigest, Error> {
        let digest = body.digest()?;
        if self.revisions.contains_key(&digest) {
            return Err(Error::DuplicateRevision(digest));
        }
        for parent in &body.parents {
            let prior = self
                .revisions
                .get(parent)
                .ok_or(Error::MissingParent(*parent))?;
            if prior.object_id != body.object_id {
                return Err(Error::ParentObjectMismatch(*parent));
            }
        }
        self.revisions.insert(digest, body);
        Ok(digest)
    }
    pub fn revision(&self, digest: &RevisionDigest) -> Option<&RevisionBody> {
        self.revisions.get(digest)
    }
    pub fn add_relation(&mut self, relation: Relation) -> Result<RelationInsertion, Error> {
        for digest in [&relation.subject, &relation.object] {
            if !self.revisions.contains_key(digest) {
                return Err(Error::MissingRevision(*digest));
            }
        }
        let digest = relation.digest()?;
        if let Some(existing) = self.relations.get(&digest) {
            return if existing == &relation {
                Ok(RelationInsertion::Duplicate(digest))
            } else {
                Err(Error::RelationDigestCollision(digest))
            };
        }
        self.relations.insert(digest, relation);
        Ok(RelationInsertion::Inserted(digest))
    }
    pub fn add_attestation(&mut self, attestation: Attestation) -> Result<(), Error> {
        if !self.revisions.contains_key(&attestation.revision) {
            return Err(Error::MissingRevision(attestation.revision));
        }
        let mut e = Encoder::new(b"fabric-corpus/attestation\0\x01");
        e.required(&attestation.attributed_to, "attributed_to")?;
        e.required(&attestation.statement, "statement")?;
        self.attestations.push(attestation);
        Ok(())
    }
    pub fn relations(&self) -> &BTreeMap<RelationDigest, Relation> {
        &self.relations
    }
    pub fn attestations(&self) -> &[Attestation] {
        &self.attestations
    }
}
