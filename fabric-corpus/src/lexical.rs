//! Rebuild an exact, bounded adjacency projection from selected revision text.
//!
//! A token is a maximal run of ASCII letters or digits, lowercased in ASCII.
//! All other bytes delimit tokens. Adjacency never crosses revision boundaries.
//! Output bytes count token UTF-8 bytes, eight bytes per pair count, and 32 bytes
//! per selected/missing identifier and per pair provenance identifier. This is
//! a logical payload budget, not a serialized format or allocator measurement.
use crate::{
    RevisionBody, RevisionDigest,
    as_of::{AsOfView, MAX_VIEW_RECORD_BYTES, MAX_VIEW_REVISIONS},
};
use sha2::{Digest, Sha256};
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

fn validate_limits(limits: Limits) -> Result<(), LexicalError> {
    if limits.scan_bytes > MAX_SCAN_BYTES
        || limits.tokens > MAX_TOKENS
        || limits.token_bytes > MAX_TOKEN_BYTES
        || limits.distinct_pairs > MAX_PAIRS
        || limits.provenance > MAX_PROVENANCE
        || limits.output_bytes > MAX_OUTPUT_BYTES
    {
        return Err(LexicalError::InvalidLimits);
    }
    Ok(())
}
fn empty() -> LexicalView {
    LexicalView {
        selected: BTreeSet::new(),
        missing: BTreeSet::new(),
        pairs: BTreeMap::new(),
        cost: Cost::default(),
    }
}

/// Returns an entire projection or an error; no partial projection escapes.
pub fn rebuild(view: &AsOfView, limits: Limits) -> Result<LexicalView, LexicalError> {
    validate_limits(limits)?;
    let mut result = empty();
    for (id, body) in view.revisions() {
        project_one(&mut result, id, body.as_ref(), limits)?;
    }
    Ok(result)
}

fn project_one(
    result: &mut LexicalView,
    id: &RevisionDigest,
    body: Option<&RevisionBody>,
    limits: Limits,
) -> Result<(), LexicalError> {
    let id_bytes = if body.is_some() { 32 } else { 64 };
    result.cost.output_bytes = add(result.cost.output_bytes, id_bytes, limits.output_bytes)?;
    result.selected.insert(*id);
    let Some(body) = body else {
        result.missing.insert(*id);
        return Ok(());
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
    Ok(())
}

/// Caller-supplied append to an explicit selection, not an authoritative feed.
/// A missing selection cannot later be filled in place; rebuild for that change.
#[derive(Clone, Copy, Debug)]
pub struct RevisionEvent<'a> {
    pub frontier: u64,
    pub revision: RevisionDigest,
    pub body: Option<&'a RevisionBody>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncrementalError {
    Projection(LexicalError),
    InvalidBody,
    DigestMismatch,
    Gap,
    OlderFrontier,
    ConflictingReplay,
    RebuildRequired,
    Limit,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Apply {
    /// Work to scan this revision alone; the view reports cumulative exact cost.
    Applied {
        work: Cost,
    },
    Duplicate,
}
/// Append-only bounded selection. Only the most recent event is replayable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incremental {
    limits: Limits,
    frontier: u64,
    last: Option<(RevisionDigest, bool)>,
    record_bytes: usize,
    view: LexicalView,
}
impl Incremental {
    pub fn new(frontier: u64, limits: Limits) -> Result<Self, IncrementalError> {
        validate_limits(limits).map_err(IncrementalError::Projection)?;
        Ok(Self {
            limits,
            frontier,
            last: None,
            record_bytes: 0,
            view: empty(),
        })
    }
    pub fn frontier(&self) -> u64 {
        self.frontier
    }
    pub fn view(&self) -> &LexicalView {
        &self.view
    }
    /// Stage and preflight a single delta before touching the committed view.
    pub fn append(&mut self, event: RevisionEvent<'_>) -> Result<Apply, IncrementalError> {
        if event.frontier < self.frontier {
            return Err(IncrementalError::OlderFrontier);
        }
        if event.frontier > self.frontier && self.frontier.checked_add(1) != Some(event.frontier) {
            return Err(IncrementalError::Gap);
        }
        let size = if let Some(body) = event.body {
            let bytes = body
                .canonical_bytes()
                .map_err(|_| IncrementalError::InvalidBody)?;
            let digest: RevisionDigest = Sha256::digest(&bytes).into();
            if digest != event.revision {
                return Err(IncrementalError::DigestMismatch);
            }
            bytes.len()
        } else {
            0
        };
        if event.frontier == self.frontier {
            return match self.last {
                Some((id, present)) if id == event.revision && present == event.body.is_some() => {
                    Ok(Apply::Duplicate)
                }
                _ => Err(IncrementalError::ConflictingReplay),
            };
        }
        if self.view.selected.contains(&event.revision) {
            return Err(IncrementalError::RebuildRequired);
        }
        if self.view.selected.len() >= MAX_VIEW_REVISIONS {
            return Err(IncrementalError::Limit);
        }
        let record_bytes = add(self.record_bytes, size, MAX_VIEW_RECORD_BYTES)
            .map_err(IncrementalError::Projection)?;
        let mut delta = empty();
        project_one(&mut delta, &event.revision, event.body, self.limits)
            .map_err(IncrementalError::Projection)?;
        let mut cost = self.view.cost;
        cost.scanned_bytes = add(
            cost.scanned_bytes,
            delta.cost.scanned_bytes,
            self.limits.scan_bytes,
        )
        .map_err(IncrementalError::Projection)?;
        cost.tokens = add(cost.tokens, delta.cost.tokens, self.limits.tokens)
            .map_err(IncrementalError::Projection)?;
        cost.pair_occurrences = cost
            .pair_occurrences
            .checked_add(delta.cost.pair_occurrences)
            .ok_or(IncrementalError::Limit)?;
        cost.provenance = add(
            cost.provenance,
            delta.cost.provenance,
            self.limits.provenance,
        )
        .map_err(IncrementalError::Projection)?;
        let mut bytes = delta.cost.output_bytes;
        let mut new_pairs = 0usize;
        for (key, pair) in &delta.pairs {
            if let Some(existing) = self.view.pairs.get(key) {
                existing
                    .occurrences
                    .checked_add(pair.occurrences)
                    .ok_or(IncrementalError::Limit)?;
                bytes -= key.0.len() + key.1.len() + 8;
            } else {
                new_pairs += 1;
            }
        }
        cost.distinct_pairs = add(cost.distinct_pairs, new_pairs, self.limits.distinct_pairs)
            .map_err(IncrementalError::Projection)?;
        cost.output_bytes = add(cost.output_bytes, bytes, self.limits.output_bytes)
            .map_err(IncrementalError::Projection)?;
        let work = delta.cost;
        self.view.selected.insert(event.revision);
        if event.body.is_none() {
            self.view.missing.insert(event.revision);
        }
        for (key, pair) in delta.pairs {
            if let Some(existing) = self.view.pairs.get_mut(&key) {
                existing.occurrences += pair.occurrences;
                existing.revisions.insert(event.revision);
            } else {
                self.view.pairs.insert(key, pair);
            }
        }
        self.view.cost = cost;
        self.record_bytes = record_bytes;
        self.frontier = event.frontier;
        self.last = Some((event.revision, event.body.is_some()));
        Ok(Apply::Applied { work })
    }
}
