# Release service and fixed-query candidate protocol

Registered 2026-10-10 before candidate measurements under R3 of the
[release plan](../../milestones/release-readiness.md). This specifies B2–B5 for
the six local main trials and four fixed-query cells. Existing historical
protocols, independent delivery/query/rate oracles and thresholds are unchanged.
Cross-family networking and prolonged outages are separate cells.

## Inputs and containment

Use the exact extracted Debian package binaries and console assets, with the
source-matched uninstalled `spindle_sim`, `spool_dump` and `server_dump` helpers.
Require successful build receipts, hashes, source bundle and payload inventory.
Archive the harness, command, configuration without secrets, independent fixtures,
source manifest and before/after hashes. Never derive expected observations from
server output. Enroll through production `serve`, actual HTTPS WebAuthn and scoped
workload credentials. The virtual CTAP2 authenticator is an automation fixture;
it supplies no physical-device compatibility claim.

The launcher admits at most 6 GiB for this complete local fixture, zero swap and
1,800 seconds. Server plus its mandatory companion have a nested maximum of
4,000,000,000 bytes, high 3,000,000,000, 512 tasks and two CPU equivalents, pinned
to CPUs 0–1. Generators and browser use other CPUs. Bound the browser separately
at 1 GiB, 512 tasks and two CPU equivalents. Real edge RSS must be at most 64 MiB;
server RSS at most 2 GiB. Record their separate RSS, combined cgroup charge, CPU,
I/O, events, task counts and cleanup. Root coordinates aggregate admission within
20 GiB. Reserve 5 GiB per cell below the 100 GB laboratory ceiling. Retained
compact results are at most 50 MiB; source/fixture archives remain disk-accounted.

## Six main cells

For each of 10 and 100 simulated edge identities, run seeds `0xA11FA001`,
`0xA11FA002`, `0xA11FA003`. Use the existing simulator's exact workload: two
512-byte logs per identity/second, alternating repetitive and seeded high entropy,
and 32 integer metric points per 15 seconds. The simulator is not a durable edge.
Add one real edge collecting the same log rate and host metrics every 15 seconds,
receiving 10 independently constructed three-span traces/second over loopback
OTLP/HTTP. Trace IDs, parentage, names and timestamps are recorded before export.
Account separately for the real edge's host/diagnostic traffic and all companion
traffic. Use ordinary sealing, not experimental selectors. Warm up for 15 seconds
and measure for 120 seconds; drain at most 120 seconds afterward.

Maintain one authenticated UI in visible Live Tail with its actual five-second
polling, 200 displayed rows, one in-flight read, 8 MiB reply cap and no accumulated
page history. Record attempted/completed/failed UI requests separately. Pause and
resume once at measured seconds 40 and 50; a failed UI request is evidence, not an
omitted sample. At measured second 45, change every simulator's metric interval
to 30 seconds through the production console control boundary. The simulator's
registered offered metrics remain fixed; this transition measures configuration
receipt/application, not a different workload.

The ACK population is every Batch created during the measured interval, including
initially failed/retried attempts and companion/edge Batches. Compute simulator
creation-to-first-ACK and native source-acceptance-to-first-ACK separately and
together. Also report first-send-to-ACK and local Spool commit durations. A missing
ACK at drain is censored and fails acceptance. Do not substitute the duration of
only successful attempts for creation-to-ACK. Native timing uses same-boot
`CLOCK_MONOTONIC`; simulator Unix stamps require a sampled wall/monotonic mapping
whose offset changes by at most 1 ms during the cell. A clock step fails timing
admission rather than producing a negative or fabricated latency.

Every second, query one rotating simulator's latest log and the real trace/log
source. Join returned identity/body/time to independently recorded offers. Record
the first successful visibility and report its observation-to-response upper
bound; the polling interval stays included. Require p99 ≤ 5 seconds, no missing
probes and at least 100 post-warmup visibility samples per source population.
Require creation/acceptance-to-ACK p99 ≤ 1 second and all changed configurations
applied within 30 seconds. Sample outstanding Batch count and byte count every
five seconds. For each population, the mean of the final measured 30 seconds must
not exceed the first measured 30 seconds; final drain must be zero. Report offered,
admitted, committed, rejected and retried counts rather than a throughput alone.

After orderly shutdown, independently inspect each native Spool and all recovered
server Batches. Require the complete native source prefix, unchanged bytes and
every observed ACK present in recovery; include every recovered Batch in the
unchanged delivery oracle. Check all offered real log bodies and trace fields
against the independent producer fixture, and sampled simulator metrics/logs with
the unchanged query oracle. Retained source-prefix loss makes the cell incomplete.

## Fixed-query cells

Use 100 independently enrolled fixture identities, each with 100 ordered Batches.
Each Batch contains 50 logs and 50 cumulative integer metric points: exactly
500,000 logs plus 500,000 metrics. Bodies are 512 ASCII bytes, alternating `R`
and the existing seeded entropy function. Metric `release.counter` has five
`series` values; point `j` belongs to `j mod 5`, increases by one per successive
point in that series and has a stable start timestamp. Use a fixed 500-second
observation window ending before measurement; sequence/point coordinates determine
timestamps independently. Add 1,000 separately accounted three-span traces with
seeded IDs and explicit root/two-child parentage. Companion records are additional
and never counted toward the million-row fixture.

Run three Segment cells (one per seed) and one journal-only cell (seed 1), using
identical declared fixture construction. Segment cells use 16 MiB journal files
and wait for all closed journals to publish. Journal-only uses a 2 GiB journal
file and verifies zero Segments. The total journal limit is 4 GiB and retention is
one day/100 GB, so test telemetry cannot expire. Check modes from actual storage.

For each cell execute 20 seeded repetitions of each of these query kinds, in a
seeded shuffled order: one-source 60-second logs, rare 12-byte text token, one-source
counter history, fleet counter history over a one-second window, and one trace ID.
Use pages of at most 1,000 rows, following every continuation, and include all
pages in latency. Run the same query list before and after a production restart.
Require p99 ≤ 2 seconds for each kind and phase independently; retain every
answer/verdict, not only one representative answer. The unchanged Python query
oracle compares exact rows, ordering, cursor consistency, retained window,
freshness, gaps and completeness against independently encoded input Batches plus
the separately inspected companion Spool. Rate semantics are checked using the
unchanged rate oracle and native historical-query tests: the scoped console has
no unpaginated rate route, and this experiment does not invent one.

### Independent scoped oracle inputs

Prospective amendment, registered before fixed-query execution: the unchanged
query oracle's historical metadata describes its complete input corpus, whereas
the production console reports only the requested signal and authorized query
labels. For each query, project the independently generated corpus to the query
label intersected with the 100 authorized fixture enrollments, retain only its
requested signal payload, and preserve the original Batch identity and payload
bytes within that signal. Omit Batches without that signal. Do not filter by the
query's substring, metric name, trace ID or time range before oracle evaluation;
those selections remain the oracle's responsibility. Projection must preserve
row indices and integers above JavaScript's exact-number range.

This adapter applies only to the declared zero-gap fixture. Any gap in any
authorized fixture Batch fails admission; any unexpected response gap fails
grading. It must never silently discard a gap. A general scoped gap-only Batch
cannot be projected this way: the console can report its redacted gap while
omitting it from signal receive bounds, whereas the unchanged oracle includes
every supplied Batch in those bounds and expects original gap text. B4 therefore
makes no claim about that adapter case; ACCESS gates cover scoped gap redaction.
Companion gaps remain outside this workload's query grants and are accounted for
separately in custody checks.

Retain complete original Batches as source/hash/custody witnesses. Obtain only
`received_ns` from independently inspected recovery metadata after verifying its
identity and bytes against the generator; never obtain expected telemetry from
query answers. Record projection and original hashes. Before full execution,
tests must reject a forbidden-label or other-signal freshness leak and verify
large integer and row-index preservation using the unchanged independent decoder
and oracle.

## Checker controls and completion

Before full cells, one disposable short smoke must exercise the production access,
companion, native edge, simulator, tracing, query and cleanup paths. Its reduced
duration/counts cannot pass a full cell. Independently alter a source hash, drop
an ACKed recovery row, duplicate a recovered Batch, alter a span parent, inject a
late/censored ACK, remove a timing sample, increase final backlog, and change one
query value; each relevant checker must reject its named defect. No expected
outcome may be weakened after observing a candidate failure. Preserve failures
and register any changed measurement before a new run.
