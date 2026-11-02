# Compact observations and Fabric custody

This repository contains an experimental representation evaluator and distributed
custody model. The production SDK wire protocol remains v2 JSON. JSON decoding uses
`float_roundtrip` so finite metric values retain their exact f64 bits; a regression
test covers a value previously changed by one bit. No production remote forwarding,
server, automatic eviction, partitioning or replication is enabled by this work.

## Custody boundaries

A **Spindle Seal** means durable local custody. A **Fabric Seal** means durable server
custody. SDK flush continues to mean local custody; forwarding must track a separate
server-confirmed frontier. These names are receipt terminology, not signatures.
Remote keys must include persistent Spindle origin, producer, generation and sequence.
A future **Fabric Weave** partitions this identity space; placement must not alter
identity or receipt meaning. Partitioning does not itself provide replicated durability.

The Lean model proves that local custody need not imply remote custody, duplicates
are idempotent, conflicts cannot overwrite, different identities remain isolated,
committed remote contents survive arbitrary modeled drop/restart/delivery traces,
acknowledgement loss can hide a durable result, and confirmation cannot skip holes.
Reclamation requires remote custody. Progress is conditional on a finite backlog,
successful fair retries and available capacity. Atomic stable storage, correct codec
refinement and the environment are assumptions. This is not a proof of SQLite,
network authentication, consensus, power-loss resilience or all implementation code.

## Representations and experiment boundaries

The frozen evaluator compares JSON, schema-coded binary rows and dictionary columns.
Codes are explicit experimental schema constants, never Rust enum layout. Integers
retain signedness/width and f64 values retain bits. Dictionaries are local to a batch,
bounded to 1,024 entries and 64 KiB, and overflow uses inline strings. Messages stay
inline. Batches permit 32 records and 256 KiB canonical JSON; encoded frames permit
1 MiB. Decoder counts, nesting, expanded strings and nodes are independently bounded.
Invalid references, schema/code changes, trailing input and excessive expansion fail.
These experimental limits do not replace production limits.

Separate sender, Spindle and receiver processes communicate through local pipes.
The sender is a codec harness, not the production SDK queue. The existing SDK's
4 MiB default payload reservation is reported as a reference, not allocated or
measured by this harness. Changing encoding alone does not change that reservation.
Compatibility custody converts to the existing JSON Store. Native custody validates
once and atomically stores the original batch plus its identity index. Compare codecs
within each custody mode; cross-mode differences also include batching and indexing.

The reference binary encoders currently stage through `serde_json::Value` and reuse
JSON-based canonical size validation. Their CPU/allocation costs are measured, not
an inherent lower bound for binary codecs. Both binary and JSON must satisfy the
same frozen losslessness and validation tests before performance comparisons.
Native full readback decodes each stored batch once; sequence projection reads the
index without full materialization. Compatibility projection uses the existing
full-read API. That comparison exposes an API/storage-layout opportunity, not a
codec-only speedup. Reopen measurements include verified readback.

## Running and interpreting results

```sh
CARGO_BUILD_JOBS=2 cargo xtask prove distributed
CARGO_BUILD_JOBS=2 cargo test -p xtask --test representation_contract --test representation_adversarial
CARGO_BUILD_JOBS=2 cargo test -p xtask --test distributed_programs -- --ignored
CARGO_BUILD_JOBS=2 cargo build --release -p xtask --bins
cargo run --release --locked -p xtask --bin representation-evaluator -- run development
cargo run --release --locked -p xtask --bin representation-evaluator -- run held-out
cargo run --release --locked -p xtask --bin distributed-faults
```

Capture JSONL outputs outside tracked source. Use one warmup and five repetitions
per case; development seed 17 and held-out seed 991 are fixed before evaluation.
Only open held-out measurements after development qualification and freeze the
candidate implementations before either run. Set `TMPDIR` to the intended filesystem;
tmpfs is not evidence of physical disk latency. Record the source, contract and fixture
hashes, compiler, hardware, filesystem and uncontrolled shared-host/cache conditions.

Build `representation-evaluator` separately with `--features allocation-metrics`
and a separate Cargo target directory, then repeat the cases. This enables a small
System allocator wrapper only in that binary and its children. Timing builds have
no allocator instrumentation. Rust allocation totals and peak live allocations cover
the whole process including fixtures/readback/reporting; native SQLite allocations
are excluded. `/proc` peak RSS includes native allocations and is reported per role.
Peak RSS values must not be summed and described as a simultaneous aggregate peak.

`representation-report INPUT.jsonl ...` requires five measured samples per group,
uses the matching JSON baseline, reports independent medians and ratios, and suppresses
instrumented timing comparisons. It also reports a one-hop CPU/transfer break-even
bandwidth where both added CPU and saved bytes are positive; this serial cost model
is not a throughput or energy prediction. Setup and shutdown are outside ingest timing;
encoding, validation, durable commit and receipt exchange are inside it. Readback,
projection and recovery are measured after seals. Input batches are preformed.

Remote estimates cover 10/100/1,000 Mbps and 1/20/100 ms RTT. Serial estimates add
one remote transfer and RTT to measured local completion. Pipeline bounds use the
largest observed stage service cost, transfer time and RTT divided by a 32-batch
window. They exclude congestion, TLS, real network stacks and concurrent-disk effects;
the extra in-flight payload capacity must be budgeted before implementing that window.
A separate constant-arrival model exposes first-record and mean batch-fill delay
at 100/1,000/10,000 records/s. These are models, not actual two-host measurements.

## Decisions for the next Fabric-server milestone

Keep canonical identity/conflict semantics independent of codec and shard placement.
Persist dictionary context with the batch before issuing its Fabric Seal; never make
recovery depend on a sender's volatile interning table. Bound validation and expanded
content before indexing or materialization, and account for decoder memory separately
from socket and custody buffers. No format earns acceptance by omitting validation.

Expose projection/index APIs separately from full readback. Preserve an immutable
validated-frame boundary so the receiver can persist input without redundant encoding.
Use the evaluator's per-role costs to decide whether a compact format reduces total
work or merely moves it to the server. Keep JSON supported until a format and explicit
migration contract are selected. The present comparison deliberately chooses no
production winner and makes no global minimum-memory claim.
