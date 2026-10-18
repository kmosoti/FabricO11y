# Native storage census migration

Register before new baseline experiments or implementation. Alpha acceptance
remains paused. This ports a read-only admission helper, not a product store or
acceptance grader. The existing
[cleanup-race protocol](installation-census-race-protocol.md) records actual
owner file-unlink and exact-root deletion failures against the original helper.
Preserve those failures and all independent Python product oracles.

## Contract and unresolved observations

The pinned helper is DATA-relative
`results/tls-dependency-update/census-owned-retry.py`, SHA-256
`d044822178a2d64305ea496d9df324021ff2514b6185975dece133caa45137a7`.
It traverses the complete root without descending through directory symlinks.
It sums logical `lstat` sizes of file entries and directory symlinks, counts file
entries only, and separately sums regular-file bytes. Directory/root sizes are
excluded. Hard links count per pathname and sparse files count logical length;
inode deduplication or allocated-block accounting would change the contract.

The owner deletion RED report is
`results/owner-census-race-01/baseline-observed-report-01.json`, SHA-256
`7568bc9ed7ce4e08684f08d82019a57c2dec10c929133f8e0ecaf9424792e205`.
It exercises real deletion, not a simulated successful scan. Its independent
size helper is only valid for its no-symlink fixture; do not silently reuse it
as an oracle for directory-symlink classification.

Before native implementation, use a small Rust experiment driver to invoke the
unchanged pinned Python source with exactly one documented literal-root
substitution into owned DATA scratch. Build a compact independently sized fixture
with regular, empty, sparse and hard-linked files, file/directory/dangling
symlinks, an outside-root symlink target and a FIFO. Record a manifest and expected
totals derived from fixture sizes, not the candidate traversal. Compare all three
totals and ensure outside contents are not visited. Preserve transformed-source
hash, argv, raw output, actual exit and launcher stop. This baseline establishes
classification semantics; it is not a benchmark or current acceptance result.

## Native primitives and controls

Use checked `u64` counters, one complete traversal per attempt, explicit
path/errno errors and an iterative directory stack. Summation does not require
sorting. Reading all entries has an Omega(N) entry-inspection lower bound; make
no global optimality claim. Treat errors produced while advancing a directory
iterator as errors, not just errors constructing it. Never follow a directory
symlink; explicitly preserve the reference's directory-symlink classification.
No GPU, new dependency, generic filesystem abstraction or core change is needed.

Retry only ENOENT under the existing runner-controls category or the one exact
scratch root derived by the caller from pinned run/admission identities. Reject
siblings, relative/ambiguous paths and broad prefixes. Owner and installation
callers must retain their original binding checks; a CLI path alone cannot attest
ownership. Each retry discards every partial counter and starts at the full root.
At most ten attempts and nine 250 ms delays; final exhaustion, EACCES, EIO and
unrecognized missing paths are fatal. Checked arithmetic failure is fatal.

Before adoption, use a private deterministic test seam after enumeration and
before metadata/descent to cause real unlink and root removal. Require real
ENOENT, an allowed restart and final totals equal to independently calculated
remaining fixtures. Test unknown and prefix-sibling missing paths, permissions,
I/O, exhaustion, symlink classification and overflow separately; label injected
errors. A representative mutant that keeps partial counters must be detected.
Keep failed fixtures and distinguish checker rejection from startup failure.

## Execution and adoption boundary

All builds, controls and benchmarks use the required launcher, DATA scratch,
original resource caps, complete reservation and the sole heavy-job scheduler.
The production invocation must use a pinned prebuilt native executable and the
same established user namespace as the original read-only admission inspection;
never invoke an uncapped Cargo build during admission. Preserve UID/GID,
attempt count, complete totals, the 95 GB stop and full reservation guards.
No system-storage fallback and no partial scan admitted as complete.

Archive commands, hashes, exits, stops, resources and cleanup. Publish measured
results in the research wiki. Policy/binding changes, implementation and caller
adoption are separate commits. Full native acceptance still requires a newly
admitted actual run; source controls cannot repair an interrupted result.
