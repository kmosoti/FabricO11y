# Coverage prototype interface (frozen before candidates)

The authoritative semantics and finite workload are in the [E1R protocol](../../docs/experiments/ablation/coverage-e1-protocol.md).
Implement `pub mod coverage` without changing S1 behavior. Add Eq/PartialEq to Query.
Use sha2 0.10.9 only in this research package. Every wire type below derives Clone,
Debug, PartialEq, Eq. The opaque SealedSnapshot need not implement these traits.

```rust
use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use fabric_o11y::Event;
use crate::Query;
pub type Digest = [u8; 32];
pub struct Summary {
    pub min_time: i64,
    pub max_time: i64,
    pub tenants: BTreeSet<u64>,
    pub tokens: BTreeSet<String>,
}
pub struct BlockCommitment {
    pub ordinal: usize,
    pub start: usize,
    pub len: usize,
    pub rows_digest: Digest,
    pub summary_digest: Digest,
}
pub struct Anchor {
    pub snapshot_id: u64,
    pub block_count: usize,
    pub row_count: usize,
    pub root: Digest,
}
pub enum Disposition { Scanned, Excluded(Summary), Unavailable }
pub struct BlockReceipt {
    pub commitment: BlockCommitment,
    pub proof: Vec<Digest>,
    pub disposition: Disposition,
}
pub struct Receipt {
    pub snapshot_id: u64,
    pub query: Query,
    pub blocks: Vec<BlockReceipt>,
}
pub enum CoverageStatus { Complete, Incomplete { unavailable: Vec<usize> } }
pub enum CoverageError {
    InvalidSummary, InvalidLayout, InvalidReceipt, WrongSnapshot, WrongQuery,
}
pub struct Answer { pub positions: Vec<usize>, pub receipt: Receipt }
pub fn summarize(events: &[Event]) -> Result<Summary, CoverageError>;
pub fn validate_summary(events: &[Event], summary: &Summary) -> Result<(), CoverageError>;
pub fn summary_digest(summary: &Summary) -> Digest;
pub fn rows_digest(events: &[Event]) -> Digest;
pub fn authenticate(snapshot_id: u64, row_count: usize, commitments: &[BlockCommitment])
    -> (Anchor, Vec<Vec<Digest>>);
pub fn verify(anchor: &Anchor, query: &Query, receipt: &Receipt)
    -> Result<CoverageStatus, CoverageError>;
pub struct SealedSnapshot { /* private rows, summaries, metadata */ }
impl SealedSnapshot {
    pub fn new(events: Vec<Event>, block_size: NonZeroUsize, snapshot_id: u64,
        supplied_summaries: Option<Vec<Summary>>) -> Result<Self, CoverageError>;
    pub fn anchor(&self) -> &Anchor;
    pub fn query(&self, query: &Query, available: &[bool]) -> Result<Answer, CoverageError>;
}
```

`authenticate` is a low-level commitment utility, NOT summary validation or an
independent trusted channel. It intentionally permits the test's faulty-builder
counterexample and malformed-layout fixtures. A caller must retain an anchor from
the trusted builder independently of the received receipt. Query output never
supplies the verifier's authority. Hashes use explicit domain separation, a fixed
version and unambiguous tagged/length-prefixed fields, with every Event field and
float bit preserved. This is an experimental encoding, not a disk format.

`new` partitions owned rows by block size. When supplied, the number of summaries
must equal the number of nonempty blocks and each is validated against its rows.
Otherwise derive and validate them. `summarize` and `validate_summary` reject empty
row slices. Empty snapshots accept zero summaries. Conservative superset summaries
are valid. Private snapshot fields prevent mutation after validation.

`query` requires one availability bit per block. It excludes using a summary
first (even if the raw block is unavailable); otherwise marks unavailable or scans
exactly. Positions refer to original input order. `verify` has no row access and
checks all metadata and dispositions as in the protocol. Wrong queries/snapshot
IDs, malformed proofs, arithmetic overflow or invalid layouts return an error,
never panic. Incomplete ordinals are ascending. Query comparisons are structural.
No claim is made that a Scanned marker proves the executor actually scanned.
