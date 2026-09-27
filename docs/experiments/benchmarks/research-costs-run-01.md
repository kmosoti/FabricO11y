# Receipt, layout and sidecar costs, run 01

Status: measured on one shared WSL2/ext4 host after the full E3 correctness gate.
The separate JSON lifecycle remains the prototype baseline. This experiment compares
specific research mechanisms; it does not migrate the application's storage format.

## Registered contract and execution

The [receipt/resume](receipt-resume-cost-protocol.md),
[layout/postings](columnar-selective-s3-s4-protocol.md) and
[collection/sidecar](collection-sidecar-s5-protocol.md) protocols were fixed before
formal timing. Harness review clarified persisted metadata and durable index-build
cost accounting before this run. Correctness evidence is separate:
[E3](../ablation/resume-e3-run-01.md), [S2](../ablation/durable-snapshot-s2-run-01.md),
[layout](../ablation/columnar-selective-s3-s4-run-01.md) and
[collection/lifecycle](../ablation/local-prototype-run-01.md).

```sh
python3 -B tools/bench/run_research_costs.py target/research-costs/run-01 --formal
```

Use a fresh output path on durable local storage. Release builds use locked offline
dependencies. One warmup precedes five measured trials in alternating order. S1 mixed
and shuffled-Log inputs use seeds 201/202/203 and 2,048 events; the layout-only scale
cell uses seed 201 and 8,192 events. There are 128 queries per small snapshot and 512
per scaled snapshot. The E3 fault corpus has deliberately different duplicate-ID and
query-offset mutations; it is the correctness gate, not this cost fixture.

The runner exited **0**, with **490 child commands**, all exit **0**. It ran from `2026-09-27T13:40:04.331398+00:00` to `2026-09-27T14:25:59.211819+00:00` on `/dev/sdd ext4 /`. The recorded CPU is Intel(R) Core(TM) i7-10750H CPU @ 2.60GHz. Every timed answer passed exact positional and full-row-digest equality; sidecar variants also reproduced the same snapshot root. Warmups are retained but excluded from every table below.

The [raw results and command ledger](data/research-costs-run-01/raw/) preserve
individual timings, CPU, peak RSS, source/output hashes, calibration and kernel I/O
counters. [Descriptive metrics](data/research-costs-run-01/metrics.json) contain
per-phase CPU/wall medians, p50/p99 by mode, every query family's paired ratios and
resource costs. Phase medians use five trials; query p50/p99 values are medians of
each trial's nearest-rank percentile. Gates are evaluated separately per dataset.

## Hybrid layout result

Each candidate stores the complete raw Event as well as projected predicate columns.
All query paths authenticate the full file; projection saves decoding, not that byte
pass. The layout table includes external table anchors and excludes optional postings.
The raw summary also records totals with postings and their external digests.
Footer bytes are already inside Parquet. Independently retained source JSON is
reported separately and is common to all variants; the table below does not include
that source or the application's FOL2 journal. These are final retained bytes, not
filesystem allocation, replication or compaction write amplification.

In each candidate cell: **bytes / projected-time ratio / gate**. Bytes are relative
to JSON including corresponding trust metadata; time compares median trial p50s.
The registered gate requires bytes <=80% and time <=1.10x JSON.

| Dataset (shape–seed–rows) | JSON bytes | JSON p50 ms | Plain/64 | Zstd/64 | Zstd/256 |
| --- | ---: | ---: | --- | --- | --- |
| mixed-201-2048 | 723,381 | 5.427 | 153.00% / 0.911x / fail | 25.90% / 0.327x / pass | 19.02% / 0.204x / pass |
| mixed-202-2048 | 723,381 | 5.475 | 153.00% / 0.916x / fail | 25.90% / 0.336x / pass | 19.01% / 0.198x / pass |
| mixed-203-2048 | 723,381 | 5.855 | 153.00% / 0.866x / fail | 25.93% / 0.315x / pass | 18.98% / 0.190x / pass |
| shuffled_logs-201-2048 | 826,626 | 5.788 | 167.14% / 1.067x / fail | 23.15% / 0.339x / pass | 17.65% / 0.213x / pass |
| shuffled_logs-202-2048 | 826,626 | 5.860 | 167.14% / 1.048x / fail | 23.15% / 0.326x / pass | 17.65% / 0.206x / pass |
| shuffled_logs-203-2048 | 826,626 | 5.768 | 167.14% / 1.065x / fail | 23.20% / 0.331x / pass | 17.64% / 0.198x / pass |
| mixed-201-8192 | 2,898,507 | 22.184 | 152.93% / 0.901x / fail | 25.74% / 0.325x / pass | 18.83% / 0.182x / pass |
| shuffled_logs-201-8192 | 3,313,152 | 39.194 | 167.07% / 0.955x / fail | 23.03% / 0.291x / pass | 17.56% / 0.166x / pass |

`plain64` passes 0/8 dataset gates.

`zstd64` passes 8/8 dataset gates.

`zstd256` passes 8/8 dataset gates.

The small synthetic datasets, duplicated schema and cached reads limit transfer to
production. Log bodies use repeated templates and 128 repeated `x` bytes, favoring
compression; these ratios need a less repetitive workload before generalization. Compression and larger row groups can be promising without selecting a
new durable format. Per-family comparisons and full-materialization p99 remain in
the descriptive metrics; the aggregate gate does not imply every family improves.

## Selective postings

Only `rare` is indexed. Each cell gives the measured break-even number of rare-token
queries, followed by median per-query saving in microseconds. Amortization includes
pure construction **plus durable publication** of postings and their external digest.
Index bytes/digest are resident during querying; table reading and authentication
remain inside each query. Non-indexed predicates fall back to projection.

| Dataset | Plain/64 | Zstd/64 | Zstd/256 |
| --- | ---: | ---: | ---: |
| mixed-201-2048 | 18 / 241.7 µs | 22 / 198.3 µs | 25 / 191.8 µs |
| mixed-202-2048 | 15 / 263.2 µs | 19 / 215.2 µs | 21 / 202.0 µs |
| mixed-203-2048 | 13 / 319.6 µs | 21 / 216.4 µs | 22 / 200.6 µs |
| shuffled_logs-201-2048 | 13 / 404.1 µs | 12 / 381.2 µs | 13 / 386.7 µs |
| shuffled_logs-202-2048 | 20 / 285.4 µs | 18 / 351.4 µs | 13 / 413.8 µs |
| shuffled_logs-203-2048 | 16 / 328.2 µs | 16 / 346.7 µs | 14 / 358.1 µs |
| mixed-201-8192 | 11 / 658.5 µs | 8 / 786.0 µs | 10 / 722.2 µs |
| shuffled_logs-201-8192 | 8 / 1982.0 µs | 6 / 1855.0 µs | 5 / 1708.5 µs |

This is an observed amortization calculation, not a future-query guarantee. It uses
the median trial saving and median trial build-plus-publication cost. Repeated rare
queries, residency and unchanged snapshot identity are assumptions. Correctness tests
cover absent, malformed and wrong-table postings and deterministic rebuild separately.

## Receipt and retry overhead

The direct scalar comparison scans the source Vec and hashes matching full Events.
The receipt comparison executes E3 and verifies the resulting page. Serialization is
outside query timing; wire bytes are reported separately. These are in-memory query
costs, not time-to-durable-search or proof of an untrusted executor's work.

Each cell below is the median **paired receipt/scalar wall-time ratio** for that
query family, preserving all favorable and unfavorable predicates.

| Dataset | All rows | Narrow time | Tenant | Rare | Absent | Common | Combined | Empty range |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| mixed-201-2048 | 1.17x | 12.88x | 75.93x | 4.09x | 5.37x | 1.26x | 91.91x | 131.42x |
| mixed-202-2048 | 1.17x | 12.94x | 74.81x | 4.15x | 5.38x | 1.26x | 92.01x | 164.28x |
| mixed-203-2048 | 1.17x | 12.94x | 67.53x | 4.03x | 5.25x | 1.25x | 78.74x | 100.27x |
| shuffled_logs-201-2048 | 1.15x | 6.36x | 98.19x | 2.93x | 3.54x | 1.15x | 84.61x | 151.04x |
| shuffled_logs-202-2048 | 1.16x | 6.41x | 100.07x | 2.91x | 3.61x | 1.16x | 86.40x | 183.85x |
| shuffled_logs-203-2048 | 1.15x | 6.53x | 98.55x | 2.94x | 3.61x | 1.16x | 89.45x | 183.12x |

Constructor readiness, verification and retries have separate boundaries. The table
shows median wall milliseconds. S1 construction includes its conservative summaries;
sealed construction includes exact summaries and commitments, so their difference
is not hashing alone. Verify checks an already-created all-rows page. Resume executes
two complementary pages and merges to closure. Replay merges the same page again.

| Dataset | S1 build ms | Sealed build ms | Verify ms | Two-page resume ms | Replay ms | All-rows page B | Two pages B |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| mixed-201-2048 | 0.388 | 3.087 | 0.275 | 5.596 | 0.344 | 336,959 | 367,370 |
| mixed-202-2048 | 0.394 | 2.845 | 0.242 | 4.285 | 0.251 | 336,968 | 367,304 |
| mixed-203-2048 | 0.421 | 3.084 | 0.240 | 4.342 | 0.266 | 336,921 | 367,115 |
| shuffled_logs-201-2048 | 0.706 | 4.529 | 0.267 | 5.583 | 0.340 | 337,084 | 367,455 |
| shuffled_logs-202-2048 | 0.731 | 4.697 | 0.237 | 5.058 | 0.265 | 336,965 | 367,329 |
| shuffled_logs-203-2048 | 0.705 | 4.429 | 0.236 | 4.649 | 0.240 | 337,045 | 367,458 |

The full-page byte field in each raw result is the **sum over 128 queries**; it must
not be compared with the two-page all-rows retry as if it were one page. The table
uses the all-rows family's single-page size instead. Residual sizes and all family
page sizes remain in the metrics. No universal receipt speed gate was registered:
the prototype pays this overhead to expose incomplete coverage and reject mismatched
resume work. Timing calibration is retained without subtraction; ratios involving
very cheap scalar predicates are sensitive to timer overhead.

## Conservative sidecar result

The source constructs hints from the same retained Events. The server checks those
hints against every raw block before constructing its root. No evidence is dropped.
Each value is a median paired phase-CPU ratio. Benefit requires **both** server CPU
<=0.90x baseline and total source+server CPU <=1.10x baseline.

| Dataset | Server CPU ratio | Total CPU ratio | Hint bytes | Source bytes | Both gates |
| --- | ---: | ---: | ---: | ---: | --- |
| mixed-201-2048 | 0.848x | 1.669x | 32397 | 720492 | fail |
| mixed-202-2048 | 1.028x | 1.809x | 32397 | 720492 | fail |
| mixed-203-2048 | 1.045x | 1.815x | 32397 | 720492 | fail |
| shuffled_logs-201-2048 | 1.040x | 1.745x | 49118 | 823737 | fail |
| shuffled_logs-202-2048 | 0.992x | 1.670x | 49118 | 823737 | fail |
| shuffled_logs-203-2048 | 1.026x | 1.721x | 49118 | 823737 | fail |

Missing, malformed, stale and underinclusive hints each triggered central rebuild
and preserved the exact answers. Their phase/resource costs are retained alongside
the valid and baseline runs. These are logical file-transfer payload sizes, not
network measurements. Validating raw data centrally can erase the proposed source
work saving; changing that trust boundary would require a different contract.

## Review, limitations and decision impact

Before formal execution, independent GPT and Claude reviewers exercised the harness
and its evidence validator. Review found that a mutated stored percentile could
reverse a gate, and that external metadata and durable index publication were omitted
from cost accounting. The repaired harness recomputes percentiles, enforces the sample
grid and matched counts, validates retained file sizes/hashes, and includes the full
publication boundary. Injected percentile, missing/duplicate sample, negative/bool
timing, wrong count, source/answer mismatch and fsync-error defects were rejected.
The [review archive](data/research-costs-run-01/review/) retains failures and source-pinned
approvals; smoke trials are not mixed into formal results.

The [independent formal audit](data/research-costs-run-01/review/formal-review.md)
exited 0, rechecking all eight cells, 490 successful commands and 4,436 original
file hashes. It recomputed every gate and every query family's observations; a forged
gate with an updated hash manifest exited 1 as intended. Twenty-eight descriptive
metric spot-checks matched exactly. This audit was independent of the harness
summarizer; Claude reviewed the harness and smoke behavior before formal execution.
All 29 [measured source hashes](data/research-costs-run-01/source-pin-check.json)
matched integration before a later [import-only rustfmt adjustment](data/research-costs-run-01/rustfmt-adaptation.json).
The selected layout body is unchanged, and its post-format tests are recorded in
the final check ledger.

The [retention manifest](data/research-costs-run-01/artifact-retention.json) identifies
regenerable data files omitted from Git. The full original was audited before omission;
the retained samples and metadata remain sufficient to recalculate the reported costs.

This host is shared: start/end process lists and load averages are evidence of load,
not isolation. E3 had finished before timing; no cache drop was performed. Most reads
can hit the OS cache. `logical_data_bytes`, process read syscalls, kernel device bytes
and retained storage bytes are distinct metrics. Layout execution order alternates
between trials, but each query runs full, projected and then postings in fixed order;
this can favor later modes through cache effects. Correctness prechecks also read
the files before timed queries. CLOCK_PROCESS_CPUTIME_ID measures
phase CPU; whole-process CPU/RSS include setup and oracle checks and are reported
separately. A fresh process does not make the OS page cache cold. The resource record
does not establish device IOPS or real-network cost.

The measurements justify further compressed-layout experiments, subject to their
per-dataset gates. They do not justify migrating FOL2, weakening verification,
automatically creating indexes, or dropping raw source data. The local lifecycle
continues with inspectable S2 JSON snapshots. Hardware offload, external-system
comparisons, network OTLP, clustering, event-time partition redesign and adaptive
indexes remain explicitly conditional future work in the
[agenda](../ablation/observability-storage-research.md).
