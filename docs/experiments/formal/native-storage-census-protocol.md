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

The classification baseline subsequently ran at clean `2ab563b`: all three
totals matched the declared fixture, original source remained pinned and owned
scratch was removed. Its report is
`results/native-storage-census-reference-01/baseline-report.json`. This observation
admits the native implementation experiment, not caller adoption.

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

Use a depth-first stack of owned directory handles and iterators. Open child
entries relative to the held parent with Linux O_PATH/O_NOFOLLOW; inspect the
opened object rather than following a raced pathname. A directory iterator may
be opened through the held handle's `/proc/self/fd` alias. Retain its anchor until
that frame is finished; report errors using the original logical path for retry
classification. This requires O(depth) live directory handles and fails closed
on descriptor exhaustion. It is not an atomic snapshot or protection against a
privileged adversary. The read-only target lookup used solely to classify a
symlink as directory/non-directory preserves Python's error-to-nondirectory
behavior; errors enumerating or inspecting the actual census entries remain
fatal except the precisely admitted cleanup ENOENT.

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

## Prospective candidate comparison and reset mutant

After the six focused controls completed at `9e3b6e0`, extend the Rust experiment
driver with an explicit archived native executable and mandatory SHA-256 pin.
Keep baseline mode and its original records. Invoke the prebuilt CLI with
`FABRIC_STORAGE_ROOT` set to the canonical owned fixture, under the original
outer launcher. Never time Cargo builds. Require all six native output fields
to agree with the unchanged Python reference, including UID/GID and attempt 1;
also require the independently declared totals from the classification fixture.

For a registered metadata-cost workload, create ten directories containing 2,000
regular 64-byte files each: 20,000 paths and 1,280,000 logical/regular bytes.
Keep output files outside this immutable fixture. Use one warmup per program and
seven alternating-order pairs. Measure subprocess wall time and GNU time CPU/RSS;
parse and compare complete output outside the clock. Record hardware, filesystem,
warm-cache conditions and source/reference/executable pins before and after.
Report only this finite corpus; no whole-pipeline or universal speedup claim.
Mismatch or identity failure prevents performance adoption and retains fixtures.

In a separate clean mutant worktree, change `scan_once` to receive its counter by
mutable reference and move its initialization before the retry loop. Keep all
other traversal and retry conditions intact. This deliberately retains the
failed scan's partial counters. Run the six focused controls: the real-second-
file-unlink reset assertion must fail (36 bytes instead of 32 after restoration),
while the other five controls retain their outcomes. Expect test process 101,
not a compile/setup error. Preserve source, logs, stop and failure fixture; never
merge this defect. This checks one concrete composite failure, not every defect.

## Source/cache join correction before continuation

After the mutant ran, the production-source rerun at `102bdfc` executed a cached
mutant test binary from the shared target. The source was unchanged; Cargo did
not compile, the embedded path named `storage-census-mutant`, and the same named
36-versus-32 assertion failed. Preserve that actual 101 as an invalid source/
artifact join, not a production regression. The retained test ELF SHA-256 is
`de251eded7ac465a50eb63b56d0371a8e886abf01c9bb81b73e3ff2534bdd9c8`;
`results/storage-census-comparison-01/actual-cache-failure-01.json` records origin.

Before continuation, give production and mutant variants distinct, explicit
DATA build targets inside the original resource launcher command, using
`env CARGO_TARGET_DIR=ABS_DATA_VARIANT_TARGET`. The launcher itself and all caps
remain unchanged; an outer environment setting is insufficient because the
launcher assigns its default target. Keep download caches shared. Pin each
target to its worktree/source and inspect compiler activity plus resulting
artifact identity. Rerun the affected production controls in a new empty target;
only then build/archive the native executable and comparison driver together.
Do not erase the offending cache or claim the failed run passed. This correction
does not by itself invalidate unrelated historical artifacts or refreeze product
packages; those retain their own source/provenance and actual acceptance bounds.
