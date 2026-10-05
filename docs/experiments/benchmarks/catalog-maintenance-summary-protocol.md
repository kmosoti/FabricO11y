# CQ2 retained-evidence consolidation

Status: draft; register this checker separately before execution. It changes no
measurement, fixture, independent oracle or performance decision threshold.
[CQ2](catalog-maintenance-protocol.md) and its separately registered
[publication fixture correction](catalog-maintenance-fixture-correction-protocol.md)
remain authoritative. Failed preflight 01 and frozen build 01 remain evidence;
they cannot count toward the replacement campaign or be mixed into a matched pair.

The [standalone validator](../../../tools/bench/labs/catalog/maintenance_summary.py)
accepts the replacement freeze directory and exactly fifteen explicit slice paths:
rates 0/1/4, each with preflight, diagnostic, pair1, pair2 and pair3. It requires
nine tiny preflight trials, six full counted trials and eighteen full plain trials,
33 total. Every trial has `32*rate+2` complete original oracle chains, yielding
22, 374 and 1,430 chains by rate, 1,826 overall. Missing or duplicated paths,
populations, trials or verdict IDs fail; a partial campaign has no summary success.

Recheck compressed and decoded frozen binary hashes, source/oracle hashes, exact
frozen receipts, native variant/selectors, schedule/drain command counts and exits,
successful scratch cleanup and all six original per-trial mutation controls.
Reconstruct retained actual custody ledgers; verify every Batch digest, credential,
generation, identity, sequence, 128-row count, chronological timestamp, body length,
empty attributes and identical Batch bytes across trials. Independently re-hash
ordered native JSON framing and every decoded row chunk; the decoder cache is
capped at 96 MiB. Confirm byte-identical first-page framing/chunks, committed-prefix
association, snapshot, continuation count and terminal token. Require every retained
original verdict to pass with no violations and exact expected/answered row counts.
This validates the complete original oracle receipts and their retained artifacts;
it does not rerun a new semantic oracle or replace exactness with Scan/Walk agreement.

Read original `timings.jsonl` and independently recompute total CPU/wall equality,
32 append/ACK spans, rate-dependent query/serialization counts, eager refresh count
(31 only for the skipped-refresh preflight control), and sums of each nonoverlapping
phase. Reject missing/duplicated phases and phase sums exceeding the inclusive total.
Recompute all three plain eager/lazy total CPU and wall ratios for each rate.
H1 is supported only when all three total-CPU ratios are strictly below 1 with
complete exact controls; otherwise label that rate H0-compatible. Report counted
phase requested-byte sums and maximum observed phase peak increment separately.
Neither is a total-schedule allocation/peak measurement. Report 32 native durable
ACKs divided by total schedule wall time as schedule throughput, with no Spool,
forwarding, server demand or deployment claim.

Before reporting success, positive synthetic matrix/header/timing/reconstruction
controls must be accepted. The same checker functions must reject missing and
duplicate slices/verdicts, changed frozen provenance, wrong answer count, missing
append timing, mismatched refresh count, changed reconstruction hash and a missing
row chunk. These controls require no storage mutation and no new native workload.
Original independent query-oracle expected outcomes remain unchanged.

Root first executes all fifteen admitted slices serially against freeze 02.
The expected fresh names below preserve the failed rate-zero preflight 01:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-maintenance-summary-01 --lab query --stage query --seconds 600 -- python3 -B tools/bench/labs/catalog/maintenance_summary.py --freeze docs/experiments/benchmarks/data/catalog-maintenance-freeze-02 --slices docs/experiments/benchmarks/data/catalog-maintenance-0-preflight-02 docs/experiments/benchmarks/data/catalog-maintenance-0-diagnostic-01 docs/experiments/benchmarks/data/catalog-maintenance-0-pair1-01 docs/experiments/benchmarks/data/catalog-maintenance-0-pair2-01 docs/experiments/benchmarks/data/catalog-maintenance-0-pair3-01 docs/experiments/benchmarks/data/catalog-maintenance-1-preflight-01 docs/experiments/benchmarks/data/catalog-maintenance-1-diagnostic-01 docs/experiments/benchmarks/data/catalog-maintenance-1-pair1-01 docs/experiments/benchmarks/data/catalog-maintenance-1-pair2-01 docs/experiments/benchmarks/data/catalog-maintenance-1-pair3-01 docs/experiments/benchmarks/data/catalog-maintenance-4-preflight-01 docs/experiments/benchmarks/data/catalog-maintenance-4-diagnostic-01 docs/experiments/benchmarks/data/catalog-maintenance-4-pair1-01 docs/experiments/benchmarks/data/catalog-maintenance-4-pair2-01 docs/experiments/benchmarks/data/catalog-maintenance-4-pair3-01
```

Optional `--out JSONFILE` writes the same stdout JSON to a fresh file. If root
chooses different fresh slice names before registration, substitute their literal
paths; rates/stages remain independently read from archived driver arguments and
must cover the unchanged matrix. Exit zero means complete validated evidence,
including H0-compatible cells; it does not mean a performance improvement.
Root records validator source/protocol hashes, exact command/exit and resources.
No validator or native workload ran while preparing this registration.

For each remaining native slice, use the original driver command with
`--freeze .../catalog-maintenance-freeze-02`, `--objects .../catalog-maintenance-objects-01`,
the explicit rate/stage/fresh ID, `--cap-mib 1024 --reserve-mib 96`, and update
`--account` to include all existing query datasets: four CQ1 slices, metadata
precursor, both preboundary archives, canonical evidence pool, both compaction
receipts, frozen maintenance build 01, failed maintenance preflight 01 and every
completed replacement CQ2 slice. The driver adds freeze 02, current slice and
real shared object pool itself. Before each admission root additionally inventories
coordinator/recovery/preserved-failure bytes and the separate capacity datasets
against the registered aggregate allocation. Admit tiny preflights first, then a
full counted diagnostic cost checkpoint, then remaining plain pairs; no shell loop
may implicitly admit the next workload. Current shared time/resource budgets apply.
