# CR2 execution slicing amendment

Status: draft; root registers separately before new execution.
The original [CR2 protocol](catalog-borrowed-log-protocol.md) remains frozen.
CQ1 diagnostic receipt `catalog-boundary-diagnostic-01` completed nine trials in
411.7 seconds at approximately4.2GiB whole-job peak. This motivates bounded
execution slices rather than admitting all12 full CR2 children plus builds
under one1400-second driver deadline. No CR2 outcome is inferred from CQ1.

This amendment changes orchestration only. Keep H65536,1024-byte bodies,
seed42, shuffled timestamps, page10000, tail/Segment, Scan/Walk, all four
shapes, first+3warm calls,64 complete independently graded chains per child,
three fresh alternating pairs per plain/count build. All12 children and768
chains remain required for a nomination; all original primary/guard rules stay.
A partial campaign is incomplete, never a zero-valued or successful population.

`borrowed_log.py --stage freeze` runs validation and adapter/guard controls,
builds plain/count baseline/candidate once, records source/verifier/protocol
hashes and archives all four executable binaries. No performance population
runs in this stage. `--stage pair` restores two selected binaries from those
archives, requires their exact original SHA256, unchanged driver/verifier,
controls-complete receipt and native plain/count/borrowed acknowledgements.
Every pair has its own fresh scratch, child commands, timings, phase archives,
full-chain maps, identity receipts and cleanup; no rebuilding between slices.
Pairs1/3 run baseline then candidate; pair2 reverses order.

All slices are siblings of `freeze` under one owned result root. Their `objects`
links resolve to a single shared exact-byte deduplicated store in that root;
chain and ledger maps continue to name the exact original retained bytes.
After exact `profile.collect` map verification, each closed trial's answer-map,
chain-map, oracle and fixture JSON is compressed through the separately
registered `compact_evidence.compress_json` API. Deterministic gzip decoded
bytes are compared exactly, original/new hashes and lengths are retained
incrementally BEFORE removal, then removal completion is recorded. Aggregate
requires these receipts and rechecks decoded/compressed hashes and lengths;
logical JSON content and full-chain association remain unchanged.
The aggregate256MiB evidence cap includes shared objects, archived binaries
and every slice. Scratch8GiB,16GiB drive reserve, no swap and original cgroup
caps remain. Each freeze/pair driver deadline is1400s within1500s coordinator.
Root admits each slice against the remaining shared campaign/stage budget.
The first full plain pair supplies a runtime admission checkpoint for remaining
pairs. If remaining budget cannot admit them, record incomplete and stop;
never narrow workloads or substitute partial aggregation to claim nomination.

Run freeze once (all commands remain inside the resource launcher):

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-borrowed-freeze-01 --lab memory --stage capacity --seconds 1500 -- python3 -B tools/bench/labs/catalog/borrowed_log.py --stage freeze --out docs/experiments/benchmarks/data/catalog-borrowed-log-run-01/freeze
```

Then run each `BUILD` in `plain counted`, each `PAIR` in `1 2 3` serially,
substituting these explicit values in both ID and output (six jobs):

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-borrowed-BUILD-pair-PAIR --lab memory --stage capacity --seconds 1500 -- python3 -B tools/bench/labs/catalog/borrowed_log.py --stage pair --freeze docs/experiments/benchmarks/data/catalog-borrowed-log-run-01/freeze --build BUILD --pair PAIR --out docs/experiments/benchmarks/data/catalog-borrowed-log-run-01/BUILD-PAIR
```

Only after six completed slices, run the aggregate validator (no workloads):

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-borrowed-summary-01 --lab memory --stage capacity --seconds 60 -- python3 -B tools/bench/labs/catalog/borrowed_log.py --stage aggregate --freeze docs/experiments/benchmarks/data/catalog-borrowed-log-run-01/freeze --out docs/experiments/benchmarks/data/catalog-borrowed-log-run-01/summary --slices docs/experiments/benchmarks/data/catalog-borrowed-log-run-01/plain-1 docs/experiments/benchmarks/data/catalog-borrowed-log-run-01/plain-2 docs/experiments/benchmarks/data/catalog-borrowed-log-run-01/plain-3 docs/experiments/benchmarks/data/catalog-borrowed-log-run-01/counted-1 docs/experiments/benchmarks/data/catalog-borrowed-log-run-01/counted-2 docs/experiments/benchmarks/data/catalog-borrowed-log-run-01/counted-3
```

Aggregation reads both trial oracle receipts (JSON or exact gzip), requires64
unique passed verdicts for the exact sixteen files/four iterations per child,
checks complete cleanup, exact archive receipts, expected variant/order sets,
native selectors and every timings-derived metric against its pair summary.
It rejects missing/duplicate pairs and mismatched provenance/fixture hashes,
recomputes unchanged guards and injects missing/duplicate/provenance/guard
defects that must be rejected. Complete-chain count is counted from receipts. False nomination is collected evidence;
process exit0 means complete collection, not performance acceptance.
Builds, controls and validators were not run during amendment preparation.
