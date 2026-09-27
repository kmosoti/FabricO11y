# S2 durable snapshot and checkpoint boundary

Status: registered before implementation. This adds a `disk` module to the separate
research package and uses the [coverage](COVERAGE_API.md) and [resume](RESUME_API.md)
contracts. The application's FOL2 log remains the independently retained input.
This is an initial JSON block layout for measuring alternatives, not a format winner.

## Fixed API

```rust
use std::{io, num::NonZeroUsize, path::Path};
use fabric_o11y::Event;
use crate::{Query, coverage::{Anchor, Digest}, resume::{Binding, Page, Residual, Accumulator}};
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Publication {
    pub anchor: Anchor,
    pub block_rows: usize,
    pub codec_version: u32, // only version 1 is supported
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReadMetrics {
    pub metadata_bytes: u64,
    pub raw_bytes: u64,
    pub raw_files: usize,
    pub unavailable: Vec<usize>,
}
pub struct DiskAnswer { pub page: Page, pub reads: ReadMetrics }
pub struct DiskSnapshot { /* immutable metadata, paths and expected publication */ }
pub fn encode_events(events: &[Event]) -> io::Result<Vec<u8>>;
pub fn decode_events(bytes: &[u8]) -> io::Result<Vec<Event>>;
pub fn publish(events: Vec<Event>, block_rows: NonZeroUsize, snapshot_id: u64,
               final_dir: &Path) -> io::Result<Publication>;
pub fn save_publication(path: &Path, publication: &Publication) -> io::Result<()>;
pub fn load_publication(path: &Path) -> io::Result<Publication>;
impl DiskSnapshot {
    pub fn open(dir: &Path, expected: Publication) -> io::Result<Self>;
    pub fn query(&self, binding: &Binding, available: &[bool]) -> io::Result<DiskAnswer>;
    pub fn resume(&self, residual: &Residual, available: &[bool]) -> io::Result<DiskAnswer>;
}
// Returns the path of the immutable reconstructed manifest, never overwrites evidence.
pub fn rebuild_manifest(dir: &Path, expected: &Publication) -> io::Result<std::path::PathBuf>;
// Create a new immutable checkpoint, verify every page by Accumulator before writing.
// Caller retains the returned digest independently from the checkpoint contents.
pub fn save_checkpoint(path: &Path, binding: &Binding, pages: &[Page]) -> io::Result<Digest>;
pub fn load_checkpoint(path: &Path, expected_binding: &Binding,
                       expected_digest: Digest) -> io::Result<(Accumulator, Vec<Page>)>;
```

Error variants are ordinary `io::Error`; tests check failed operations and retained
state, not incidental message wording. Existing E1/E3 public structs may gain
Serde derives; **never deserialize Accumulator's private state directly**. Loading
pages must invoke its existing new/merge validation. Dependencies belong only to
the research package. Root Event types and FOL2 framing do not change.

## Files and publication

Version-1 event JSON preserves all identity/time values, exact string contents,
attribute order/duplicates, payload variants and every floating-point bit pattern.
Represent floating point as 16 hexadecimal digits for its raw 64 bits, not JSON numeric floats.
Use the envelope `{"version":1,"events":[...]}` and reject unknown fields,
unsupported versions, malformed tags, bad bit
encodings and missing required fields. `decode_events(encode_events(rows))` must
preserve `same_record_contents` and coverage digests even for NaN payloads and -0.
The per-event object has exactly `id`, `tenant`, `source`, `resource` (u64 JSON
integers), `event_time`, `observed_time` (i64 JSON integers), `attributes` (array),
and `payload`. Each attribute is `{ "key": <string>, "value": <scalar> }`.
A scalar is `{ "kind": <tag>, "value": <value> }`; tags are `bool`, `i64`, `u64`,
`string`, or `f64_bits`, whose value is a lowercase 16-digit hex string. A Log
payload is `{ "kind": "log", "body": <string> }`; a Gauge payload is
`{ "kind": "gauge", "name": <string>, "value_bits": <16-digit hex string>,
"unit": <string> }`. All these objects reject unknown or missing fields. Encode
hex in lowercase; decode accepts only the registered lowercase form.

The codec is deterministic for the same events. The maximum encoded file is 64 MiB;
reject larger input/output explicitly. Bounds limit parser/file allocation, not the
original caller-owned Vec. No lossy normalization is allowed.

`publish` requires a fresh final directory name with an already durable existing
parent. Create a private staging sibling, write and sync each `block-N.json` and
`manifest.json`, sync the staging directory, rename it to the fresh final name,
then sync the parent before returning Publication. Use an exclusive creation/lock
or equivalent that prevents two publishers from replacing the same name. The
manifest envelope uses `version: 1`, `block_rows`, `anchor` and `blocks` fields and carries the full validated commitments/summaries and
metadata proofs (or enough information to reconstruct proofs without reading rows).
Use the E1 trusted builder; the publisher always derives deterministic summaries.
No anchor may be advertised before all publication syncs succeed. Failure does not
return a successful publication; an orphan stage/final directory is not independent
authority. Preserve failed output for inspection; never overwrite a prior snapshot.

`save_publication` creates and syncs a fresh caller-owned file and its parent. The
caller keeps it independently of snapshot data, then supplies it to `open`. Loading
this trusted file checks its version and structural fields. Reading an anchor from
an untrusted snapshot manifest does not make it trusted. After any known storage
I/O error the caller must rebuild from the trusted FOL2/source on healthy storage;
these bytes cannot reveal an earlier failed sync.

## Reads, availability and placement

`open` validates manifest metadata, full contiguous block layout and root against
expected Publication without reading raw blocks. Prefer `manifest.json`; if absent
or invalid, a valid `manifest.rebuilt.json` may be used. With no valid manifest,
return an error, never empty Complete. Empty snapshots remain valid.

Block N is at `block-N.json` in the snapshot directory or its `cold/` subdirectory.
These are the only locations; do not interpret arbitrary paths from metadata. Cold
placement is a separate local directory on the same filesystem, not object storage.
Query tries those locations in order and may use a valid cold copy after a missing
or invalid hot copy. It reads only nonexcluded blocks requested by availability.
Before scanning a loaded block, check its decoded count and full Event digest against
its authenticated commitment. Missing, unreadable, malformed or mismatching raw
blocks become Unavailable with no rows contributed. Metadata failures remain errors.
A safe authenticated exclusion needs no raw reads, even if that block is missing.
Rows contain E3 coordinates/digests in arrival order; exact duplicate Events stay
separate. Binding, residual subset and atomic merge rules are unchanged.

ReadMetrics counts actual bytes successfully read from metadata/raw files and raw
file-open attempts, including failed/corrupt candidate copies, for that operation.
`metadata_bytes` includes the open-time selected/rejected manifest reads attributed
to that DiskSnapshot; each answer reports that acquisition cost explicitly rather
than hiding it. `raw_bytes` is userspace file bytes, **not physical device reads**.
No physical-I/O claim follows from page-cache hits. `unavailable` records ascending
raw candidate ordinals that could not be scanned, including false availability bits;
exclusions are not unavailable. This describes the returned page, not previously
accumulated progress; derive retry work from Accumulator::residual.
False availability bits do not open raw files. Reject a wrong availability length,
changed binding, unsupported tokenizer/order or invalid residual before raw reads.

`rebuild_manifest` reads every required block in ordinal order using the expected
block size/count, derives metadata via the trusted builder, and requires equality
with the original independent anchor. It writes/syncs a fresh
`manifest.rebuilt.json`, preserving an invalid original manifest. Missing or corrupt
rows prevent a successful rebuild. It never creates a new trusted root to excuse a
mismatch. Rebuilding already present output is an error. An index loss can cost a
full reread; record that cost rather than claiming free fallback.

## Checkpoint integrity

A checkpoint serializes `version: 1`, the expected `binding` and complete `pages`
history in an envelope. Validate that history through Accumulator before writing it.
Write/sync a fresh file and parent, and return SHA-256 of all written bytes only on
success. `load_checkpoint` first checks the externally retained digest, then version,
expected binding and every page through Accumulator. Empty page history is invalid.
Invalid input or digest/binding mismatch is an error without file mutation. A retry
writes a new checkpoint; earlier state survives failed writes. Do not infer progress
from a serialized residual list or deserialize internal resolved bits as authority.

The external digest is part of the caller's trusted state. A digest stored only
inside the same replaceable checkpoint would not protect against coherent changes.
This is integrity and restart validation under the trusted executor assumptions,
not proof that a malicious executor scanned rows. The final CLI must make the
publication and checkpoint trust boundaries explicit and reproducible.

## Rebuild ambiguity after metadata loss

Review clarification: query has authenticated per-block commitments and therefore
tries a valid cold copy after a digest-mismatching hot copy. Rebuild after complete
metadata loss has only the independently retained whole-snapshot root. If both
locations decode to the expected count but contain different rows, the current
rebuild tries hot first and fails when the reconstructed root differs; it does not
search every combination of alternatives. This is a fail-closed recoverability
limit, not a Complete answer. Restore from the independent source or remove the
identified bad hot copy before rebuilding. No root is replaced to hide a mismatch.
