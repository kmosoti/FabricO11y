# Spindle run 01: one node's collection and delivery on this machine

Status: **Exploratory.** Measured on 2026-10-03 for [ADR-0025](../../decisions/ADR-0025-carry-traces-as-a-third-signal.md) on a 4-CPU, 15 GiB Ubuntu 24.04 container. No protocol was registered; nothing here is **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense.

## Setup

The release `fabric-node` pinned to CPU 3 drains a pre-filled log of real text (the research corpus repeated: 130 bytes per line on average), with its metric interval at one hour so the log dominates. Three modes, 60 s each ([spindle_bench.json](data/spindle/spindle_bench.json), [spindle_bench.py](data/spindle/spindle_bench.py.txt)):

1. **collect only**: no server, 64 MiB of log;
2. **deliver**: to the release server on CPUs 0 to 2 (`seal_workers=2`), 256 MiB of log;
3. **deliver under a cap**: the same with `max_output_bytes_per_s` at 4 MiB/s.

Rates are log-text bytes consumed per second of busy time; the node's CPU and peak memory come from `/proc`.

## Results

| Mode | Log text, MB/s | Lines/s | Batches | Node CPU, share of one core | Node peak RSS, MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| collect only | 30.7 | 235,097 | 246 in 2.2 s | 0.03 over 60 s (1.8 s of CPU) | 9 |
| deliver | 3.63 | 27,853 | 779 in 60 s | 0.15 | 11 |
| deliver, capped at 4 MiB/s | 1.57 | 12,051 | 337 in 60 s | 0.07 | 11 |

Before the fixes below, the same deliver mode reached 3.21 MB/s (Batches of 768 KiB counted budget) and, before them, collection exited on a full Spool.

## Verdict

1. **Collection is cheap; delivery sets one node's ceiling.** A node collects about 31 MB/s of real log text on a fraction of one core (about 38 MB/s per core of CPU). Delivering, it is limited to 3.6 MB/s of text by the delivery rule (one Batch in flight per Strand) and the server's 50 ms group commit window: 13 Batches per second, 77 ms each. Its CPU stays at 15% of a core and its memory at 11 MiB.
2. **The encoding is 2.7 times the text.** Under the cap, 337 Batches carried 94 MB of text in 4 MiB/s × 60 s of encoded Batches: about 746 KB encoded for 280 KB of text per Batch. Each line's record repeats its file path, device, inode and offsets as attributes; on 130-byte lines that is more than the line.
3. **The cap holds.** At 4 MiB/s the delivered rate is 4 MiB/s of Batch bytes, half the uncapped rate, and the node's CPU halves with it.
4. **Two defects found and fixed in this run.** The Spool rotates only before a Batch carrying host metrics (so every retained file starts with counter state); a log-heavy node with a long metric interval therefore grew one Spool file without bound and could not reclaim acknowledged bytes, and filled its Spool. The node now samples host metrics whenever rotation is due (test `a_log_heavy_node_rotates_its_spool_between_metric_intervals`, which fails without the fix). And catch-up passes now wait for delivery to have caught up, so collection cannot outrun what the server accepts. Filling Batches to the cap less the reserve (928 KiB counted) took delivery from 3.21 to 3.63 MB/s.

## Limits

- One node, one server, one machine; the server shared the machine (three CPUs).
- The text is the research corpus repeated; a host with longer lines has a smaller encoding ratio and a higher text rate at the same Batch rate.
- Encoded Batch sizes are inferred from the cap run, not read from the meter.
