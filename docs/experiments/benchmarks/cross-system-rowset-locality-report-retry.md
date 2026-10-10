# RowSet locality report retry

Prospective retry of the standalone report reducer only. The native 48-cell
screen at `query/rowset-locality-01` completed with exit 0 and remains the
unchanged source evidence. The first report attempt, coordinator
`rowset-locality-report-01`, failed at `read_outputs` while inspecting the
retained native archives. Preserve that failed receipt and its logs.

The failure exposed a mismatch between the reducer's expected archive shape
and the runner's actual `tarfile.add(output, arcname='output')` output. Each
archive contains the root `output/` directory and five regular files: the three
exact operation outputs plus `a.native` and `b.native`. The reducer now permits
that one root directory and exactly those five direct-child files. It still
rejects absolute, traversal, nested, noncanonical, duplicate, linked, and
unregistered members. It checks the archive digest, complete regular-member
inventory and readback, exact operation-output bytes, and the combined native
serialized byte count.

The revised reducer also carries representative valid-root/file and invalid
archive-path controls. No native cells are rerun and no acceptance threshold
changes. If approved after source review, run it under a fresh coordinator ID
and report destination:

```sh
python3 tools/resource_group.py -- python3 -B tools/bench/labs/cross_system/run_sweep_job.py --id rowset-locality-report-02 --lab coordinator --seconds 120 --reserve-mib 8 -- python3 -B tools/bench/labs/cross_system/rowset_locality_report.py --source docs/experiments/benchmarks/data/cross-system-sweep-01/query/rowset-locality-01 --out docs/experiments/benchmarks/data/cross-system-sweep-01/coordinator/rowset-locality-report-02/report.json
```

The retry has not run. Its result will remain a held-density physical-ID
arrangement screen with two seeds, not a Fabric performance or format claim.
