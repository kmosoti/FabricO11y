# E1R/E3R receipt and retry cost protocol

Status: registered before benchmark implementation or timing. Correctness gates are
[E1R](../ablation/coverage-e1-run-01.md) and [E3R](../ablation/resume-e3-protocol.md).
This measures research-library overhead; no latency target has been promised.

## Workload and controls

Use the existing S1 mixed and shuffled-Log generators with seeds 201/202/203,
2,048 events, 64-row blocks and the 128-query eight-predicate cycle. Preserve the
existing generator code or record its hash and exact adaptation. Use one warmup
then five paired trials in alternating baseline/receipt order per snapshot. Each
trial executes each query once. Run sequentially on this shared host; record other
active work and build/source/platform versions. Use release, locked dependencies.

Fixture provenance: "existing S1" means the workload and eight-predicate cycle in
the [S1 runner](../../../tools/storage-probe/src/main.rs), including its deterministic
`cycle * 127` time offsets. It does not mean the later E3 fault corpus, which adds
duplicate IDs and uses its own seeded query offsets. E3 supplies a correctness gate;
the already registered S1 fixture supplies the cost comparison. The shared cost
helper records its source hash; the S3 scaled cell changes the count/modulus and
repeats the cycle as that cell explicitly registers. Duplicate-ID edge cases remain
in the separate correctness suites.

Compare two outputs with identical physical positions and full-event row digests:
(1) a direct scalar scan of the source Vec plus digest calculation for matches;
(2) E3 execute with all blocks available, followed by Accumulator::new verification.
The scalar evaluator is independent of the production predicate and summary code.
Fixture generation and expected results are outside measurement. Returned digests
are consumed and checked after each measured operation. False negatives, wrong
ordering or wrong row digests fail the entire trial.

Measure construction separately: S1 Snapshot with conservative summaries versus
SealedSnapshot. Both include event ownership transfer and summary construction;
prepare equivalent input Vecs before timing. This records authenticated metadata
build cost; differences include summary representation, so it does not isolate
hashing alone. Neither construction includes durable I/O.

## Measurements

Record query elapsed nanoseconds per predicate and P50/P99 (nearest rank), total
query time, construction time, serialized complete-page bytes, residual bytes,
and process peak RSS. Isolate receipt verification by timing Accumulator::new on
an already-produced page (clone outside timing). Time two complementary partial
pages plus merge to closure, and separately an identical replay. Verify closure
matches the scalar full answer and residual becomes empty. Record complete-page
and two-page byte totals; partial answers may cost more than one full answer.

The trial process CPU measurement includes setup, all timed operations and post-run
checking; report it as whole-process CPU, not isolated query CPU. Wall measurements
use Instant; no wall-time-to-CPU inference. Per-query serialization stays outside
query timing and is reported as byte overhead, not free networking. If phase CPU
is added, define and preserve its measurement boundary separately before running.

Freshness here is constructor-completion to immediate query availability; record
construction as the in-memory readiness cost. Durable ingest-to-publication latency
belongs to S2 and the end-to-end run. Do not present this as time-to-durable-search.

## Interpretation

All paired observations and the ratio distribution are retained. Report overhead
and work reduction per query family, never only the favorable narrow queries.
The experiment has no universal speed-winner threshold: the correctness prototype
uses receipts to expose incomplete work even if they are slower. A proposed
performance optimization needs a new registered gate. Byte counts are wire JSON
size for the versioned S2 serializer, not an estimate of allocator or device cost.
No production latency, malicious-executor proof, durability or raw-retention claim
follows from this in-memory comparison.

## Timing implementation boundary (registered before the harness)

On this Linux host, phase CPU uses CLOCK_PROCESS_CPUTIME_ID alongside Instant wall
time. Record timer-call overhead with an empty calibration loop; do not subtract a
noisy estimate from samples. Whole-process CPU/RSS are reported separately and include
setup/checks. Gates using server/build/query CPU refer to the named measured phase,
not the sum of unrelated validation work. All results must retain these boundaries.
