# Fedora performance refinement, run 01

In progress under the separately committed [protocol](performance-refinement-fedora-protocol.md),
on `milestone/streaming-segment-output` containing `a6d4905`. The cloud isolation
screen remains completed finite evidence; it is not rerun or called qualification.
No installed defaults or durability/custody rules change.

## Host and rate correction

[Environment capture](data/performance-refinement-fedora-run-01/environment.json):
Fedora 44, i7-10750H, 6 physical cores / 12 SMT CPUs, affinity 0–11, 33,491,931,136
bytes RAM, 8 GiB unused compressed swap at capture, about 25.3 GB available RAM.
Every inspected ancestor has unlimited CPU/memory high/max (except the root,
which exposes no such files); process open-file limit 1,024. Repository storage is
LUKS-encrypted NVMe/Btrfs with about 494 GB free; `/tmp` is tmpfs. Rust/Cargo 1.99,
Python 3.14.7; Bun absent initially, native Bun 1.4.2 downloaded into owned `/tmp`
for documentation checks. The cloud had five affinity CPUs, four quota CPU
equivalents, shared 16 GiB memory and buffered cloud storage, Rust 1.98. This
workstation has background desktop processes and thermal/frequency variability;
it is neither a dedicated device benchmark nor the Debian deployment profile.

The unchanged retained diagnostic initially could not build offline (missing
local Cargo cache, exit 101 inside its subprocess); `cargo fetch --locked`
completed with exit 0. A new diagnostic invocation then exited **1** with all
three original disagreements reproduced: [output](data/performance-refinement-fedora-run-01/rate-reproduction.json).
The original audit inputs, source, receipts and expected outputs are unchanged.

The repair caches existing display sort keys once, uses structural map equality
for series identity, and breaks ambiguous display-key ties structurally. It
retains time/identity/sequence/index order. A typed pure-core adapter computes
integer deltas in i128 and compares mixed represented values without premature
integer rounding, then delegates time/reset/finite-rate decisions to the existing
kernel. Mixed subtraction keeps the independent oracle's floating arithmetic;
it does not invent precision absent from an OTLP double. No dependency was added
to the core and the Python oracles were not changed.

The [HTTP integration regression](../../../crates/fabric-server/tests/history.rs)
covers the three retained failures, multi-point colliding maps, mixed numeric
pairs/fractions/decreases, the full i64 range and 2,048 independent series. The
corrected implementation produced **2,060 rows**; its targeted Cargo test exited
0 through durable TLS delivery, scan/walk, journal, sealed Segment and restart,
with independent oracle grading. Restoring the exact original query source
made this same regression exit **101**, with 2,062 rows (invented cross-series
rates): [negative-control receipt](data/performance-refinement-fedora-run-01/rate-integration-negative-control.json)
and [failure](data/performance-refinement-fedora-run-01/rate-integration-negative-control.txt).
This is independent behavior evidence, not a claim about arbitrary counter inputs.

Initial fast profile exited 1: **19 checks passed, Clippy failed** on the previously
recorded Rust chunk-iterator lint. After replacing the two existing iterator
instances with typed `as_chunks`, the second profile also exited 1 solely on an
unnecessary borrow in the new test; that borrow was corrected. Both failed
profiles and their receipts are retained. The direct final Clippy command exited
0: [commands](data/performance-refinement-fedora-run-01/rate-final-check-commands.json).
Semantic-mutant outcomes and subsequent attribution/composites are pending.
These records do not claim a full fast-profile pass.

## Continuation status

The registered next measurements add native collection/OTLP, lifetime and
builder/query phase attribution, then independent journal granularity, worker
counts including three, selected interactions and sequential native consumer
composites. None is claimed complete by this correctness record. Default-sized
files and memory caps remain hypotheses; enterprise and QUIC remain secondary.
