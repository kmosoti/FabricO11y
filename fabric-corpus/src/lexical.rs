//! Rebuild an exact, bounded adjacency projection from selected revision text.
//!
//! A token is a maximal run of ASCII letters or digits, lowercased in ASCII.
//! All other bytes delimit tokens. Adjacency never crosses revision boundaries.
//! Output bytes count token UTF-8 bytes, eight bytes per pair count, and 32 bytes
//! per selected/missing identifier and per pair provenance identifier. This is
//! a logical payload budget, not a serialized format or allocator measurement.
use crate::{RevisionDigest, as_of::AsOfView};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_SCAN_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TOKENS: usize = 262_144;
pub const MAX_TOKEN_BYTES: usize = 128;
pub const MAX_PAIRS: usize = 32_768;
pub const MAX_PROVENANCE: usize = 65_536;
pub const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub scan_bytes: usize,
    pub tokens: usize,
    pub token_bytes: usize,
    pub distinct_pairs: usize,
    pub provenance: usize,
    pub output_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            scan_bytes: MAX_SCAN_BYTES,
            tokens: MAX_TOKENS,
            token_bytes: MAX_TOKEN_BYTES,
            distinct_pairs: MAX_PAIRS,
            provenance: MAX_PROVENANCE,
            output_bytes: MAX_OUTPUT_BYTES,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LexicalError {
    InvalidLimits,
    Limit,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    pub scanned_bytes: usize,
    pub tokens: usize,
    pub pair_occurrences: usize,
    pub distinct_pairs: usize,
    pub provenance: usize,
    pub output_bytes: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    pub occurrences: usize,
    pub revisions: BTreeSet<RevisionDigest>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexicalView {
    pub selected: BTreeSet<RevisionDigest>,
    pub missing: BTreeSet<RevisionDigest>,
    pub pairs: BTreeMap<(String, String), Pair>,
    pub cost: Cost,
}
fn add(value: usize, amount: usize, cap: usize) -> Result<usize, LexicalError> {
    let result = value.checked_add(amount).ok_or(LexicalError::Limit)?;
    if result > cap {
        return Err(LexicalError::Limit);
    }
    Ok(result)
}

/// Returns an entire projection or an error; no partial projection escapes.
pub fn rebuild(view: &AsOfView, limits: Limits) -> Result<LexicalView, LexicalError> {
    if limits.scan_bytes > MAX_SCAN_BYTES
        || limits.tokens > MAX_TOKENS
        || limits.token_bytes > MAX_TOKEN_BYTES
        || limits.distinct_pairs > MAX_PAIRS
        || limits.provenance > MAX_PROVENANCE
        || limits.output_bytes > MAX_OUTPUT_BYTES
    {
        return Err(LexicalError::InvalidLimits);
    }
    let mut result = LexicalView {
        selected: BTreeSet::new(),
        missing: BTreeSet::new(),
        pairs: BTreeMap::new(),
        cost: Cost::default(),
    };
    for (id, body) in view.revisions() {
        let id_bytes = if body.is_some() { 32 } else { 64 };
        result.cost.output_bytes = add(result.cost.output_bytes, id_bytes, limits.output_bytes)?;
        result.selected.insert(*id);
        let Some(body) = body else {
            result.missing.insert(*id);
            continue;
        };
        result.cost.scanned_bytes = add(
            result.cost.scanned_bytes,
            body.text.len(),
            limits.scan_bytes,
        )?;
        let mut prior: Option<String> = None;
        for bytes in body
            .text
            .as_bytes()
            .split(|b| !b.is_ascii_alphanumeric())
            .filter(|b| !b.is_empty())
        {
            result.cost.tokens = add(result.cost.tokens, 1, limits.tokens)?;
            if bytes.len() > limits.token_bytes {
                return Err(LexicalError::Limit);
            }
            let token = String::from_utf8(bytes.iter().map(u8::to_ascii_lowercase).collect())
                .expect("ASCII token");
            if let Some(left) = prior {
                result.cost.pair_occurrences = result
                    .cost
                    .pair_occurrences
                    .checked_add(1)
                    .ok_or(LexicalError::Limit)?;
                let key = (left, token.clone());
                let existing = result.pairs.get(&key);
                let new_pair = existing.is_none();
                let new_provenance = existing.is_none_or(|pair| !pair.revisions.contains(id));
                let next_pairs = add(
                    result.cost.distinct_pairs,
                    usize::from(new_pair),
                    limits.distinct_pairs,
                )?;
                let next_provenance = add(
                    result.cost.provenance,
                    usize::from(new_provenance),
                    limits.provenance,
                )?;
                let pair_bytes = if new_pair {
                    key.0.len() + key.1.len() + 8
                } else {
                    0
                };
                let next_output = add(
                    result.cost.output_bytes,
                    pair_bytes + if new_provenance { 32 } else { 0 },
                    limits.output_bytes,
                )?;
                result.cost.distinct_pairs = next_pairs;
                result.cost.provenance = next_provenance;
                result.cost.output_bytes = next_output;
                let pair = result.pairs.entry(key).or_insert_with(|| Pair {
                    occurrences: 0,
                    revisions: BTreeSet::new(),
                });
                if new_provenance {
                    pair.revisions.insert(*id);
                }
                pair.occurrences = pair.occurrences.checked_add(1).ok_or(LexicalError::Limit)?;
            }
            prior = Some(token);
        }
    }
    Ok(result)
}
