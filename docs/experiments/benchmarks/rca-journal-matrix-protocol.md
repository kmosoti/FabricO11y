# RCA journal case and plan matrix

The [native pilot](rca-native-pilot-protocol.md) ran as `rca-native-01`: 25 full
chains matched the unchanged independent query oracle, three corrupted answers
were rejected, all three Batches recovered exactly, and cleanup completed.
Expand that same finite method to all seven prepared cases under Scan and Walk.
No production behavior, oracle, wire format or resource ceiling changes.

H1: the exact fixture evidence and answer metadata survive initial delivery,
duplicate replay, any delayed delivery and graceful restart under either plan.
H0: one case/plan omits or substitutes evidence, mishandles counter reset/gaps,
changes an old snapshot or differs after restart. Correctness gates are exact;
query and resource timings remain small-sample diagnostics, not SLO estimates
or a plan speed nomination. This does not score a causal investigator.

Execute cases `rca-01` through `rca-07`, each with Scan then Walk, one server at
a time in a fresh contained job. Use the same authenticated packet, frozen
Rust/manifest membership and release binaries as the pilot. Keep original
pilot source snapshots and receipts unchanged. Parameterize only the laboratory
driver for case and plan; its default remains the original late-span Walk case.

Each cell has two initial Batches. Only case 05 has a third delayed Batch.
Cases 04/05 initially have two spans; other cases have three. Fresh case 04
still has two, while every other fresh case has three. Run the eight full
query chains at initial, after-delivery and after-restart observations, plus
one held initial trace chain finished after the delivery stage: 25 chains per
cell, **350 chains total**. For cases without delayed evidence, the middle
observation is a stable-population control, not a new injected failure.

Require producer/recovery identity and exact-byte equality, identical recovery
across restart, full unchanged-oracle verdicts (including reset and receive-time
gap semantics), stable held snapshots, clean server exits, no OOM/swap and the
same bounded cleanup/evidence checks as the pilot. Run omitted-row and altered
metric controls in every cell; run late-row-in-old-snapshot control in the two
case-05 cells: **30 rejection controls total**. No expected oracle outcome changes.

Admit each cell for at most 180 s and 32 MiB query evidence; internal runtime
remains at most 160 s including cleanup, 8 MiB live fixture scratch, at most 32
query requests per phase, 256 total HTTP requests, 1 MiB per response and 8 MiB
cumulative response bytes. All memory/CPU/task/disk/cgroup and source-binding
rules from the pilot remain in force. Unspent per-cell reservations are not a
new time allocation: actual time is charged to the same 7,200 s round, 36,000 s
frontier and 86,400 s campaign. Preserve final-check and cleanup time. If a cell
fails a semantic gate, stop expansion, retain the counterexample and investigate.

Use a dedicated matrix wrapper around the existing ledger. IDs are
`rca-matrix-01-scan`, `rca-matrix-01-walk`, through `rca-matrix-07-walk`.
Each command follows this pattern:

```sh
python3 -B tools/resource_group.py --delegate -- \
  python3 -B tools/bench/labs/rca/matrix_job.py \
  --id rca-matrix-01-scan --lab query --seconds 180 --reserve-mib 32 -- \
  python3 -B tools/bench/labs/rca/native.py --case rca-01 --plan scan \
  --out docs/experiments/benchmarks/data/hammer-reference-01/query/rca-matrix-01-scan
```

This expands journal/restart coverage only. Sealed publication, real application
SDK/Spindle transport, browser intake, Nomos, and blind interpretation remain
unmeasured. A later publication experiment needs explicit bounded padding and
proof of the actual published storage state; do not call this the entire
previously proposed 56-episode publication matrix.
