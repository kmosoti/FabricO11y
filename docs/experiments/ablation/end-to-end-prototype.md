# End-to-end prototype completion contract

Status: complete for the scoped local research prototype. The user objective was to
complete testing and experimentation and develop an end-to-end prototype. This ledger
records the evidence for that local scope, including failed performance hypotheses.
Conditional hardware, external services and production features remain outside it.

## Required evidence

| Requirement | Completion evidence | Current state |
| --- | --- | --- |
| Exact baseline and conservative pruning | Independent positional oracle, fixed workloads, mutation detection and raw results | [S1 complete](storage-query-s1-run-01.md) |
| Account for query coverage | Trusted builder/anchor boundaries, missing/tampered block checks, explicit incomplete results | [E1R complete](coverage-e1-run-01.md) under its assumptions |
| Snapshot-bound resumable answers | Fixed E3R corpus, repeat/reorder/conflict/compaction cases, mutation kills, final closed answer equals own-snapshot full scan | [E3R complete](resume-e3-run-01.md), finite corpus and independent probes passed |
| Attribute current append cost | Fixed FOL2 behavior, encoding/write/sync breakdown and instrumentation-overhead control, replay and resource records | [S0 measured](../benchmarks/append-attribution-s0.md), with 11.017% perturbation flag |
| Investigate one-sync seals separately | E2R sector/error model, external failed-I/O authority, counterexamples; only surviving designs enter equal-batching cost comparison | [E2R model checked and reviewed](seal-e2-run-01.md); [cost comparison measured](../benchmarks/group-seal-cost-run-01.md), with mixed gates; no application durability change |
| Quantify receipt/resume overhead | Preregistered equal-workload comparison of build/query/verify CPU and wall time, bytes, RSS and freshness boundaries | [Measured](../benchmarks/research-costs-run-01.md), all query families retained |
| Durable immutable snapshots and cold placement | S2 publication contract, full round trip, restart and interrupted-publication/corruption/rebuild checks, physical/logical read accounting; late arrivals and cold-directory behavior | [S2 checked and integrated](durable-snapshot-s2-run-01.md); [lifecycle passed](local-prototype-run-01.md) |
| Columnar and selective-acceleration experiments | S3/S4 matched semantics, all payload/scalar edge cases, serialization/float-bit policy, projection/index loss and rebuild, measured storage/build/query costs | [Correctness checked](columnar-selective-s3-s4-run-01.md), [costs measured](../benchmarks/research-costs-run-01.md); no production migration |
| Collection path | S5 specified receiver/adapter, explicit input mapping/rejections, bounded ownership/backpressure and retained-event equality; sidecar costs measured separately | [Offline adapter and process tests passed](local-prototype-run-01.md); [sidecar benefit gate failed](../benchmarks/research-costs-run-01.md) |
| Runnable end-to-end prototype | Reproducible commands accept input, commit evidence, publish snapshot, query partial availability, persist/resume against the same snapshot, reject mismatches, and independently verify final results after process restart | [21-command integrated lifecycle passed](local-prototype-run-01.md), including new caller input and independent verification |
| Documentation and review | Current architecture/diagrams/learning path, source-pinned artifacts, actual check exits, cross-family adversarial review and final requirement-by-requirement audit | [Current architecture](../../architecture/research-prototype.md), [review and final audit](../benchmarks/research-costs-run-01.md), synchronized learning path and checked documentation |

The prototype can remain a separate executable/package while its contracts are
experimental. It must accept new inputs and expose the complete lifecycle, not
merely replay fixed test answers. The existing application log is the durable
baseline until a separately justified change is accepted.

The [original agenda](observability-storage-research.md) keeps external backends and
NIC/GPU/DPU experiments conditional on equivalent semantics and facilities. Record
the available facilities and explicit limits; do not substitute simulator results
for hardware measurements. Distributed clustering and production deployment are
not implied by a local end-to-end prototype. Future hypotheses in the source
survey remain visible rather than silently being described as implemented.

## Completion audit

The implementation and reviews are source-pinned in each result record. The full
E3 corpus completed with exit 0; its Serde adaptation is explicitly distinguished
from a second full-corpus run. Integrated quick research tests, layout tests, real
read-error probes, root all-features tests and the separate-process demo passed.
Cost trials completed with all child commands successful, and an independent auditor
recomputed the gates from raw observations. Cross-family reviews exercised probes
and preserved rejected candidates and repairs. The final documentation/check record
is retained with the [cost evidence](../benchmarks/data/research-costs-run-01/).

The [experiment discipline](../../experiments/README.md) applies throughout. Negative
sidecar and uncompressed-layout gates are completed investigations, not missing work.
The [facilities inventory](data/facilities-2026-09-27/inventory.json) explains why no
external-backend, NIC, GPU or DPU result is claimed. Network OTLP, clustering, adaptive
indexing and event-time partition redesign remain proposals, not hidden prerequisites
or implemented features of this local prototype.
