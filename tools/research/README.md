# Research harnesses

The experiments of the [research ledger](../../docs/research/ledger.md) as reproducible runs: the generator, the prototype patches, the harness scripts, and a smoke loop that runs the whole chain on a four-CPU container in a few minutes. Nothing here is product code or a registered check; the [documentation](../../docs/research/experiment-durability.md) says what is durable and what is not yet.

## Layout

| Path | What |
| --- | --- |
| `build_prototypes.sh` | Builds `bin/stock/fabric-server` from HEAD, then a throwaway worktree with the patches below applied, from which `bin/proto` and `bin/budget` (the same binary; the budget is an environment variable), `bin/sealbench` and `bin/fobbench` come. The product tree is never modified. |
| `patches/query-prototypes.diff` | The tail key index, the threshold walk and the budget boundary on `query.rs` and `segment.rs`, selected at run time by `FABRIC_PROTO_TAIL=index`, `FABRIC_PROTO_TOPK=on`, `FABRIC_PROTO_BUDGET=<rows>`, `FABRIC_PROTO_DISPOSITION=sound`. |
| `patches/bench-wiring.diff`, `patches/sealbench.rs`, `patches/sealbench-driver.rs` | The sealer-study module and its driver (`gen` workloads, `fob` measurements), as a module and example of the server crate in the worktree. |
| `patches/fobbench.rs` | The observation codec micro-benchmark. |
| `fetch_corpus.sh`, `corpus.sha256` | The eight Loghub samples used as real log text, checked against recorded digests. |
| `harness/` | The experiment scripts, one per record: `tailbench.py` (L-03), `topk.py` (L-04), `budget.py` and `budget_recheck.py` (L-05), `l21.py` (L-21), `retscale.py` (retention scale), `e2e_latency.py` and `qattr.py` (latency and attribution), `segstat.py`, `reencode.py`, `corpus.py`, `corpus_seal.py` (storage layout). `research_paths.py` resolves every path from `FABRIC_RESEARCH_ROOT`. |
| `smoke.sh` | Build, fetch, generate a 16 MiB real-text tail, run the L-21 harness in smoke mode, the codec micro-benchmark and the cost model's calibration test. |

## Running on this machine

```
bash tools/research/smoke.sh                     # the whole loop, a few minutes
FABRIC_RESEARCH_ROOT=/mnt/scratch/research bash tools/research/smoke.sh   # elsewhere than target/research
python3 -B tools/research/harness/topk.py "$FABRIC_RESEARCH_ROOT/data/topk"   # a full experiment (hours)
pip install -r tools/research/requirements.txt   # only for segstat.py, reencode.py, corpus.py
```

Budgets on a four-CPU container: the build about two minutes warm; a 64 MiB generated tail under a minute; a full `topk.py` or `l21.py` run about an hour, `budget.py` three hours (its stock drains dominate); 64 Segments of 1 MiB seal in seconds, a 64 MiB Segment in about a second. Disk: a full set of states is about 1 GiB; keep 3 GiB free. Servers are pinned to CPUs 0 and 1 by the harnesses so a second job can use 2 and 3 without disturbing the timings much; do not run two servers on one state directory (the journal lock is exclusive).

## What a run produces

Each harness writes `<out-root>/<name>.json` with every point, memory sample and the differential, and the server logs beside each state. Records under `docs/experiments/benchmarks/` cite these files; copy a run's JSON and log into the record's `data/` directory with the harness source as `.py.txt`.
