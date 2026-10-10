# O8 finite logs/metrics overlap supplement

Status: registered for the finite pilot before its builds/measurement; no pilot
outcome claimed. Corrected deterministic controls are recorded separately. The planned dev/small service cells remain conditional, not completed
by this short pilot. [Completion readiness](coupled-query-completion-readiness.md)
defines custody/query dependencies. Root alone admits contained workloads.

## Mechanism and correctness

Default collection/delivery is unchanged. A hidden experimental runtime method
starts one scoped worker owning the immutable oldest unacknowledged request;
only the main thread owns Spool, cursors, metric history, meter and configuration.
It may collect/commit one successor while that request waits, only if no successor
was already durable. Retry/backlog cannot prepare another. Worker joins before
ACK handling, observer notification precedes persisted ACK, and no successor is
sent inside the method. Future ACKs cannot become valid solely because an unsent
successor was prepared. Trace listeners are outside scope and rejected.
Stop before dispatch performs no send/preparation; an in-flight worker finishes
under existing request timeout, then retained custody is processed/drained.
Collection failure leaves its existing gap/unknown marker and unadvanced cursors;
it does not suppress an independent successful ACK. Applied pauses prevent
collection; configuration polling and metering remain on the main thread.

Six deterministic controls use the private worker callback (not mock transport
performance): durable N+1 before delayed ACK/no second send; byte-identical retry
without N+2; full Spool/cursor/unknown behavior; stop/future ACK/worker panic;
paused configuration/meter counters; rate wait past deadline/no send or prepare.
Run before the real TLS pilot, preserving every failure and exact expectations:

```sh
python3 -B tools/resource_group.py -- cargo test --offline --locked -p fabric_o11y --lib overlap_ -- --nocapture
```

## Frozen screening fixture and decision

Freeze current-working-tree native example, actual fabric-server and server_dump,
plain only; no new features. Borrowed-log compile selector 0. Record source and
decoded/archive executable hashes once; each arm uses identical frozen binaries.
One fresh development node per arm, serial then overlap, offers real file lines:
10/30/10 logs/s in three 5-second phases, exactly 250 bodies of 512 bytes, deterministic
seed 42/index identifiers with repetitive padding. Fixture host counters are fixed;
normal 15-second metric cadence and 5-second configuration polls remain active.
No traces, destructive faults, resume/restart or small 20-node result is inferred.

Actual HTTPS relay forwards exact Batch bytes to actual server; it delays only
the Batch answer 50 ms after receiving the server response. Configuration/health
are undelayed. Both arms share this artificial delay, certificate/token validation
and schedule. The server stays journal-backed at this tiny population; final
recovery still establishes custody rather than assuming ACK implies evidence.

Primary screen metric is source-offer→client-observed ACK median at matched exact
acceptance, with node CPU per accepted logical log MiB as a preservation guard.
This supplementary screen cannot nominate from one pair or establish p99/capacity.
Report all sample counts and maxima; do not reinterpret cycle count as throughput.
Spool→ACK uses the post-durable-collection timestamp before worker join and the
client attempt event. This timestamp follows collection return, not the exact
marker syscall. Visibility polling stores complete live answers every 100 ms and
first-seen intervals (previous query start/current response finish); initial lower
bound is unknown. ACK→queryable upper differences can be negative because the
server committed before the delayed ACK. No instantaneous/30-second promise.
All clocks share this host; retain wall/monotonic anchors and reject >10 ms drift.

Each arm must recover every producer Batch SHA/identity/sequence and every offered
body exactly, with no unexpected retries in this normal fixture. Three actual
complete final logs/metrics/rate chains pass unchanged query oracle, including
freshness, gaps, retained window and continuation. Changed custody and omitted/
duplicated query-row controls must reject. Retain raw producer/server ledgers,
requests, clocks, source offers, all query pages, hashes and control verdicts.
ACK, Batch/encoded-byte and source logical-byte counts are separate quantities.

## Commands, bounds and cleanup

Existing O6/O7 decisions, resource admission and the six controls gate measurement.
Root registers this protocol and source identity before executing these commands:

```sh
python3 -B tools/resource_group.py --delegate -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-overlap-freeze-01 --lab query --stage query --seconds 300 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 32 -- python3 -B tools/bench/labs/catalog/coupled_overlap.py --stage freeze --out docs/experiments/benchmarks/data/catalog-overlap-freeze-01 --seconds 240
python3 -B tools/resource_group.py --delegate -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-overlap-pair1-01 --lab query --stage query --seconds 180 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 8 -- python3 -B tools/bench/labs/catalog/coupled_overlap.py --stage pair --freeze docs/experiments/benchmarks/data/catalog-overlap-freeze-01 --out docs/experiments/benchmarks/data/catalog-overlap-pair1-01 --seconds 150
```

The driver verifies inherited containment then creates independent enforced
service cgroups: server 384 MiB maximum/320 MiB high/two CPU equivalents; node 64 MiB maximum/
48 MiB high/one CPU equivalent; both no swap and 128-task cap. Parent 20 GiB/no-swap
and 8 GiB scratch remain. Query/proxy/verifier work is outside those service groups
but inside the parent. All builds/subprocesses/requests/joins have deadlines.
Projected freeze+controls+pair <300 s within O8's 900 s execution ceiling; actual
coordinator receipts govern admission and usage. Decoded binaries and fixtures
live only under mounted FABRIC_SCRATCH_ROOT. New retained data prefix catalog-overlap-*;
proposed 32 MiB incremental evidence plus existing lab/aggregate caps requires root
admission. Failure preserves owned scratch; success terminates children/relay and
cleans only owned temporary state after exact grading. Existing evidence is untouched.
