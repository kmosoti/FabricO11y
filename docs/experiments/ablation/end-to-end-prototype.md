# End-to-end prototype completion contract

Status: active. User objective: complete testing and experimentation and develop an
end-to-end prototype. The prior goal turn made progress: E1R was implemented,
tested, independently reviewed and committed as `a82111c`. It did not complete
this objective. This record retains outstanding work across goal continuations.

## Required evidence

| Requirement | Completion evidence | Current state |
| --- | --- | --- |
| Exact baseline and conservative pruning | Independent positional oracle, fixed workloads, mutation detection and raw results | [S1 complete](storage-query-s1-run-01.md) |
| Account for query coverage | Trusted builder/anchor boundaries, missing/tampered block checks, explicit incomplete results | [E1R complete](coverage-e1-run-01.md) under its assumptions |
| Snapshot-bound resumable answers | Fixed E3R corpus, repeat/reorder/conflict/compaction cases, mutation kills, final closed answer equals own-snapshot full scan | Pending |
| Attribute current append cost | Fixed FOL2 behavior, encoding/write/sync breakdown and instrumentation-overhead control, replay and resource records | [S0 measured](../benchmarks/append-attribution-s0.md), with 11.017% perturbation flag |
| Investigate one-sync seals separately | E2R sector/error model, external failed-I/O authority, counterexamples; only surviving designs enter equal-batching cost comparison | [E2R model checked and reviewed](seal-e2-run-01.md); [cost comparison measured](../benchmarks/group-seal-cost-run-01.md), with mixed gates; no application durability change |
| Quantify receipt/resume overhead | Preregistered equal-workload comparison of build/query/verify CPU and wall time, bytes, RSS and freshness boundaries | Pending |
| Durable immutable snapshots and cold placement | S2 publication contract, full round trip, restart and interrupted-publication/corruption/rebuild checks, physical/logical read accounting; late arrivals and cold-directory behavior | [S2 contract registered](durable-snapshot-s2-protocol.md); independent oracle frozen, implementation candidates in progress |
| Columnar and selective-acceleration experiments | S3/S4 matched semantics, all payload/scalar edge cases, serialization/float-bit policy, projection/index loss and rebuild, measured storage/build/query costs | Pending; no format winner selected |
| Collection path | S5 specified receiver/adapter, explicit input mapping/rejections, bounded ownership/backpressure and retained-event equality; sidecar costs measured separately | Pending |
| Runnable end-to-end prototype | Reproducible commands accept input, commit evidence, publish snapshot, query partial availability, persist/resume against the same snapshot, reject mismatches, and independently verify final results after process restart | Pending |
| Documentation and review | Current architecture/diagrams/learning path, source-pinned artifacts, actual check exits, cross-family adversarial review and final requirement-by-requirement audit | Ongoing |

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

## Execution order

Complete E3R next. In separable work, attribute append cost and test the E2R crash
claim. Then measure the novel metadata path, define durable publication before S2,
and build the runnable lifecycle incrementally. Compare physical layouts and
selective acceleration with fixed semantics before choosing prototype defaults.
Complete the collection experiment against that lifecycle, then perform the full
completion audit above. A failed hypothesis is a result if its counterexample is
preserved and the end-to-end contract is still met by the chosen baseline.

The registered [experiment discipline](../../experiments/README.md) applies to each
cell. Passing E3R alone, an in-memory demo, or a green build does not close this goal.
