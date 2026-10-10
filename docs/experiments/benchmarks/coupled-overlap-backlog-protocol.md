# O8 preloaded-backlog discriminator

Status: registered before execution, supplementing [reclamation registration](coupled-reclamation-protocol.md). This fixture uses the
same frozen native node/server/dump binaries as the finite pilot; it changes only
the opt-in Python `--profile backlog` schedule. The default remains `lightpilot`.
No production default, wire, oracle, durability or runtime mechanism changes.

Each serial/overlap arm preloads 512 unique seed42/index bodies of exactly 4000
ASCII bytes plus newline into its private source file before node startup.
The source closes before spawn. Exactly 2,048,000 logical body bytes exceed the
950,272-byte maximum collection budget, including per-line encoding charges;
each valid line is below the 4096-byte source limit. An 8 MiB Spool admits both
current and prepared successor. Actual node duration remains 20 seconds,
metrics interval15 seconds, configuration polling5 seconds and TLS relay delay
50 ms. No ongoing offers or settling period consumes the backlog before startup.

H1: this bounded candidate exposes successor preparation and reduces matched
source-offer-to-ACK latency without worsening CPU per accepted logical MiB.
H0 remains compatible if timing/CPU do not improve. A candidate with zero actual
`commit(N) < successful client ACK(N-1)` transitions is explicitly inconclusive,
even if overall timing changes. Counts and sequence lists are retained. One pair
cannot nominate, measure capacity or support p99. Preload latency includes node
startup; post-durable-Spool-to-ACK samples remain separately reported.

Unchanged complete source/body/Batch hash/identity/sequence recovery, exact ACK
association and full logs/metrics/rate query-oracle checks apply. Missing and
duplicate answer controls and changed custody control remain. Rate has no limit
field. Visibility polls retain all512 rows and actual interval bounds; negative
ACK-to-visible differences are possible with the deliberately delayed answer.
Final ACK catches up; no unexpected retry or partial acceptance is admitted.

One-second observations retain per-service cgroup CPU/IO/memory events and peak,
process sampled RSS/HWM and sampled total Spool file bytes. Sampled RSS/Spool
peaks are lower bounds; memory.peak is cgroup memory, not process RSS. IO bytes
are kernel accounting, not logical payload. Collection/delivery errors and
request status counts are retained; refused byte counts have no separate native
instrumentation and are not inferred from error messages. Exact accepted body
bytes, encoded Batch bytes and ACK counts remain separate metrics.

Bounds match the pilot: server384 MiB/node64 MiB max, high320/48 MiB, CPU2/1,
no swap,128 tasks each, inherited parent containment and mounted scratch.
Existing frozen hashes are verified; execution-driver hash records this amendment.
No build is required. Native requests/joins, process waits and visibility joins
remain bounded; success cleans owned scratch and failure preserves evidence.
Large source/final-answer/visibility-association JSON files are gzip archived
without normalization; other artifacts and all live visibility answers remain.
Forecast incremental retained evidence8 MiB, conservative reservation16 MiB;
existing32 MiB pilot-prefix aggregate guard and campaign cap still apply.
Root must admit actual remaining room before launch. Raw visibility generation
may produce approximately400 MiB uncompressed per arm, streamed into gzip.

```sh
python3 -B tools/resource_group.py --delegate -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-overlap-backlog-pair1-01 --lab query --stage query --seconds 180 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 16 -- python3 -B tools/bench/labs/catalog/coupled_overlap.py --stage pair --profile backlog --freeze docs/experiments/benchmarks/data/catalog-overlap-freeze-01 --out docs/experiments/benchmarks/data/catalog-overlap-backlog-pair1-01 --seconds 150
```

Expected pair execution under90 seconds is a forecast. Actual receipts govern
the cumulative O8/global workload budget. No small20-node result is inferred.
