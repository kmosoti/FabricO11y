//! Conservative block pruning for immutable, replayed research snapshots.
//! See the S1 protocol; this is not an application query or persistence API.
use std::num::NonZeroUsize;

use fabric_o11y::{Event, Payload};

const BLOOM_WORDS: usize = 32;
const BLOOM_BITS: u64 = (BLOOM_WORDS * 64) as u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Scan,
    Pruned,
}

#[derive(Clone, Debug)]
pub struct Query {
    pub start_ns: i64,
    pub end_ns: i64,
    pub tenant: Option<u64>,
    pub token: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryResult {
    pub positions: Vec<usize>,
    pub scanned_events: usize,
    pub skipped_blocks: usize,
}

#[derive(Clone)]
struct Bloom([u64; BLOOM_WORDS]);

impl Bloom {
    fn new() -> Self {
        Self([0; BLOOM_WORDS])
    }

    fn insert(&mut self, bytes: &[u8], domain: u8) {
        let (first, step) = probes(bytes, domain);
        for probe in 0..3 {
            let bit = first.wrapping_add(step.wrapping_mul(probe)) % BLOOM_BITS;
            self.0[(bit / 64) as usize] |= 1_u64 << (bit % 64);
        }
    }

    fn might_contain(&self, bytes: &[u8], domain: u8) -> bool {
        let (first, step) = probes(bytes, domain);
        (0..3).all(|probe| {
            let bit = first.wrapping_add(step.wrapping_mul(probe)) % BLOOM_BITS;
            self.0[(bit / 64) as usize] & (1_u64 << (bit % 64)) != 0
        })
    }
}

// Stable, domain-separated hashing: the filter's behavior does not depend on
// process-randomized standard-library hash seeds.
fn probes(bytes: &[u8], domain: u8) -> (u64, u64) {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64 ^ u64::from(domain);
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^= hash >> 27;
    hash = hash.wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^= hash >> 31;

    let mut step = hash.rotate_left(29) ^ 0x9e37_79b9_7f4a_7c15;
    step |= 1;
    (hash, step)
}

struct BlockSummary {
    min_time: i64,
    max_time: i64,
    tenants: Bloom,
    tokens: Bloom,
}

impl BlockSummary {
    fn from_events(events: &[Event]) -> Self {
        let first_time = events[0].event_time.0;
        let mut summary = Self {
            min_time: first_time,
            max_time: first_time,
            tenants: Bloom::new(),
            tokens: Bloom::new(),
        };
        for event in events {
            let time = event.event_time.0;
            summary.min_time = summary.min_time.min(time);
            summary.max_time = summary.max_time.max(time);
            summary.tenants.insert(&event.tenant.0.to_le_bytes(), 0x54);
            if let Payload::Log { body } = &event.payload {
                for token in body.split_whitespace() {
                    summary.tokens.insert(token.as_bytes(), 0x42);
                }
            }
        }
        summary
    }

    fn excludes(&self, query: &Query) -> bool {
        if query.start_ns > query.end_ns
            || self.max_time < query.start_ns
            || self.min_time > query.end_ns
        {
            return true;
        }
        if let Some(tenant) = query.tenant {
            if !self.tenants.might_contain(&tenant.to_le_bytes(), 0x54) {
                return true;
            }
        }
        if let Some(token) = &query.token {
            if !self.tokens.might_contain(token.as_bytes(), 0x42) {
                return true;
            }
        }
        false
    }
}

pub struct Snapshot {
    events: Vec<Event>,
    block_size: NonZeroUsize,
    summaries: Option<Vec<BlockSummary>>,
}

impl Snapshot {
    /// Own the rows and derive summaries from exactly these immutable values.
    pub fn new(events: Vec<Event>, block_size: NonZeroUsize) -> Self {
        let summaries = Some(
            events
                .chunks(block_size.get())
                .map(BlockSummary::from_events)
                .collect(),
        );
        Self {
            events,
            block_size,
            summaries,
        }
    }

    /// Return exact matching row positions, preserving input order and duplicates.
    pub fn query(&self, query: &Query, mode: Mode) -> QueryResult {
        let mut result = QueryResult {
            positions: Vec::new(),
            scanned_events: 0,
            skipped_blocks: 0,
        };

        match (mode, self.summaries.as_deref()) {
            (Mode::Pruned, Some(summaries)) => {
                for (block_index, block) in self.events.chunks(self.block_size.get()).enumerate() {
                    if summaries[block_index].excludes(query) {
                        result.skipped_blocks += 1;
                        continue;
                    }
                    self.scan_block(block_index, block, query, &mut result);
                }
            }
            _ => {
                for (block_index, block) in self.events.chunks(self.block_size.get()).enumerate() {
                    self.scan_block(block_index, block, query, &mut result);
                }
            }
        }
        result
    }

    fn scan_block(
        &self,
        block_index: usize,
        block: &[Event],
        query: &Query,
        result: &mut QueryResult,
    ) {
        let start_position = block_index * self.block_size.get();
        for (offset, event) in block.iter().enumerate() {
            result.scanned_events += 1;
            let time = event.event_time.0;
            let in_time = query.start_ns <= time && time <= query.end_ns;
            let in_tenant = query.tenant.is_none_or(|tenant| tenant == event.tenant.0);
            let has_token = match (&query.token, &event.payload) {
                (None, _) => true,
                (Some(token), Payload::Log { body }) => {
                    body.split_whitespace().any(|word| word == token)
                }
                (Some(_), Payload::Gauge { .. }) => false,
            };
            if in_time && in_tenant && has_token {
                result.positions.push(start_position + offset);
            }
        }
    }

    /// Drop all optional summaries. Subsequent queries must scan every row.
    pub fn disable_summaries(&mut self) {
        self.summaries = None;
    }

    /// Logical bounds and bitset bytes; excludes allocation/container overhead.
    pub fn summary_bytes(&self) -> usize {
        self.summaries
            .as_ref()
            .map_or(0, |summaries| summaries.len() * 528)
    }
}
