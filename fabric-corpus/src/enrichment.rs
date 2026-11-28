//! Pure, in-memory enrichment identity and cache checks. No provider is invoked.
//! Output and scope are caller claims; neither truth nor authority is implied.
use crate::{RevisionDigest, portable::PortableView};
use sha2::{Digest, Sha256};

pub const MAX_SELECTED: usize = 128;
pub const MAX_ID_BYTES: usize = 256;
pub const MAX_INPUT_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
pub const MAX_OPERATIONS: u32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Disabled,
    Unavailable,
    Available,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub selected: usize,
    pub operations: u32,
}
/// All identifiers are caller supplied, including scope. They do not grant access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub view_digest: [u8; 32],
    pub root: RevisionDigest,
    /// Strictly increasing exact selected revision digests, including root.
    pub selected: Vec<RevisionDigest>,
    pub scope: String,
    pub provider: String,
    pub provider_version: String,
    pub model: String,
    pub model_version: String,
    pub contract: String,
    pub config_digest: [u8; 32],
    pub limits: Limits,
}
/// A closed validated in-memory key; no persistent encoding is defined.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    request: Request,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preparation {
    Disabled,
    Unavailable,
    Ready(Box<Prepared>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Limit,
    InvalidIdentity,
    ViewMismatch,
    RootMismatch,
    Portable,
    Omitted,
    Missing,
    CacheMismatch,
    OutputLength,
    OutputDigest,
}
fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_ID_BYTES && !s.chars().any(char::is_control)
}
/// Validates all inputs before producing a key suitable for a cache lookup.
pub fn prepare(
    availability: Availability,
    request: Request,
    view_bytes: &[u8],
    expected_view: [u8; 32],
    expected_root: RevisionDigest,
) -> Result<Preparation, Error> {
    match availability {
        Availability::Disabled => return Ok(Preparation::Disabled),
        Availability::Unavailable => return Ok(Preparation::Unavailable),
        Availability::Available => (),
    }
    let l = request.limits;
    if l.input_bytes == 0
        || l.input_bytes > MAX_INPUT_BYTES
        || l.output_bytes == 0
        || l.output_bytes > MAX_OUTPUT_BYTES
        || l.selected == 0
        || l.selected > MAX_SELECTED
        || l.operations == 0
        || l.operations > MAX_OPERATIONS
        || view_bytes.len() > l.input_bytes
        || request.selected.len() > l.selected
    {
        return Err(Error::Limit);
    }
    if ![
        &request.scope,
        &request.provider,
        &request.provider_version,
        &request.model,
        &request.model_version,
        &request.contract,
    ]
    .iter()
    .all(|s| valid_id(s))
    {
        return Err(Error::InvalidIdentity);
    }
    if request.selected.is_empty() || request.selected.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(Error::InvalidIdentity);
    }
    if request.view_digest != expected_view {
        return Err(Error::ViewMismatch);
    }
    if request.root != expected_root || request.selected.binary_search(&request.root).is_err() {
        return Err(Error::RootMismatch);
    }
    let view = PortableView::import(view_bytes, expected_view).map_err(|_| Error::Portable)?;
    for id in &request.selected {
        match view.view().revision(id) {
            Some(Some(_)) => (),
            Some(None) => return Err(Error::Missing),
            None => return Err(Error::Omitted),
        }
    }
    Ok(Preparation::Ready(Box::new(Prepared { request })))
}
impl Prepared {
    /// Compare the complete key and validate the claimed output length and digest.
    /// The returned bytes are a derivative claim, not independently verified content.
    pub fn validate_cached<'a>(
        &self,
        stored_key: &Prepared,
        bytes: &'a [u8],
        claimed_len: usize,
        claimed_digest: [u8; 32],
    ) -> Result<&'a [u8], Error> {
        if self != stored_key {
            return Err(Error::CacheMismatch);
        }
        if bytes.len() > self.request.limits.output_bytes {
            return Err(Error::Limit);
        }
        if bytes.len() != claimed_len {
            return Err(Error::OutputLength);
        }
        if <[u8; 32]>::from(Sha256::digest(bytes)) != claimed_digest {
            return Err(Error::OutputDigest);
        }
        Ok(bytes)
    }
}
