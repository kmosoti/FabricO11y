//! Versioned inert claims about prior attempts, bound to a separate frozen basis.
//! A referenced observation is a claim, not verification, enforcement or custody.
use crate::{
    Corpus, Kind, RevisionBody, RevisionDigest, Value,
    portable::{PortableError, PortableView},
};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_ID_BYTES: usize = 256;
pub const MAX_RECEIPT_BYTES: usize = 1024;
const NS: &str = "fabric.prior-failure";
const CLAIM: &str = "claim";
const OBSERVATION: &str = "attempt-observation";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttemptIdentity {
    pub attempt: String,
    pub candidate: String,
    pub intended_strategy: String,
    pub actual_artifact: String,
    pub diagnosis: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailureClaim {
    pub identity: AttemptIdentity,
    pub basis_digest: [u8; 32],
    pub observation: Option<RevisionDigest>,
    /// An opaque untrusted claim; this module does not authenticate it.
    pub receipt_pointer: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Unsupported,
    Omitted,
    Unknown,
    Conflicting,
    EvidenceReferenced,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaimError {
    Invalid,
    MissingClaim,
    BasisMismatch,
    ClaimInBasis,
    Portable(PortableError),
}

fn valid(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}
fn validate(identity: &AttemptIdentity) -> Result<(), ClaimError> {
    if [
        &identity.attempt,
        &identity.candidate,
        &identity.intended_strategy,
        &identity.actual_artifact,
        &identity.diagnosis,
    ]
    .iter()
    .all(|s| valid(s, MAX_ID_BYTES))
    {
        Ok(())
    } else {
        Err(ClaimError::Invalid)
    }
}
fn hex(id: &[u8; 32]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}
fn digest(s: &str) -> Result<[u8; 32], ClaimError> {
    if s.len() != 64
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ClaimError::Invalid);
    }
    let mut out = [0; 32];
    for (slot, pair) in out.iter_mut().zip(s.as_bytes().as_chunks::<2>().0.iter()) {
        let nibble = |b: u8| {
            if b.is_ascii_digit() {
                b - b'0'
            } else {
                b - b'a' + 10
            }
        };
        *slot = nibble(pair[0]) * 16 + nibble(pair[1]);
    }
    Ok(out)
}
fn field<'a>(body: &'a RevisionBody, key: &str) -> Result<&'a str, ClaimError> {
    match body.extensions.get(key) {
        Some(Value::Text(value)) => Ok(value),
        _ => Err(ClaimError::Invalid),
    }
}
fn optional<'a>(body: &'a RevisionBody, key: &str) -> Result<Option<&'a str>, ClaimError> {
    match body.extensions.get(key) {
        None => Ok(None),
        Some(Value::Text(value)) => Ok(Some(value)),
        _ => Err(ClaimError::Invalid),
    }
}
fn identity(body: &RevisionBody) -> Result<AttemptIdentity, ClaimError> {
    let attempt = field(body, "attempt")?;
    let candidate = field(body, "candidate")?;
    let intended_strategy = field(body, "intended_strategy")?;
    let actual_artifact = field(body, "actual_artifact")?;
    let diagnosis = field(body, "diagnosis")?;
    if ![
        attempt,
        candidate,
        intended_strategy,
        actual_artifact,
        diagnosis,
    ]
    .iter()
    .all(|value| valid(value, MAX_ID_BYTES))
    {
        return Err(ClaimError::Invalid);
    }
    Ok(AttemptIdentity {
        attempt: attempt.into(),
        candidate: candidate.into(),
        intended_strategy: intended_strategy.into(),
        actual_artifact: actual_artifact.into(),
        diagnosis: diagnosis.into(),
    })
}
fn extensions(id: &AttemptIdentity) -> BTreeMap<String, Value> {
    [
        ("version", "1"),
        ("attempt", &id.attempt),
        ("candidate", &id.candidate),
        ("intended_strategy", &id.intended_strategy),
        ("actual_artifact", &id.actual_artifact),
        ("diagnosis", &id.diagnosis),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), Value::Text(v.into())))
    .collect()
}
fn body(
    object_id: &str,
    name: &str,
    text: String,
    extensions: BTreeMap<String, Value>,
) -> Result<RevisionBody, ClaimError> {
    if !valid(object_id, MAX_ID_BYTES) {
        return Err(ClaimError::Invalid);
    }
    let result = RevisionBody {
        object_id: object_id.into(),
        kind: Kind::Custom {
            namespace: NS.into(),
            name: name.into(),
        },
        text,
        parents: BTreeSet::new(),
        sources: vec![],
        extensions,
    };
    result.canonical_bytes().map_err(|_| ClaimError::Invalid)?;
    Ok(result)
}
impl FailureClaim {
    /// Constructs an inert ordinary revision body; it is stored outside its basis.
    pub fn to_body(&self, object_id: &str, text: String) -> Result<RevisionBody, ClaimError> {
        validate(&self.identity)?;
        let mut fields = extensions(&self.identity);
        fields.insert("basis".into(), Value::Text(hex(&self.basis_digest)));
        if let Some(id) = &self.observation {
            fields.insert("observation".into(), Value::Text(hex(id)));
        }
        if let Some(pointer) = &self.receipt_pointer {
            if !valid(pointer, MAX_RECEIPT_BYTES) {
                return Err(ClaimError::Invalid);
            }
            fields.insert("receipt_pointer".into(), Value::Text(pointer.clone()));
        }
        body(object_id, CLAIM, text, fields)
    }
}
/// A typed observed-failure record. Its content remains a recorded claim.
pub fn observation_body(
    object_id: &str,
    id: &AttemptIdentity,
    text: String,
) -> Result<RevisionBody, ClaimError> {
    validate(id)?;
    let mut fields = extensions(id);
    fields.insert("state".into(), Value::Text("observed_failure".into()));
    body(object_id, OBSERVATION, text, fields)
}
fn is_kind(body: &RevisionBody, expected: &str) -> bool {
    matches!(&body.kind, Kind::Custom { namespace, name } if namespace == NS && name == expected)
}
/// Evaluate against exactly the named frozen basis; never re-evaluate an old
/// claim against a later basis or treat a receipt pointer as a verified receipt.
pub fn resolve(
    claims: &Corpus,
    claim_id: RevisionDigest,
    basis: &PortableView,
) -> Result<Disposition, ClaimError> {
    let claim = claims.revision(&claim_id).ok_or(ClaimError::MissingClaim)?;
    if claim.digest().map_err(|_| ClaimError::Invalid)? != claim_id {
        return Err(ClaimError::Invalid);
    }
    if !is_kind(claim, CLAIM) {
        return Err(ClaimError::Invalid);
    }
    let version = field(claim, "version")?;
    let named_basis = digest(field(claim, "basis")?)?;
    let (_, actual_basis) = basis.export().map_err(ClaimError::Portable)?;
    if named_basis != actual_basis {
        return Err(ClaimError::BasisMismatch);
    }
    if basis.view().revision(&claim_id).is_some() {
        return Err(ClaimError::ClaimInBasis);
    }
    if version != "1" {
        return Ok(Disposition::Unknown);
    }
    let expected = identity(claim)?;
    if let Some(pointer) = optional(claim, "receipt_pointer")?
        && !valid(pointer, MAX_RECEIPT_BYTES)
    {
        return Err(ClaimError::Invalid);
    }
    let Some(reference) = optional(claim, "observation")? else {
        return Ok(Disposition::Unsupported);
    };
    let id = digest(reference)?;
    match basis.view().revision(&id) {
        None => Ok(Disposition::Omitted),
        Some(None) => Ok(Disposition::Unknown),
        Some(Some(record)) => {
            if record.digest().map_err(|_| ClaimError::Invalid)? != id {
                return Err(ClaimError::Invalid);
            }
            if !is_kind(record, OBSERVATION) {
                return Ok(Disposition::Conflicting);
            }
            if field(record, "version").ok() != Some("1")
                || field(record, "state").ok() != Some("observed_failure")
            {
                return Ok(Disposition::Unknown);
            }
            match identity(record) {
                Ok(actual) if actual == expected => Ok(Disposition::EvidenceReferenced),
                Ok(_) => Ok(Disposition::Conflicting),
                Err(_) => Ok(Disposition::Unknown),
            }
        }
    }
}
