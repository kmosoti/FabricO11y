# Collection proofs and resource checks

Run the source-linked checks and executable experiments with the pinned repository
toolchain:

```sh
CARGO_BUILD_JOBS=2 cargo xtask prove all
CARGO_BUILD_JOBS=2 cargo test -p xtask --test collection_programs -- --ignored --nocapture
CARGO_BUILD_JOBS=2 cargo run --locked -p xtask --bin collection-resources
CARGO_BUILD_JOBS=2 cargo run --locked -p xtask --bin collection-faults
```

The acceptance tier includes both programs, all proof paths and checker controls.
Each program prints JSON and exits unsuccessfully on a violated assertion. The
resource program starts the production daemon code in a private child process; an
optional absolute Spindle executable path exercises a separately built CLI. The
fault program runs the production custody store in an owned child process. They
use temporary directories and kill/reap their own children on ordinary failure.
Choose `TMPDIR` on the intended filesystem when measuring durable I/O: a memory
filesystem does not represent physical storage costs. Build time is outside the
reported measurements. Debug/release, hardware, filesystem and cache state must
accompany any comparison; these small runs do not rank implementations.

## What is proved

Verus checks the executable `admission-kernel/src/delivery.rs` functions called by
the SDK: a next sequence exists exactly when its state is valid, not exhausted and
has capacity; successful admission preserves the outstanding-record bound; only
the next admitted sequence can advance confirmation; and flush coverage is the
captured target inequality. Arithmetic holds over all machine integer inputs.
Invalid states and exhaustion are refused. A deliberately modified function that
accepts skipped receipts must fail its Verus postcondition.

Lean separately checks the window induction, impossibility of admitting another
record when full, monotonic coverage of a fixed flush target, byte-storage bounds,
and a pure transaction model's duplicate, conflict, abort, atomic-publication and
receipt-retention properties. Named theorem axiom audits permit only Lean's standard
`propext`, `Classical.choice` and `Quot.sound`; unfinished proofs are errors.

The transaction model assumes atomic durable publication. It does **not** verify
SQLite, filesystem synchronization, power-loss behavior, the mutex/worker adapter,
JSON parsing, Unix transport or source collectors. The existing Aeneas proofs remain
about the original admission kernel: extracting the new functions does not give
them additional Aeneas theorem coverage. Verus establishes their executable
arithmetic properties. Runtime tests connect the SDK and store to the stated
invariants but are not a formal refinement proof of the entire application.

## Memory bounds and their limits

With `N` SDK slots and `B` bytes per slot, preallocated payload storage is exactly
`N × B` bytes. Defaults reserve 4 MiB; the largest permitted configuration reserves
256 MiB. This excludes buffer descriptors, queue metadata, producer inputs, the
worker stack, transport/reply allocations and allocator overhead. The independent
Rust program checks the configured bound through outage saturation and recovery,
including concurrent producers. A full-width fixture retains exactly `N × B`
encoded bytes, attaining the lower bound **for disjoint, uncompressed byte storage**.
Shorter records leave measurable unused capacity. Compression, shared storage and
other admission contracts are outside that cost model; total RSS is not proven
minimal or equal to the payload bound.

The collector program holds seven incomplete maximum-length frames and checks
observed live connection count and allocated connection-buffer capacity against
configured bounds. RSS snapshots are reported separately for the harness/SDK and
collector; they are neither peak measurements nor a universal process memory cap.
Journal subprocess memory, SQLite allocations, host file contents and the allocator
remain additional resource domains. Configured input/count limits constrain their
work, but the present proofs do not establish a whole-process RSS bound.

## Failure evidence

The fault program stops before submission, after commit before acknowledgement,
and at fixed time offsets during repeated commits. After each kill it verifies
acknowledged records, all recovered payloads, the atomic checkpoint, duplicate IDs,
conflict rejection and exact final readback after retry and another reopen. Timed
kills do not guarantee every SQLite instruction is interrupted. These are process
crashes, not physical power-loss tests. Existing tests additionally inject SQLite
checkpoint failures and exercise storage quotas, malformed input, lost socket
acknowledgements, file rotation and truncation.
