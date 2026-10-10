# Capacity/lifecycle completion map

Prepared read-only against the current working tree; no workload, build or validator ran. These are proposed admission budgets and commands, not results. Explicit completion scope covers the later waves; root serializes execution and freezes new protocols before measurement.

## Admission and finite budgets

All commands use `python3 tools/resource_group.py -- COMMAND`, mounted `/run/media/kmosoti/data/FabricO11y` caches/scratch, 16 GiB high/20 GiB aggregate max/no swap. Keep four CPU placement, resource receipts, clock controls, sampled storage accounting and cleanup. Never use remote hosts here.

Proposed additional machine-work ceiling: **800 minutes (13 h 20 min)**, including builds, controls, validators and cleanup: shared preparation/closure 60 minutes; C1-6 15 minutes; C2 120 minutes; C4 60 minutes; soak 125 minutes; outage 155 minutes; development lifecycle 265 minutes. Register this finite allocation separately from the spent screen budget. Each stage stops with preserved evidence when exhausted.

Per-run deadlines: C1-6 900 s; C2 process 120 s and bounded batches under 1700 s; C4 cell 900 s; soak frozen inner 7200 s/outer 7500 s; each outage inner 3000 s/outer 3100 s; fresh lifecycle workload maximum 14400 s/outer 15000 s. The longer outer deadlines require a minimal finite deadline option in `resource_group.py` (currently hardcoded 30 min), with the same memory/swap constraints and recorded requested/effective deadline. No existing qualification runner limits change.

Ordinary cells retain exact source/ACK/replay, server RSS ≤2 GiB, each node RSS ≤64 MiB, ACK-observed p99 ≤1 s, producer lateness p99 ≤100 ms, 5 ms clock guard, zero unexpected exits/gaps and finite drain. Pressure cells have separate expected refusal/gap ledgers. Initial diagnostic scratch ceiling is 4 GiB/free reserve 4 GiB; soak explicitly requires its registered 5 GiB live-data ceiling. Evidence caps remain protocol-specific, and failure preservation precedes cleanup.

## C1-6

Existing `tools/bench/labs/dev_small/run.py --cell c1-6` already selects one node, 10/30/10 logs/s, H256 published prefix, Scan, 60/60/60 s, five-second settle and twenty-second drain. Its native helper supports the required seeded-state shape. Minimal correction: parameterize destination/protocol/queue in the private runner and coordinator, whose constants currently target spent run-02 allocation; create a new campaign root. Preserve prefix hashes and empty active-journal start. Run private `prepare.py` and controls on the final binary first. C1-6 was deferred for time/evidence reserve, not a measured failure.

## C2 acceptance

Build prerequisite: `cargo build --release --locked --workspace --bins --examples`. Existing `readiness_memory_lab ROOT SHAPE MIB 2703204353 reference|bounded` is explicitly reconstructed diagnostic evidence, **not** the registered generator. It batches 64 rows and adds spans, whereas the study shape models 100 nodes, two logs/s and 32 metric points/15 s. Reimplement the registered generator in a private verification fixture using study evidence; retain seed `0xA11FA001`. Eight cells: four shapes at 64 MiB plus steady 16/32/128/256 MiB; three fresh processes per builder/cell (48 builds), same input digest, independent ordered row ledgers, determinism, counted heap, CPU/time/read/write/spill, leftovers and 60/10-second window pruning amplification.

The diagnostic runner currently omits pruning, executes one pair, and wrongly compares physical `text_filter.bin.rows` across legitimate byte-capped groups. Before acceptance, separately commit a precise interpretation preserving the original manifest failure; verify each physical group/filter relationship and exact logical rows, with negative controls. Do not blanket-ignore metadata or pruning differences. Applicable logs/metrics byte equality, 80 MiB ceiling and ≤10% steady 64→256 heap growth remain unchanged. BS-2/4/5/8/9 require their independent properties/faults/policy/docs checks; BS-7 belongs to C3.

## C3

Freeze candidate binaries plus harness/dependency files. Soak command inside the launcher: `python3 -B tools/qualification/runner.py --out DATA_DRIVE_OWNED_SOAK --duration-s 7200 --disk-bytes 5368709120 --max-output-bytes 1048576 -- python3 -B FROZEN/tools/soak_tier.py --seed 0xA11FA001 --bin-dir FROZEN --server-cpus 0-1 --sim-cpus 2-3`. Keep **5460 s**, all ten gates, no smoke substitution. For outage use the analogous runner with 3000 s, 1073741824 bytes and `FROZEN/tools/outage_drain.py --seed SEED --bin-dir FROZEN`, seeds `0xA11FA001`–`003`, sequential, default 1800 s outage and 600 s drain gate.

Fresh development lifecycle needs a new private driver: empty state, one real Spindle, sustained 10 logs/s/900-byte mix, real 15 s metrics/TLS, natural 64 MiB rotations, continuous queries and exact final grading. Q1-fresh archived 1195.914 encoded Batch bytes/log including metrics; a rough 10/s estimate gives ~11223 s for two thresholds. This phase-averaged proxy is not calibrated normal throughput. Use final-candidate calibration, freeze duration before admission, require two actual publish/reclaim cycles within 14400 s; otherwise report incomplete. No near-rotation seed or changed threshold.

## C4

Native spawning uses `prlimit --as`, not nested cgroups. Add verified delegated child groups beneath the outer service; record kernel memory.high/max/swap, pids, CPU quota/placement and charges before releasing child startup. Development maxima: server512 MiB/node128 MiB. Small: server3072 MiB/node256 MiB; host aggregate3328 MiB/high2816 MiB/tasks640, service tasks512/128. Model distinct edge-host slices; never place20nodes in one installed host slice. Four declared cells cover ordinary+finite burst/drain and separate journal/Spool pressure for each profile. Freeze quotas, configs and expected-gap ledger; report accepted custody, backpressure, queue slope/drain and disk cleanup. Delegation unavailable means environment unavailable, never RSS-based substitution.
