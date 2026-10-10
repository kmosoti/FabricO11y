# Proposed bounded-builder acceptance interpretation

Status: prepared before execution; not registered, built or validated. Root must
review and commit this trust-boundary interpretation separately before measuring.
The original readiness manifest mismatch remains failed and archived unchanged.

The authoritative workload is the registered bounded-sealer protocol and the
study driver's generator: seed `0xA11FA001`, 100 nodes, two logs/node-second,
32 named gauge metrics every 15 seconds, per-node sequences, gap every 500th
Batch, twenty commit groups/second, deterministic receive jitter. Outage nodes
0–19 buffer seconds [120,360), then send ten backlog batches plus their current
batch each second. Adversarial observation timestamps lie in a 520-second window.
Big rows use 16384-byte bodies; other shapes use 512. There are no spans. Stop
after the first journal file reaches the requested frame-inclusive threshold;
retain the original generator's complete-group overshoot, RNG order and rotation.
The historical driver is evidence for fixture reproduction, not product code.

Run four shapes at 64 MiB and steady at 16/32/128/256 MiB. Each builder runs
three times in a separate process over identical fixture bytes. The reference
is current `segment::read_sealed` plus legacy `segment::build`; the candidate is
current `segment::build_sealed`. Build measurement includes reference decoding.
Independent heap snapshots exclude fixture generation and post-build readback.
This is a differential oracle with shared readers, not independent query-oracle
coverage. BS-2/4/5/7/8/9 remain separate obligations.

Manifest semantic fields, file names and **logical table** row counts must match.
File SHA-256 and encoded byte length describe representation and are reported;
byte equality of logs/metrics remains gated wherever byte caps do not close a
row group early. `text_filter.bin.rows` counts physical Parquet groups, not logs.
It may differ only when logs grouping differs, and each layout must independently
have exactly one authenticated filter per actual group, zero false negatives
over every body's trigrams and matching decoded group row counts. Missing-filter
and cleared-filter controls must be rejected. This exception does not cover any
logical table count, other metadata, missing files or optional index corruption.

Pruning remains strict: compare integer matched/read row totals for the study's
sliding 60- and 10-second windows, half-window steps, over identical sorted
timestamp populations; report group counts and all windows. Different physical
grouping may **fail** pruning equivalence. Do not waive that outcome or silently
reinterpret it as improvement. The 80 MiB heap and 10% 64→256 scale gates remain.
Determinism compares all file hashes within each builder across its three runs.

Exact cumulative spill bytes are unavailable through the existing public API.
Record them as null with the reason, alongside build-window `/proc/self/io`
counter deltas and final file sizes; never call write characters minus output
bytes exact spill. This missing reported metric makes full registered measurement
coverage incomplete even if every implemented gate passes. Root can add separate
non-product tracing under a new frozen measurement boundary or retain this limit.

`builder.py --controls` checks rejection of altered rows, logical counts, filter
alignment, pruning, heap, leftovers, non-applicable byte differences and scale.
Actual filter controls run during Rust readback. These controls need execution
before full cells. Commands are all wrapped by the resource launcher; scratch is
owned under `FABRIC_SCRATCH_ROOT`; measured trials are never shortened after launch.
On the first failed implemented gate stop dependent cells, archive compact JSON
and preserve owned failure state. No retry overwrites a previous destination.
