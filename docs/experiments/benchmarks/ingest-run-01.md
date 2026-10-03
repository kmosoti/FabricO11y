# Ingest run 01: server ingest on this machine with parallel sealing

Status: **Exploratory.** Measured on 2026-10-03 for [ADR-0025](../../decisions/ADR-0025-carry-traces-as-a-third-signal.md) on a 4-CPU, 15 GiB Ubuntu 24.04 container. No protocol was registered; nothing here is **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense.

## Setup

The product server (release build) on CPUs 0 to 2 with `seal_workers` 1, 2 and 3, a 1 GiB journal of 64 MiB files and 512 MiB of Segment retention (the disk allowed no more). The load generator `ingest_load` ([example](../../../crates/fabric-server/examples/ingest_load.rs)) on CPU 3: 32 enrolled nodes, one thread each, one Batch in flight per node as a Spindle sends, each Batch about 512 KiB of real log lines from the research corpus plus three spans, over TLS through the Spindle's own sender. 30 s of load per configuration, then the time for sealing to drain ([ingest.json](data/ingest/ingest.json), [ingest.py](data/ingest/ingest.py.txt)).

## Results

| Sealing workers | Ingest, MB/s | Batches | Batch latency p50 / p99, ms | Refused sends (journal full) | Server CPU, s | Server peak RSS, MiB | Sealed files waiting, max | Drain after load, s |
| ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 48.4 | 2,462 | 119 / 169 | 6,190 | 59.5 | 584 | 15 | 13.0 |
| 2 | 74.9 | 3,806 | 137 / 193 | 4,022 | 83.6 | 976 | 15 | 12.5 |
| 3 | 80.5 | 4,092 | 153 / 239 | 2,838 | 87.6 | 1,564 | 15 | 6.0 |

## Verdict

**Sealing is the server's ingest bottleneck on this machine, and parallel sealing lifts it 1.55× at two workers and 1.66× at three.** Every configuration filled the 1 GiB journal (15 sealed 64 MiB files waiting) and leaned on back-pressure: the server refused Batches with "journal full" until sealing freed space, and the senders retried. Custody held throughout: a refused Batch is not acknowledged and stays in its sender. Sustained ingest rose from 48 MB/s with one worker to 75 MB/s with two and 81 MB/s with three, where the server used about three cores' worth of CPU over the run on its three CPUs: it is CPU-bound, and the third worker competes with the commit path and TLS for the same cores. Peak memory grows with the workers (584 to 1,564 MiB) because today's Segment build holds about 5.5 times its file; [ADR-0022](../../decisions/ADR-0022-build-segments-by-external-merge-sort.md)'s flat-memory build is what makes more workers safe.

Against the 500 MB/s projection for 500 hosts: this machine's three server cores sustain about 80 MB/s of real-text ingest with sealing, so the projection needs about six times the sealing throughput (more cores with more workers, and a cheaper build), as projected.

## Limits

- One machine, one run per configuration, 30 s each; the disk cap forced a 1 GiB journal, so back-pressure set in within seconds and the figures are sustained-with-back-pressure rates, not burst rates.
- The load generator shared the machine (one CPU), and its own TLS and encoding cost may cap the highest figure.
- Senders bypass the Spool; a Spindle's Spool commit adds its own cost on the sending host, not on the server.
