# S2: immutable disk snapshots and persistent query progress

Status: registered before implementation. The [fixed API](../../../tools/storage-probe/DISK_API.md)
defines publication, exact event JSON, independent anchors, cold-directory placement,
raw integrity, read counters, rebuild and externally bound checkpoints. This is the
next persistence increment in the [completion contract](end-to-end-prototype.md),
using the E1/E3 trust assumptions. It does not replace FOL2 or select JSON over Parquet.

## Contract and mechanism

Rust ownership transfers a Vec of events to the publisher. The immutable published
artifact has its own retained anchor; later arrivals require a new snapshot identity.
A directory rename becomes the visibility step after file/staging syncs, followed by
a parent sync before success. Model ordinary interruption at each publication boundary
with prebuilt partial fixtures or explicit fault injection; process restart checks
are not physical power-loss evidence. Failed publication cannot advertise a new anchor.

Queries validate metadata before pruning and block contents before producing rows.
Unknown raw state remains unavailable. A missing index is an error until a rebuild
matches the old root. Checkpoints store page history and are loaded only against a
caller-retained digest and binding, replaying the checked accumulator transitions.

## Fixed correctness cases, before implementation

An independent oracle author must implement these API probes before candidate code:

- Empty, singleton and uneven blocks; duplicate IDs and identical Events; all five
  Scalar variants, both payloads, duplicate attributes, non-ASCII strings, integer
  extremes, signed zeros, infinities and distinct NaN bits. Compare canonical FOL2
  contents and coverage digests, not floating-point PartialEq.
- Publish/reopen/query results equal an independent inclusive-time/tenant/whitespace
  token scan in original arrival order. Include out-of-order and late event times.
- Missing/corrupt raw blocks stay unresolved for candidate queries; exclusions still
  require zero raw reads. Check cold placement and a valid cold copy after corrupt hot.
- Tampered manifest/root/count/version fail; incomplete staging, partial blocks and
  unpublished directories cannot yield a successful query. Existing final snapshot
  names and checkpoint files are never replaced. Concurrent publishers to one name
  cannot both succeed or alter the winner. Check real subprocess exit outcomes.
- Remove/modify index metadata, rebuild from intact raw rows to the same anchor, then
  require exact answers; refuse rebuild after raw corruption or missing blocks.
- Save partial query pages, close the process/state, load using the retained digest,
  resume complementary availability and require exact closure. Reordered identical
  retries remain safe; conflicting page rows, different query/version/snapshot and
  changed checkpoint bytes fail without changing files or accumulated state.
- Validate malformed/oversized codec files and incorrect availability/residual shapes;
  reject before reading raw rows where the API requires it. Inject checker defects
  (accept changed anchor, declare missing raw Complete, skip checkpoint digest) and
  require real failing exits.

No timing threshold is assigned to this correctness cell. Record source hashes,
commands/exits, mutation counterexamples and actual userspace read counts. Do not
claim reduced device I/O from fewer bytes decoded. The later layout/cost comparison
must register fixed datasets, both acquisition and steady query cost, output equality,
storage amplification and kernel I/O counters before selecting an acceleration.

## End-to-end follow-up

The executable will ingest explicit new input into FOL2, publish a snapshot, issue a
partial query, save and reload a checkpoint in another process, resume it against the
same anchor, and independently verify complete output. Failed or rejected input and
late arrivals remain observable. This document registers the persistence library
first; a green library test alone does not fulfill that CLI/collection requirement.
