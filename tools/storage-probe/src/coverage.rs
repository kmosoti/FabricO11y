//! Authenticated coverage receipts for immutable research snapshots.
//!
//! Receipts bind every block and its conservative summary to a separately held
//! anchor. Verification proves metadata coverage; it cannot prove that a block
//! marked `Scanned` was actually read or that a summary builder was honest.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use fabric_o11y::{Attribute, Event, Payload, Scalar};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::Query;

pub type Digest = [u8; 32];

const VERSION: &[u8] = b"fabric-o11y-coverage-v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub min_time: i64,
    pub max_time: i64,
    pub tenants: BTreeSet<u64>,
    pub tokens: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockCommitment {
    pub ordinal: usize,
    pub start: usize,
    pub len: usize,
    pub rows_digest: Digest,
    pub summary_digest: Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub snapshot_id: u64,
    pub block_count: usize,
    pub row_count: usize,
    pub root: Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Disposition {
    Scanned,
    Excluded(Summary),
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockReceipt {
    pub commitment: BlockCommitment,
    pub proof: Vec<Digest>,
    pub disposition: Disposition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub snapshot_id: u64,
    pub query: Query,
    pub blocks: Vec<BlockReceipt>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoverageStatus {
    Complete,
    Incomplete { unavailable: Vec<usize> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoverageError {
    InvalidSummary,
    InvalidLayout,
    InvalidReceipt,
    WrongSnapshot,
    WrongQuery,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub positions: Vec<usize>,
    pub receipt: Receipt,
}

/// A summary derived from nonempty rows, with every row value represented.
pub fn summarize(events: &[Event]) -> Result<Summary, CoverageError> {
    let Some(first) = events.first() else {
        return Err(CoverageError::InvalidSummary);
    };
    let mut summary = Summary {
        min_time: first.event_time.0,
        max_time: first.event_time.0,
        tenants: BTreeSet::new(),
        tokens: BTreeSet::new(),
    };
    for event in events {
        summary.min_time = summary.min_time.min(event.event_time.0);
        summary.max_time = summary.max_time.max(event.event_time.0);
        summary.tenants.insert(event.tenant.0);
        if let Payload::Log { body } = &event.payload {
            summary
                .tokens
                .extend(body.split_whitespace().map(str::to_owned));
        }
    }
    Ok(summary)
}

/// Accept supersets, but reject any summary that could exclude a real row.
pub fn validate_summary(events: &[Event], summary: &Summary) -> Result<(), CoverageError> {
    if events.is_empty() || summary.min_time > summary.max_time {
        return Err(CoverageError::InvalidSummary);
    }
    for event in events {
        if event.event_time.0 < summary.min_time
            || event.event_time.0 > summary.max_time
            || !summary.tenants.contains(&event.tenant.0)
        {
            return Err(CoverageError::InvalidSummary);
        }
        if let Payload::Log { body } = &event.payload {
            if body
                .split_whitespace()
                .any(|token| !summary.tokens.contains(token))
            {
                return Err(CoverageError::InvalidSummary);
            }
        }
    }
    Ok(())
}

/// Domain-separated encoding of the complete summary, including ordered sets.
pub fn summary_digest(summary: &Summary) -> Digest {
    let mut e = Encoder::new(b"summary");
    e.i64(summary.min_time);
    e.i64(summary.max_time);
    e.usize(summary.tenants.len());
    for tenant in &summary.tenants {
        e.u64(*tenant);
    }
    e.usize(summary.tokens.len());
    for token in &summary.tokens {
        e.string(token);
    }
    e.finish()
}

/// Domain-separated digest of ordered rows and every field of each event.
pub fn rows_digest(events: &[Event]) -> Digest {
    let mut e = Encoder::new(b"rows");
    e.usize(events.len());
    for event in events {
        encode_event(&mut e, event);
    }
    e.finish()
}

/// Build an anchor and per-leaf Merkle paths. Layout validity is checked by verify.
pub fn authenticate(
    snapshot_id: u64,
    row_count: usize,
    commitments: &[BlockCommitment],
) -> (Anchor, Vec<Vec<Digest>>) {
    let leaves: Vec<_> = commitments.iter().map(commitment_digest).collect();
    let (tree_root, proofs) = merkle_tree(&leaves);
    let anchor = Anchor {
        snapshot_id,
        block_count: commitments.len(),
        row_count,
        root: anchor_digest(snapshot_id, commitments.len(), row_count, tree_root),
    };
    (anchor, proofs)
}

/// Verify the receipt against an independently retained anchor, without row access.
pub fn verify(
    anchor: &Anchor,
    query: &Query,
    receipt: &Receipt,
) -> Result<CoverageStatus, CoverageError> {
    if receipt.snapshot_id != anchor.snapshot_id {
        return Err(CoverageError::WrongSnapshot);
    }
    if receipt.query != *query {
        return Err(CoverageError::WrongQuery);
    }
    if receipt.blocks.len() != anchor.block_count {
        return Err(CoverageError::InvalidLayout);
    }
    let mut next_start = 0usize;
    for (ordinal, block) in receipt.blocks.iter().enumerate() {
        let c = &block.commitment;
        if c.ordinal != ordinal || c.start != next_start || c.len == 0 {
            return Err(CoverageError::InvalidLayout);
        }
        next_start = next_start
            .checked_add(c.len)
            .ok_or(CoverageError::InvalidLayout)?;
        if block.proof.len() != proof_depth(anchor.block_count) {
            return Err(CoverageError::InvalidReceipt);
        }
    }
    if next_start != anchor.row_count {
        return Err(CoverageError::InvalidLayout);
    }
    if anchor.block_count == 0 && anchor.row_count != 0 {
        return Err(CoverageError::InvalidLayout);
    }
    if anchor.block_count == 0
        && anchor.root != anchor_digest(anchor.snapshot_id, 0, 0, hash(b"empty-tree", &[]))
    {
        return Err(CoverageError::InvalidReceipt);
    }

    let mut unavailable = Vec::new();
    for block in &receipt.blocks {
        let c = &block.commitment;
        let leaf = commitment_digest(c);
        let tree_root = verify_proof(leaf, c.ordinal, anchor.block_count, &block.proof)?;
        if anchor_digest(
            anchor.snapshot_id,
            anchor.block_count,
            anchor.row_count,
            tree_root,
        ) != anchor.root
        {
            return Err(CoverageError::InvalidReceipt);
        }
        match &block.disposition {
            Disposition::Scanned => {}
            Disposition::Unavailable => unavailable.push(c.ordinal),
            Disposition::Excluded(summary) => {
                if summary_digest(summary) != c.summary_digest || !excludes(summary, query) {
                    return Err(CoverageError::InvalidReceipt);
                }
            }
        }
    }
    if unavailable.is_empty() {
        Ok(CoverageStatus::Complete)
    } else {
        Ok(CoverageStatus::Incomplete { unavailable })
    }
}

/// Owned, immutable rows and summaries validated together at construction.
pub struct SealedSnapshot {
    rows: Vec<Event>,
    block_size: NonZeroUsize,
    summaries: Vec<Summary>,
    commitments: Vec<BlockCommitment>,
    anchor: Anchor,
    proofs: Vec<Vec<Digest>>,
}

impl SealedSnapshot {
    pub fn new(
        events: Vec<Event>,
        block_size: NonZeroUsize,
        snapshot_id: u64,
        supplied_summaries: Option<Vec<Summary>>,
    ) -> Result<Self, CoverageError> {
        let block_count = events.len().div_ceil(block_size.get());
        let summaries = match supplied_summaries {
            Some(summaries) if summaries.len() == block_count => summaries,
            Some(_) => return Err(CoverageError::InvalidSummary),
            None => events
                .chunks(block_size.get())
                .map(summarize)
                .collect::<Result<Vec<_>, _>>()?,
        };
        let mut commitments = Vec::with_capacity(block_count);
        let mut start = 0usize;
        for (ordinal, (rows, summary)) in
            events.chunks(block_size.get()).zip(&summaries).enumerate()
        {
            validate_summary(rows, summary)?;
            commitments.push(BlockCommitment {
                ordinal,
                start,
                len: rows.len(),
                rows_digest: rows_digest(rows),
                summary_digest: summary_digest(summary),
            });
            start = start
                .checked_add(rows.len())
                .ok_or(CoverageError::InvalidLayout)?;
        }
        let (anchor, proofs) = authenticate(snapshot_id, events.len(), &commitments);
        Ok(Self {
            rows: events,
            block_size,
            summaries,
            commitments,
            anchor,
            proofs,
        })
    }

    pub fn anchor(&self) -> &Anchor {
        &self.anchor
    }

    /// Digest one immutable row using the same complete-event encoding as its block.
    pub fn row_digest_at(&self, position: usize) -> Option<Digest> {
        self.rows
            .get(position)
            .map(|event| rows_digest(std::slice::from_ref(event)))
    }

    /// Evaluate exact matches and emit one authenticated disposition per block.
    pub fn query(&self, query: &Query, available: &[bool]) -> Result<Answer, CoverageError> {
        if available.len() != self.commitments.len() {
            return Err(CoverageError::InvalidLayout);
        }
        let mut positions = Vec::new();
        let mut blocks = Vec::with_capacity(self.commitments.len());
        for (ordinal, rows) in self.rows.chunks(self.block_size.get()).enumerate() {
            let summary = &self.summaries[ordinal];
            let disposition = if excludes(summary, query) {
                Disposition::Excluded(summary.clone())
            } else if !available[ordinal] {
                Disposition::Unavailable
            } else {
                let start = self.commitments[ordinal].start;
                for (offset, event) in rows.iter().enumerate() {
                    if matches_query(event, query) {
                        positions.push(start + offset);
                    }
                }
                Disposition::Scanned
            };
            blocks.push(BlockReceipt {
                commitment: self.commitments[ordinal].clone(),
                proof: self.proofs[ordinal].clone(),
                disposition,
            });
        }
        Ok(Answer {
            positions,
            receipt: Receipt {
                snapshot_id: self.anchor.snapshot_id,
                query: query.clone(),
                blocks,
            },
        })
    }
}

fn excludes(summary: &Summary, query: &Query) -> bool {
    query.start_ns > query.end_ns
        || summary.max_time < query.start_ns
        || summary.min_time > query.end_ns
        || query
            .tenant
            .is_some_and(|tenant| !summary.tenants.contains(&tenant))
        || query
            .token
            .as_ref()
            .is_some_and(|token| !summary.tokens.contains(token))
}

fn matches_query(event: &Event, query: &Query) -> bool {
    let time = event.event_time.0;
    if time < query.start_ns || time > query.end_ns {
        return false;
    }
    if query.tenant.is_some_and(|tenant| tenant != event.tenant.0) {
        return false;
    }
    match (&query.token, &event.payload) {
        (None, _) => true,
        (Some(token), Payload::Log { body }) => body.split_whitespace().any(|part| part == token),
        (Some(_), Payload::Gauge { .. }) => false,
    }
}

fn encode_event(e: &mut Encoder, event: &Event) {
    e.u64(event.id.0);
    e.u64(event.tenant.0);
    e.u64(event.source.0);
    e.u64(event.resource.0);
    e.i64(event.event_time.0);
    e.i64(event.observed_time.0);
    e.usize(event.attributes.len());
    for Attribute { key, value } in &event.attributes {
        e.string(key);
        match value {
            Scalar::Bool(value) => {
                e.tag(0);
                e.bool(*value);
            }
            Scalar::I64(value) => {
                e.tag(1);
                e.i64(*value);
            }
            Scalar::U64(value) => {
                e.tag(2);
                e.u64(*value);
            }
            Scalar::F64(value) => {
                e.tag(3);
                e.u64(value.to_bits());
            }
            Scalar::String(value) => {
                e.tag(4);
                e.string(value);
            }
        }
    }
    match &event.payload {
        Payload::Log { body } => {
            e.tag(0);
            e.string(body);
        }
        Payload::Gauge { name, value, unit } => {
            e.tag(1);
            e.string(name);
            e.u64(value.to_bits());
            e.string(unit);
        }
    }
}

fn commitment_digest(commitment: &BlockCommitment) -> Digest {
    let mut e = Encoder::new(b"block-leaf");
    e.usize(commitment.ordinal);
    e.usize(commitment.start);
    e.usize(commitment.len);
    e.bytes(&commitment.rows_digest);
    e.bytes(&commitment.summary_digest);
    e.finish()
}

fn anchor_digest(snapshot_id: u64, block_count: usize, row_count: usize, tree: Digest) -> Digest {
    let mut e = Encoder::new(b"anchor");
    e.u64(snapshot_id);
    e.usize(block_count);
    e.usize(row_count);
    e.bytes(&tree);
    e.finish()
}

fn merkle_tree(leaves: &[Digest]) -> (Digest, Vec<Vec<Digest>>) {
    if leaves.is_empty() {
        return (hash(b"empty-tree", &[]), Vec::new());
    }
    let mut levels = vec![leaves.to_vec()];
    while levels.last().is_some_and(|level| level.len() > 1) {
        let level = levels.last().expect("nonempty levels");
        let mut parents = Vec::with_capacity(level.len().div_ceil(2));
        for pair in (0..level.len()).step_by(2) {
            parents.push(parent_digest(
                level[pair],
                level.get(pair + 1).copied().unwrap_or(level[pair]),
            ));
        }
        levels.push(parents);
    }
    let proofs = (0..leaves.len())
        .map(|leaf| {
            let mut index = leaf;
            levels[..levels.len() - 1]
                .iter()
                .map(|level| {
                    let sibling = if index % 2 == 0 {
                        level.get(index + 1).copied().unwrap_or(level[index])
                    } else {
                        level[index - 1]
                    };
                    index /= 2;
                    sibling
                })
                .collect()
        })
        .collect();
    (levels.last().expect("nonempty levels")[0], proofs)
}

fn verify_proof(
    leaf: Digest,
    mut index: usize,
    count: usize,
    proof: &[Digest],
) -> Result<Digest, CoverageError> {
    if count == 0 || index >= count || proof.len() != proof_depth(count) {
        return Err(CoverageError::InvalidReceipt);
    }
    let mut width = count;
    let mut node = leaf;
    for sibling in proof {
        if index % 2 == 0 {
            if index + 1 >= width && *sibling != node {
                return Err(CoverageError::InvalidReceipt);
            }
            node = parent_digest(node, *sibling);
        } else {
            node = parent_digest(*sibling, node);
        }
        index /= 2;
        width = width.div_ceil(2);
    }
    Ok(node)
}

fn proof_depth(mut count: usize) -> usize {
    let mut depth = 0;
    while count > 1 {
        count = count.div_ceil(2);
        depth += 1;
    }
    depth
}

fn parent_digest(left: Digest, right: Digest) -> Digest {
    let mut e = Encoder::new(b"merkle-node");
    e.bytes(&left);
    e.bytes(&right);
    e.finish()
}

fn hash(domain: &[u8], bytes: &[u8]) -> Digest {
    let mut hasher = Sha256::new();
    hasher.update(VERSION);
    hasher.update((domain.len() as u64).to_le_bytes());
    hasher.update(domain);
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
    hasher.finalize().into()
}

struct Encoder(Vec<u8>);

impl Encoder {
    fn new(domain: &[u8]) -> Self {
        let mut this = Self(Vec::new());
        this.bytes(VERSION);
        this.bytes(domain);
        this
    }

    fn tag(&mut self, tag: u8) {
        self.0.push(tag);
    }
    fn bool(&mut self, value: bool) {
        self.0.push(u8::from(value));
    }
    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn i64(&mut self, value: i64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn usize(&mut self, value: usize) {
        self.u64(value as u64);
    }
    fn bytes(&mut self, bytes: &[u8]) {
        self.u64(bytes.len() as u64);
        self.0.extend_from_slice(bytes);
    }
    fn string(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }
    fn finish(self) -> Digest {
        hash(b"encoded", &self.0)
    }
}
