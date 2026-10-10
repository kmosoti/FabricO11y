# Development and small: ingestion and consumer observation

Registered before execution on 2026-10-04, on `milestone/streaming-segment-output`.
This finite local observation uses the current bounded binary-spill sealer. It
extends the [earlier native pilot](dev-small-local-protocol.md), with concurrent
queries and explicit acceptance/resource accounting. It selects no production
defaults, changes no durability or answer contract, and is not qualification.

## Profiles and competing explanations

| Setting | Development | Small |
| --- | --- | --- |
| Real native Spindles | 1 | 20 |
| Aggregate ordinary / burst logs/s | 10 / 30 | 1,000 / 3,000 |
| Ordinary / burst / recovery | 60 / 60 / 60 s | 60 / 60 / 60 s |
| Expected source logs | 3,000 | 300,000 |
| Server CPU placement | two logical CPUs | two logical CPUs |
| Spindles, producer and client placement | two other logical CPUs | two other logical CPUs |
| Seal workers / journal file | 1 / 64 MiB | 1 / 64 MiB |
| Journal / retained Segment ceiling | 512 / 512 MiB | 512 / 512 MiB |
| Each Spool ceiling | 256 MiB | 256 MiB |
| Query plan | default Scan | default Scan |

Use the earlier pilot's exact seed 2703163393, 900-byte bodies, 100 ms producer
ticks, alternating repetitive/seeded entropy, host metrics every 15 seconds,
TLS loopback, five-second settling and twenty-second drain. Fresh state, one
trial per profile, development before small. Do not lower the development seal
threshold: an unsealed development result cannot establish lifetime memory.
No spans, outages, retention eviction, remote hosts or maximum-capacity search.

H1: at these offered loads, exact custody and timely delivery coexist with the
consumer workload and bounded observed resources. H0: any existing pilot gate
fails, input generation falls behind, queries fail, or visibility/backlog fails
to catch up. Alternative explanations include source polling, query tail
decoding, CPU/IO contention and observer overhead. CPU, disk and queue evidence
must accompany latency; a smaller builder heap alone cannot choose a profile.

## Consumers and clocks

One serial client offers one fresh query each second, rotating recent logs
(last 10 seconds, limit 50), absent text over the retained trial, and CPU metric
history (limit 50). Record scheduled/start/end times, response bytes, HTTP
errors, full answer and query. No overlapping requests in this client; late
requests are recorded and the next schedule advances without catch-up bursts.

A second serial client checks one uniquely tagged source log every five
seconds, rotating nodes. Poll every 250 ms until that exact row is returned or
30 seconds elapse. Retain every request/answer and unresolved target. Record
source-write, successful local Spool cycle stdout observation, server receive,
server ACK stdout observation and first exact response clocks. The latter is a
polling/HTTP upper bound on visibility, not the internal publication instant.
An answer may precede ACK observation; keep signed differences. The 30-second
headroom is an experimental observation threshold, not a deliberate delay or
new product freshness promise. Source-write is the time after write returns,
not an application fsync. Collection time remains the native cycle timestamp.

After node shutdown, collect complete snapshot page chains for a small tagged
log range (limit 7), an absent text search, and CPU metric history. Grade these
against the unchanged independent Python query oracle using fresh recovered
Batches. Verify returned live log rows against independent decoded rows; the
live workload does not claim full concurrent snapshot-envelope oracle grading.
Reject missing/changed/duplicate source data as before; inject changed/missing
query rows as negative controls. Keep invalid or timed-out results in counts.

## Dimensions and measurement boundaries

- **Demand and acceptance:** actual source logs/s and bytes/s, producer lateness;
  unique successful Spool commits/s, committed logs/s, encoded Batch bytes/s;
  send attempts/s, unique server ACKs/s and bytes/s, non-ACK statuses/retries.
  Join exact recovered Batch sizes by node/sequence. A cycle line follows
  successful `Spool::append`; it is not the exact internal sync clock. ACK
  stdout follows the server answer and precedes local ACK-cursor persistence.
  Report an ACK/attempt fraction, never infer a Spool attempt acceptance ratio
  from success-only cycle output. Spool append failures have no complete
  attempt counter in this screen; preserve native stderr and child exits.
- **CPU:** server, individual and aggregate Spindles, producer/client observer;
  user/system seconds, mean core equivalents and maximum sampled interval.
  Report ordinary, burst, recovery, drain and whole monitored windows. One
  core equivalent is CPU-seconds/wall-second; CPU affinity is not a reservation.
- **Memory:** sampled RSS and kernel process high-water mark, aggregate Spindle
  RSS; actual workload cgroup current/peak, anon/file/kernel breakdown,
  memory events, swap and pressure. RSS, allocator heap and page cache are
  distinct. Do not infer the earlier 97% builder-heap saving from native RSS.
- **IO/storage:** process logical and physical byte counters, cgroup IO and
  pressure, logical disk footprint, journal/Spool/Segment bytes, pending sealed
  count, Segment publications and reclaim. No fsync latency attribution or
  total heap allocation attribution without separate native instrumentation.
- **Queues:** observed unread source bytes, committed-minus-ACKed batches,
  Spool occupancy, sealed-file backlog and drain progress. Once/second disk
  samples can miss transient peaks; tolerate files renamed during inventory.
- **Consumers:** request-weighted p50/p99/max latency per shape and source phase,
  actual completed/offered requests and lateness; sampled source-to-visible,
  Spool-observed-to-visible and ACK-observed-to-visible. Keep sample counts,
  errors and missing targets; do not substitute latency for freshness.
- **Correctness/reproducibility:** exact source hashes, native ACK/recovered
  Batch equality, contiguous sequences, gaps, independent query verdicts,
  binaries/source snapshot, toolchain/host/storage and command/exit receipts.

Use nearest-rank percentiles and sample adjacent `/proc` CPU/IO deltas at 1 s.
Timestamp rate events into half-open ordinary/burst/recovery/drain windows;
retain per-second buckets so phase averages cannot conceal a burst. Sample
realtime-minus-monotonic and flag range over 5 ms. No pooled confidence claim
from one trial, no causal speed comparison to older runs on another host.

## Decision and containment

Retain the earlier pilot's exact delivery gates: no data/custody errors or gaps,
clean child exits, collection-to-receive and collection-to-ACK-observed p99 at
most 1 s, server RSS at most 2 GiB, each Spindle at most 64 MiB, clock offset
range at most 5 ms. Additional screen criteria: producer lateness p99 at most
100 ms; no HTTP errors or unresolved visibility targets; each sentinel visible
within 30 s of source write; successful independent final query checks and
negative controls. Report consumer p99 above 1 s as a responsiveness concern,
not grounds to weaken delivery/correctness gates. No claim of sustainable
capacity or a minimum RAM recommendation from one finite screen.

Every build, harness and validator uses `python3 tools/resource_group.py -- …`:
16 GiB memory high, 20 GiB max, zero swap, 30-minute deadline. Per-trial wall
limit 900 s, scratch/analysis limit 4 GiB, free-drive reserve 4 GiB; monitor each
second. Server address-space cap 4 GiB, each node 512 MiB. Disk-backed owned
scratch/builds stay under `/run/media/kmosoti/data/FabricO11y`. Archive small raw
observations, source/Batch hashes, configs with credentials omitted, failures,
commands and cleanup receipts. Remove successful raw state, credentials and
temporary fixtures after recovery and grading; retain failed state on the data
drive. Never delete shared build caches. Run fast and manual documentation
checks, retain their actual exits. Freeze protocol changes in separate commits
before new measurements; preserve any failed attempt rather than overwriting.

The [harness](../../../tools/bench/run_dev_small.py) supplies the native lifecycle;
an optional observer and a separate runner extend measurement only. Existing
oracles and runtime semantics remain unchanged. The [consumer study](../../research/development-moderate-plan.md)
remains broader than these two finite profiles.
