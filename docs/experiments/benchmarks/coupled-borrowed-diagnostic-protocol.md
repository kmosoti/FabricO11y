# C4 attribute-associated row cost diagnostic

Status: registered before builds/measurements. The concrete
[capacity preparation](coupled-capacity-preparation.md) supplements the original
packet; this is a smaller initial screen admitted prospectively to fit evidence.
No old CR2 identity, criterion, oracle or default changes.

Freeze 4,096 logs, 32 Batches/Groups, one Segment, empty journal, seed 42 shuffled
unique timestamps, 1,024-byte bodies; attributes empty or eight 128-byte strings
per row. Each attribute key's value repeats across rows; this tests a compressible
case, not arbitrary attribute entropy/cardinality. The two populations have
different encoded sizes, which must be reported. Limit 50; selective one hit per
128 rows (32 hits), broad all 4,096. Walk primary, Scan context. Synthetic frontier
32 supplies component query coverage; it is not a service ACK.

H1: rich attributes increase unchanged inclusive decode/validation/output work,
explaining lower allocation without reliable CPU improvement. H0: paired timing
or observer effects cannot distinguish the proposed explanation. Attribute deltas
are associated costs, not isolated JSON parsing CPU.

Build four binaries, plain/counted × owned/borrowed (`FABRIC_BORROWED_LOG_EXPERIMENT`
0/1), using existing allocator/clock features only. Byte-verify archived binaries.
Run 12 fresh children: one pair per attribute population and plain/count-off/
count-on regime; rich order reverses empty order. Per plan/shape: one warm-up,
three measured first pages and one canonical full drain. Require 48 independently
graded canonical chains and 144 exact first-page associations. Existing rejected-
row validation tests precede measurement; truncated-chain/duplicated-row oracle
controls reject. All input Batch ledgers are retained losslessly with SHA readback.

Phase ledger <=8,192 rows per acquisition; existing inclusive nested spans only.
Direct-child CPU subtraction retains unattributed residual and rejects negative
residual beyond max(1 microsecond, 1% total). Do not sum nested spans. Observer
admission requires matched phase-on/off CPU, wall and requested-byte ratios each
in [0.95,1.05]. A miss retains semantic results but makes causal phase inference
inconclusive. One diagnostic pair cannot nominate a speed optimization.

Retained dataset `catalog-borrowed-attribution-run-01` has 48 MiB hard cap including
16 MiB projected failure reserve. Before each child project 4 MiB new objects per
unseen attribute population plus 1 MiB trial overhead; check live evidence/scratch
every 250 ms. Archive/readback four binary gzip objects, remove verified duplicate
scratch copies, restore only active binary. On failure remove only byte-verified
duplicates; retain meaningful fixture/results for archive cleanup. Scratch <=8 GiB,
drive reserve >=16 GiB; 800s driver within 900s coordinator. Initial launch resource
allocation and unchanged 14,400s frontier apply.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-c4-diagnostic-01 --lab memory --stage capacity --seconds 900 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 48 --capacity-reserve-mib 48 -- python3 -B tools/bench/labs/catalog/coupled_borrowed.py --out docs/experiments/benchmarks/data/catalog-borrowed-attribution-run-01
```

Freeze source/native/helper/protocol identities before measured children. No source
edits during builds; capture actual commands/exits, fixture bytes/storage, raw
metrics/oracles, resource and cleanup observations. Stop on correctness, bounds
or provenance drift, preserve original run and retry only under fresh identity.
