# Workload sizing: development through enterprise

Status: **planning model, not deployment qualification**. Calculations use frozen [local writer pilot data](../experiments/benchmarks/data/streaming-output-local-run-01/pairs.json), the existing [ingest investigation](../experiments/benchmarks/ingest-run-01.md), and explicitly chosen workload assumptions. No new workload was benchmarked. Runtime behavior, installed limits and product promises are unchanged. Work branch: `milestone/workload-sizing`, based on streaming-output evidence commit `279e738`.

## Workload definitions

Deployment names alone do not determine resources. This model fixes aggregate event rate, average encoded bytes per event (including amortized Batch/Group metadata), burst and retention. An event can be a log, metric point or span. **1 KiB is a sizing assumption, not a measured production average.** It is encoded input, not original log text. Small log lines with many attributes can cost more CPU/RAM per byte than the pilot's 1 KiB bodies. Node count determines fleet state and per-node buffering; event volume primarily determines storage and pipeline work.

All scenarios assume a 3× burst lasting 60 seconds, and a one-hour sealing interruption buffer at peak rate. The interruption budget protects against a stopped sealer while durable intake remains available. A server/network outage instead accumulates data at each Spindle. Query-heavy operation, high availability, replicas, backups and cross-region transfers are outside the table.

| Scenario | Nodes | Aggregate events/s | Average / peak encoded MiB/s | Encoded GiB/day | Retention |
| --- | ---: | ---: | ---: | ---: | ---: |
| Development | 1 | 10 | 0.0098 / 0.0293 | 0.824 | 1 day |
| Small | 20 | 1,000 | 0.977 / 2.930 | 82.4 | 7 days |
| Moderate | 200 | 10,000 | 9.766 / 29.297 | 824 | 14 days |
| Enterprise | 2,000 | 100,000 | 97.656 / 292.969 | 8,240 | 30 days |

MiB/GiB/TiB use powers of 1024; network Mbit/s uses powers of 1000. The example nodes evenly share the workload solely for the spool calculation. Real hot nodes need individual sizing.

## Planning hardware and storage

The CPU/RAM column gives **starting configurations for experiments**, not proven minimums or guaranteed capacity. A vCPU is a scheduling resource; configuring a worker never reserves a CPU. Development here means running the pipeline; compilation may need extra RAM/disk (the current release/debug build cache alone reached about 4 GiB).

| Scenario | Trial server vCPUs / RAM | Proposed peak sealing workers | Provisioned state storage: 0.5× / 1.6× stored/input | Peak network with 20% margin | Practical link |
| --- | --- | ---: | --- | ---: | --- |
| Development | 2 / 4 GiB | 1 | 25.6 / 26.9 GiB; start with 64 GiB device | 0.30 Mbit/s | Existing local network |
| Small | 2 / 4 GiB | 1 | 386 / 1,179 GiB; provision 512 GiB / 2 TiB | 29.5 Mbit/s | 1 Gbit/s |
| Moderate | 4 / 8 GiB | 1 | 7.17 / 22.66 TiB; provision 8 / 32 TiB | 295 Mbit/s | 1 Gbit/s |
| Enterprise | 16–24 / 32 GiB | 10 | 152.13 / 484.06 TiB; provision 160 / 512 TiB | 2,949 Mbit/s | 10 Gbit/s |

These are central-server resources; Spindle resources are additional. Large storage rows usually mean several devices, and storage can cost more than CPU. The table's 25% disk margin includes transient builder space and journal reserve, but excludes OS/build artifacts, backups, replicas and filesystem-specific overhead. Device selection also needs measured sync latency, throughput and endurance; capacity alone is insufficient.

Two storage cases are deliberately shown:

- **0.5×** is an ordinary-compression planning assumption. The separate real-text investigation reported roughly 0.41×, but that result is workload-specific.
- **1.6×** approximates the pilot's high-entropy fixture (about 1.59×). Exact Batch custody plus projected query tables can expand data. This is a tested example, not a worst-case bound. Repeated-body fixtures compressed far better, about 0.014×, and would give misleadingly cheap sizing.

Enterprise is an **extrapolation beyond current qualification**. Ten concurrent workers fit the parser's 1–16 range, but the estimated server memory allowance is about 8.81 GiB, exceeding the installed `MemoryMax=3 GiB`. Installing the existing package on a 32 GiB machine does not raise that cap. A changed deployment profile needs explicit contract/configuration work and fresh validation. Automatic multi-server routing, replication and cross-server query coordination are proposals in the [scaling design](scaling-design.md), not product features. Manually deploying independent servers can divide custody by enrollment, but requires operational planning and does not supply a unified query/control service.

## Equations and evidence boundary

Let `lambda` be aggregate events/s, `s` encoded bytes/event, `beta` burst multiplier, `T` retention seconds, `c` stored/input ratio, `O` interruption seconds, `F` journal-file bytes and `w` concurrent builders.

```text
R_avg = lambda × s                            [bytes/s]
R_peak = beta × R_avg                         [bytes/s]
D_retained = R_avg × T × c                    [bytes]
J = max(20 GiB, R_peak × O)                   [bytes]
D_temp = w × F × (1 + c)                      [bytes]
D_state = 1.25 × (D_retained + J + D_temp)     [bytes]
Network_peak = 1.20 × 8 × R_peak               [bits/s]
Disk_write_peak ~= R_peak × (1 + c)           [bytes/s]
Disk_read_peak ~= R_peak                      [bytes/s]
```

`D_temp` includes a duplicate allowance for active journal input alongside output and thus errs upward relative to a journal budget already including those files. It describes today's in-memory builder, not the proposed spill sorter. Extra spill runs/read/write traffic must be modeled if that design is implemented. Reads/writes are logical payload estimates, not a physical-I/O bound: frame overhead, metadata, double syncs, write amplification, page cache and contention also matter. At enterprise peak, these equations give about **439–762 MiB/s writes plus 293 MiB/s reads**, before those costs. At average enterprise rate the writes are about **146–254 MiB/s**. NVMe or an appropriate storage array is a sensible test platform; measured commit latency is essential.

Sealing calibration uses the **slowest of three baseline 64 MiB high-entropy pilot trials**: about 62.55 MiB/s through `segment::build`, with resident input and allocator instrumentation. The calculator derates this by 50% to **31.27 MiB/s per worker**. This factor is a planning choice for missing replay/ingress/contention, **not a calibrated confidence interval or guaranteed lower bound**. It does not estimate end-to-end throughput, ACK latency or CPU consumption. We use baseline numbers so sizing does not depend on adopting the experimental writer.

```text
mu = 0.50 × slowest measured baseline builder bytes/s
w_avg = ceil(R_avg / mu), at least 1
w_peak = ceil(R_peak / mu), at least 1
```

The model gives average workers **1, 1, 1, 4**, and peak workers **1, 1, 1, 10**. It assumes independent workers and no shared bottleneck, which is precisely what must be tested. `R < w × mu` is a fluid stability condition, not a p99 latency guarantee. Real grouped builds, ordered reclaim, failed earliest files, fairness and finite queues can delay capacity restoration. A peak worker count is a static proposed setting; an adaptive manifold has not been implemented.

The earlier [live ingest investigation](../experiments/benchmarks/ingest-run-01.md) reported 48/75/81 MB/s of real-text ingest at 1/2/3 workers, on three server CPUs, with back-pressure. Those MB/s are **real-text units**, whereas this calculator uses encoded Group bytes. Its [scaling discussion](scaling-design.md) cites roughly 2.7 encoded bytes per text byte for one corpus. Consequently the numbers must not be combined as though they measured the same rate. The earlier run's 584/976/1,564 MiB peak RSS corroborates memory growth with workers, but is neither a larger-host scaling law nor a long-term memory bound.

Memory planning deliberately uses **640 MiB per 64 MiB builder** (10×), rounded from the historical soak finding, rather than treating the writer pilot's 420 MiB baseline RSS as universal. The input shape differed, and the [soak](../experiments/benchmarks/soak-run-01.md) failed its RSS-growth gate. Allocator retention and accumulated state remain risks.

```text
M_server = 1 GiB fixed allowance + w × (640 MiB × 1.25)
M_host_floor = M_server / 0.75 + 1 GiB host reserve
```

The fixed allowance provisionally includes intake's 64 MiB byte queue, active commit state, TLS/runtime, modest fleet state and light administration. It is an assumption needing measurement, and does not cover arbitrary queries or unbounded cardinality. At one worker, modeled host floor is **3.38 GiB**, hence the 4 GiB starting point. At ten workers it is **12.75 GiB**, with 32 GiB proposed to investigate CPU/disk/page-cache contention and uncertain record shapes. The model is an allowance, not a hard cap. Cgroup memory also charges page cache/kernel memory; RSS alone cannot establish it fits. The streaming writer's observed 12% RSS reduction applies only to one fixture, and no blanket 12% discount is taken here.

A one-thread builder spends at most one CPU at a time, but wall time includes I/O and does not give its CPU demand. Proposed vCPUs leave room for commit/TLS/other work; CPU seconds per encoded byte across live loads must be measured before predicting actual CPU percentages. No automatic CPU reservation follows from these thread counts.

## Cheap resources, bursts and retention

Cheapness depends on which resource binds. A low CPU percentage does not imply a second worker will help if disk sync latency or memory blocks progress. Use the fewest workers that keep backlog and ACK latency within the chosen limits; measure RAM and I/O as well as CPU. Keep exact data fidelity as a hard constraint.

For the enterprise example, four workers suffice for average traffic in the fluid model. Keeping four through a 60-second 3× burst builds about **9.84 GiB of backlog**, then needs about **367 seconds to drain** while ordinary traffic continues. Ten workers model adequate peak capacity, at higher RAM/CPU demand. These are potential operating points, not measured controller behavior. Even four workers have a modeled server allowance of 4.13 GiB, over the installed 3 GiB cap. Restart/error holes can invalidate the drain estimate.

The journal interruption budgets are **20 / 20 / 103 / 1,030 GiB** for the four tiers. Buffering only delays overload: if sustained intake exceeds sustainable sealing, any finite journal eventually fills. Full journal means back-pressure and sender retries, not extra storage capacity. Choosing one hour at peak is deliberately demanding; changing that requirement changes disk cost directly. The source's 20 GiB default is a configured ceiling, not preallocated disk.

Default Segment retention is the smaller of 24 hours and 20 GiB. With 0.5× storage, this model fills 20 GiB after roughly **1,165 / 11.65 / 1.165 / 0.1165 hours**; the development case therefore stops at the 24-hour age limit. To achieve the table's 7/14/30 days, set **both** age and byte budgets accordingly. With 1.6× storage, small/moderate/enterprise hit 20 GiB in about **3.64 hours / 21.85 minutes / 2.18 minutes**. Newer entire Segments can be evicted immediately when the budget is undersized; nominal days are not a retention guarantee.

Spindle's default Spool is **256 MiB on disk**, not 256 MiB held in RAM. With the evenly divided examples, a one-hour server/network outage at average rate needs about **35 MiB per development node** and **176 MiB per node for the other tiers**; at 3× rate, about **105 MiB / 527 MiB**. Thus default Spool is tight at average and insufficient at sustained peak for the latter tiers. Size each node by its own output rate and required outage interval, adding framing and margin. The installed Spindle `MemoryMax=256 MiB` and the separate 64 MiB RSS qualification gate are different quantities; neither supplies a universal performance/resource guarantee.

## Reproduce or change the assumptions

The dependency-free [calculator](../../tools/bench/sizing/calculate.py) reads [scenario inputs](../../tools/bench/sizing/scenarios.json) and the frozen pilot. The [complete output](../../tools/bench/sizing/estimates.json) includes its evidence SHA-256, derivations, default-limit flags, and per-tier storage ratios. It does not change server configuration or run any services.

```sh
python3 tools/bench/sizing/calculate.py
python3 tools/bench/sizing/calculate.py --scenarios /path/to/custom-scenarios.json
```

Keep units and workload boundaries consistent when editing assumptions. Doubling events/s or encoded bytes/event doubles input, network, retention and spool bytes. Worker counts increase at integer thresholds. RAM mainly depends on active builder count and file/record shape, not the retention duration; increasing retention consumes disk and can change query/cache costs. CPU, RAM and compression cannot all be scaled linearly from average payload size.

## Validation and next measurements

This change adds arithmetic and planning documentation, not an experiment protocol or performance claim. Calculator output was recomputed, audited for byte/day/network conversions and journal/default-limit flags, and checked for deterministic output. Invalid negative-rate input was rejected. `python3 tools/bench/sizing/calculate.py` exited 0; repeated output was byte-identical to the checked-in JSON. An independent Decimal arithmetic audit checked all four byte/day and network conversions, plus the default-limit flags and enterprise backlog/drain calculation. A negative-rate input was rejected with `ValueError`. Final `bun tools/docs/check.mjs` and `git diff --check` exited 0. `cargo xtask checks --profile fast` exited 1: 19 checks passed, Clippy failed at unchanged `crates/fabric-observation/src/crc32.rs:81` (`chunks_exact_to_as_chunks`), the previously recorded toolchain/baseline issue. No gate was changed. [Fast-check output](../experiments/benchmarks/data/workload-sizing/fast.txt). Existing pilot measurements remain tied to their original sources/revisions.

On the current cloud host, development/small scenarios are practical native experimental starting points. Moderate throughput can be sampled with bounded temporary data and configured disk limits; full 14-day storage does not fit this workspace. Enterprise's extrapolated CPU/storage and sustained burst are outside this host's capacity. No Docker dependency is required.

On the personal PC, first measure the real event-size distribution, stored/input ratios and per-node rates. Then register live sustained/burst runs at 1/2/4 workers, with finite runtime/disk limits, exact delivery/query oracles, ACK p50/p99, CPU seconds per encoded byte, process RSS and cgroup peak memory, storage read/write throughput, journal backlog/reclaim, and recovery. Check real filesystem sync latency and resource enforcement. If the PC is smaller than the enterprise platform, it can calibrate per-worker service and contention, but it cannot qualify a 16–24 CPU server or hundreds of TiB of storage. That needs representative hardware. Query-heavy workloads and availability requirements need a separate sizing dimension.
