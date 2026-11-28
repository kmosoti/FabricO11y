//! Inert caller-supplied investigation proposals bound to a frozen selection.
//! Duplicate checks and text are claims, not acceptance or execution authority.
use crate::{
    Encoder, RevisionDigest, Value,
    portable::{PortableError, PortableView},
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const VERSION: u8 = 1;
pub const MAX_ANCHORS: usize = 64;
pub const MAX_ALTERNATIVES: usize = 16;
pub const MAX_EXTENSIONS: usize = 32;
pub const MAX_ID_BYTES: usize = 128;
pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_PROPOSAL_BYTES: usize = 64 * 1024;
pub const MAX_BATCH: usize = 128;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alternative {
    pub id: String,
    pub description: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extension {
    pub namespace: String,
    pub name: String,
    pub value: Value,
}
/// Attribution only: no check is performed by this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DuplicateCheck {
    Unknown,
    NoMatch { scope: String },
    PossibleMatch { proposal_id: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub version: u8,
    pub proposal_id: String,
    pub basis_digest: [u8; 32],
    pub anchors: Vec<RevisionDigest>,
    pub alternatives: Vec<Alternative>,
    pub discriminating_test: String,
    pub duplicate_check: DuplicateCheck,
    pub extensions: Vec<Extension>,
}
/// Validated immutable data; no mutation or accepted-plan transition is exposed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    input: Input,
    canonical: Vec<u8>,
    digest: [u8; 32],
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Version,
    Limit,
    Invalid,
    DuplicateIdentity,
    AnchorOmitted,
    AnchorMissing,
    Portable(PortableError),
}
fn valid(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}
fn namespace(s: &str) -> bool {
    valid(s, MAX_ID_BYTES)
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
impl Proposal {
    /// Rejects unselected or selected-missing anchors; selection is not authority.
    /// Caller must bound decoding and allocation before constructing `input`.
    pub fn new(bytes: &[u8], mut input: Input) -> Result<Self, Error> {
        if input.version != VERSION {
            return Err(Error::Version);
        }
        if input.anchors.is_empty()
            || input.anchors.len() > MAX_ANCHORS
            || !(2..=MAX_ALTERNATIVES).contains(&input.alternatives.len())
            || input.extensions.len() > MAX_EXTENSIONS
        {
            return Err(Error::Limit);
        }
        if !valid(&input.proposal_id, MAX_ID_BYTES)
            || !valid(&input.discriminating_test, MAX_TEXT_BYTES)
        {
            return Err(Error::Invalid);
        }
        match &input.duplicate_check {
            DuplicateCheck::Unknown => (),
            DuplicateCheck::NoMatch { scope } if valid(scope, MAX_ID_BYTES) => (),
            DuplicateCheck::PossibleMatch { proposal_id } if valid(proposal_id, MAX_ID_BYTES) => (),
            _ => return Err(Error::Invalid),
        }
        input.anchors.sort_unstable();
        if input.anchors.windows(2).any(|x| x[0] == x[1]) {
            return Err(Error::DuplicateIdentity);
        }
        input.alternatives.sort_by(|a, b| a.id.cmp(&b.id));
        let mut descriptions = BTreeSet::new();
        for alternative in &input.alternatives {
            if !valid(&alternative.id, MAX_ID_BYTES)
                || !valid(&alternative.description, MAX_TEXT_BYTES)
            {
                return Err(Error::Invalid);
            }
            if !descriptions.insert(&alternative.description) {
                return Err(Error::DuplicateIdentity);
            }
        }
        if input.alternatives.windows(2).any(|x| x[0].id == x[1].id) {
            return Err(Error::DuplicateIdentity);
        }
        input
            .extensions
            .sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
        for extension in &input.extensions {
            if !namespace(&extension.namespace) || !namespace(&extension.name) {
                return Err(Error::Invalid);
            }
        }
        if input
            .extensions
            .windows(2)
            .any(|x| x[0].namespace == x[1].namespace && x[0].name == x[1].name)
        {
            return Err(Error::DuplicateIdentity);
        }
        let view = PortableView::import(bytes, input.basis_digest).map_err(Error::Portable)?;
        for anchor in &input.anchors {
            match view.view().revision(anchor) {
                None => return Err(Error::AnchorOmitted),
                Some(None) => return Err(Error::AnchorMissing),
                Some(Some(_)) => (),
            }
        }
        let mut encoder = Encoder::new(b"fabric-corpus/investigation-proposal\0\x01");
        let encode = |e: &mut Encoder| -> Result<(), crate::Error> {
            e.required(&input.proposal_id, "proposal id")?;
            e.push(&input.basis_digest)?;
            e.count(input.anchors.len())?;
            for anchor in &input.anchors {
                e.push(anchor)?;
            }
            e.count(input.alternatives.len())?;
            for a in &input.alternatives {
                e.required(&a.id, "alternative id")?;
                e.required(&a.description, "alternative")?;
            }
            e.required(&input.discriminating_test, "discriminating test")?;
            match &input.duplicate_check {
                DuplicateCheck::Unknown => e.push(&[0])?,
                DuplicateCheck::NoMatch { scope } => {
                    e.push(&[1])?;
                    e.required(scope, "scope")?;
                }
                DuplicateCheck::PossibleMatch { proposal_id } => {
                    e.push(&[2])?;
                    e.required(proposal_id, "proposal id")?;
                }
            }
            e.count(input.extensions.len())?;
            for x in &input.extensions {
                e.required(&x.namespace, "namespace")?;
                e.required(&x.name, "name")?;
                e.value(&x.value, 0)?;
            }
            Ok(())
        };
        encode(&mut encoder).map_err(|_| Error::Limit)?;
        if encoder.bytes.len() > MAX_PROPOSAL_BYTES {
            return Err(Error::Limit);
        }
        let canonical = encoder.bytes;
        let digest = Sha256::digest(&canonical).into();
        Ok(Self {
            input,
            canonical,
            digest,
        })
    }
    pub fn input(&self) -> &Input {
        &self.input
    }
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical
    }
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }
}
/// Checks identities only within the explicitly supplied bounded batch.
pub fn check_unique(proposals: &[Proposal]) -> Result<(), Error> {
    if proposals.len() > MAX_BATCH {
        return Err(Error::Limit);
    }
    let mut seen = BTreeMap::new();
    for proposal in proposals {
        if seen
            .insert(&proposal.input.proposal_id, proposal.digest)
            .is_some()
        {
            return Err(Error::DuplicateIdentity);
        }
    }
    Ok(())
}
