# Snapshot-bound residual interface

Registered before implementation. This extends the [coverage API](COVERAGE_API.md)
without changing E1's trust assumptions. New module: `pub mod resume`.

```rust
use crate::coverage::{Anchor, BlockCommitment, CoverageError, CoverageStatus, Digest, Receipt, SealedSnapshot};
use crate::Query;
pub const TOKENIZER_VERSION: u32 = 1;
pub const ORDER_VERSION: u32 = 1;
// All wire types derive Clone, Debug, Eq, PartialEq.
pub struct Binding {
    pub anchor: Anchor,
    pub query: Query,
    pub tokenizer_version: u32,
    pub order_version: u32,
}
pub struct MatchedRow {
    pub block: usize,
    pub offset: usize,
    pub digest: Digest,
}
pub struct Page {
    pub binding: Binding,
    pub receipt: Receipt,
    pub rows: Vec<MatchedRow>,
}
pub struct Residual {
    pub binding: Binding,
    pub blocks: Vec<BlockCommitment>,
}
pub enum ResumeError {
    Coverage(CoverageError), WrongBinding, InvalidResidual, InvalidRows, Conflict,
}
pub fn execute(snapshot: &SealedSnapshot, binding: &Binding, available: &[bool]) -> Result<Page, ResumeError>;
pub fn resume(snapshot: &SealedSnapshot, residual: &Residual, available: &[bool]) -> Result<Page, ResumeError>;
pub struct Accumulator { /* private binding, authenticated layout, per-block results */ }
impl Accumulator {
    pub fn new(binding: Binding, page: Page) -> Result<Self, ResumeError>;
    pub fn merge(&mut self, page: Page) -> Result<(), ResumeError>;
    pub fn residual(&self) -> Residual;
    pub fn rows(&self) -> Vec<MatchedRow>;
    pub fn positions(&self) -> Vec<usize>;
    pub fn status(&self) -> CoverageStatus;
}
```

The integration adds `SealedSnapshot::row_digest_at(position: usize) -> Option<Digest>`
using `rows_digest` on the actual single stored Event; no mutable row access is added.
This supports exact bit-sensitive retry identity. Binding includes the full anchor
(root, identity and counts), exact query, tokenizer and order versions. Unsupported
versions, changed binding or a different snapshot are errors before producing or
merging a page. Empty snapshots and inverted/empty queries remain well-defined.

`execute` obtains an E1 answer, produces matching physical coordinates and full-event
digests, and preserves its receipt. Availability has exactly one bit per block.
`resume` accepts a sorted unique subset of genuine commitments for that snapshot,
checks every commitment against the snapshot's authenticated metadata, and scans
only requested blocks permitted by availability. Other candidates are unavailable
in that page; safe summary exclusions remain valid. Removing a block from a resume
request cannot remove it from an existing accumulator's outstanding work.

`Accumulator::new` trusts the caller's expected Binding, verifies the page against
it, and records the entire authenticated layout. `merge` verifies before mutation;
failure leaves state unchanged. Every row is in range and belongs to a Scanned
block. Page rows are sorted by (block, offset), without duplicate coordinates.
The executor is trusted to report the entire exact match list for each Scanned
block; metadata verification does not prove execution. Excluded blocks resolve to
an empty list. Unavailable blocks contribute no rows and stay unresolved.

Resolve each block once. Repeated resolved results must equal the entire previous
per-block row list, including digests: reject changed, added or omitted rows rather
than choosing a winner. Identical retries are idempotent; later Unavailable entries
cannot undo previously resolved work. Safe empty Scanned and Excluded results may
agree. Merge order is immaterial for compatible pages. Distinct positions survive
even if their EventIds or full row digests are equal.

`residual` derives the still-unresolved commitments from the full stored layout;
it is never trusted as evidence of completion. `rows` and `positions` return original
arrival order. `status` is Complete only when every block is resolved, otherwise
Incomplete with ascending unresolved ordinals. Permanent corruption/unavailability
remains outstanding. This increment has no wire parser, disk checkpoint or guarantee
of eventual availability; those belong to the end-to-end publication/resume path.
