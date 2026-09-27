# Independent formal cost-results audit

VERDICT: APPROVE. The retained formal output passed an independent recomputation of all eight registered cells; this approves the integrity and stated boundaries of this experiment, not a storage-format migration.

The runner's `run-01.exit.json` records exit **0**. `metadata.json` says `complete`; all **490** child commands have exit **0**. The independent auditor verified **4,436** preserved file hashes and the six harness-source hashes recorded at run start. It checked one warmup and five measured trials for each cell, including source and answer bindings, unique query grids, matched counts, p50/p99 fields, retained table/anchor/index bytes and digests, sidecar fallbacks, and receipt page totals. All eight predicate families were included for every query mode. The full recomputation is [formal-audit.json](formal-audit.json).

Recomputed registered gates: `plain64` **0/8**, `zstd64` **8/8**, `zstd256` **8/8**. Rare-token posting savings are positive in all **24** layout/index cells; observed build-plus-durable-publication break-even ranges from **5 to 25** rare queries. The sidecar server CPU condition passes **1/6**, while the source-plus-server CPU condition passes **0/6**, so the registered joint sidecar benefit passes **0/6**. Receipt costs have no registered speed gate; the audit retains ratios for all eight families across the six 2,048-event datasets.

The auditor itself was challenged with a scratch mirror of the formal output. I flipped `mixed-201-2048` `zstd64.registered_layout_gate` to false and updated the mirror's SHA-256 manifest to match. The audit exited **1**, reporting `False != True` from the raw bytes and query timings. The original `run-01` was untouched.

Executed commands and exits:

| Command | Exit |
| --- | ---: |
| `python3 -B target/cost-final-review/audit.py target/research-costs/run-01 target/cost-final-review/formal-audit.json --root-exit 0` | 0 |
| Same auditor against `target/cost-final-review/fake-gate` with the altered gate and matching manifest | 1, expected rejection |
| Independent 28-field spot-check of `target/cost-analysis/metrics.json` against raw mixed-201 trial files | 0 |

The result record's interpretation matches the checked boundaries in the spot-check: query p50/p99 are medians of each trial's nearest-rank percentile; phase CPU uses the phase timer; whole-process CPU includes setup and oracle checks. The result document keeps the complete source JSON outside the layout-byte gate, counts external trust metadata, and treats kernel device-byte counters separately from logical file sizes. Its sampled table entries agree with recomputed gates and byte ratios.

Limits: these are synthetic, repetitive S1 inputs on one shared WSL2/ext4 host with likely cached reads and fixed per-query mode order. The run does not establish a production speed winner, physical cold-cache I/O, network transfer, or device durability. This audit did not independently rerun the E3 correctness corpus or have Claude review the formal measurements.

VERDICT: APPROVE
