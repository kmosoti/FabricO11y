# O8 short small-profile fixed-demand pair

Status: registered before execution; fresh resource admission is required. Uses the existing frozen
O8 native node/server/dump binaries, no new Rust/source-default/oracle changes.
Separate `coupled_overlap_small.py` avoids changing a running/frozen pilot driver.
The helper's execution hash is retained as well as all decoded/archive hashes.

Twenty fresh distinct labels `node00` through `node19` have private source,
Spool, token, configuration, fixture-host and producer-ledger directories.
Every node runs serial or overlap for20 seconds; both arms use the same50 ms
real TLS relay delay,15-second metrics and5-second configuration polls. The
fixture relay has an explicit128-connection listen backlog for20 clients;
this is harness configuration, not a server change.
No traces. Spool8 MiB per node. After all nodes spawn, deterministic seed42
512-byte ASCII bodies are offered at50/150/50 logs/s per node in three five-second
phases,100 ms ticks: aggregate1000/3000/1000 logs/s, exactly1250 per node and25,000
total. Startup must not exhaust the five-second quiet drain allowance; a stopped
node/missing body/final backlog fails rather than shortening the schedule.

Primary source-offer→client ACK median and node CPU per accepted logical MiB are
reported alongside post-durable-Spool→ACK and source→Spool samples. One pair
cannot nominate or establish sustained capacity/p99. Actual preparation count
and per-node sequence list derive `commit(N) < ACK(N-1)` timestamps; zero
candidate exposures makes the mechanism result inconclusive. Config/host clocks
share this host; wall-monotonic drift over10 ms rejects timing interpretation.

Custody uses full `(node_id,generation,sequence)` keys, never sequence alone.
Require20 distinct Strands, contiguous producer sequences per node, exact raw
producer/recovery/request SHA equality, every offered body under its actual
recovered node label, all client ACK/commit associations and final caught-up
cursors. Normal request count equals unique producer count; non200 response or
retry fails this fixed-demand screen. Collection errors are retained; no refused
byte count is inferred from errors. Logical body bytes, encoded bytes and ACK
counts remain separate. Default runtime stop/drain semantics are unchanged.

Three final complete logs/metrics/rate chains are graded by unchanged query
oracle. Logs/metrics limit1000; Rate has no limit/page fields. All25,000 log rows
are graded; omission/duplication answer controls and changed custody reject.
Actual server_dump replays after graceful server stop. Fresh node identities
and fixture metrics are accounted by each arm's actual recovery ledger.

Visibility observes only60 named sentinel bodies: each node's source indices
0,250,1000 (first body of each phase). Round-robin polls at least50 ms apart;
node+contains filter and limit1 retain exact raw query/answer/time. The interval
is that sentinel's previous query start/current answer finish; initial lower
bound is unknown. All60 must be seen. No claim about visibility of unpolled
bodies or a contractual30-second limit. Artificial answer delay permits a
negative ACK→visible upper difference. Full final oracle remains comprehensive.

Verified child limits reuse the established small-profile shape: server-host
3328 MiB max/2816 MiB high/four CPU equivalents; server3072/2560 MiB/two CPU;
node256/192 MiB/one CPU each, no swap,128 node tasks. Node00 shares the
server-host parent; others are independent under the unchanged outer containment.
One-second observations retain CPU/IO/cgroup memory/events, sampled process
RSS/HWM and Spool file sizes. Sampled peaks are lower bounds; cgroup memory.peak
is not RSS, kernel IO is not payload throughput. Query/proxy/grading work is
inside outer containment and outside individual service groups.

No build. All scratch and decoded binaries live under mounted
FABRIC_SCRATCH_ROOT; fresh evidence prefix `catalog-overlap-small-*`.
Each arm is graded and saved incrementally. Exact gzip JSON/JSONL archives retain
source offers, raw producer/recovery, requests, full answers and sentinel polls;
no semantic normalization. Forecast8–12 MiB retained, reserve16 MiB; local16 MiB
evidence cap plus campaign admission both apply. Failure preserves owned scratch
and raw observations for root archival; success terminates workers/relay and
cleans owned scratch. Expected execution under150 seconds is a forecast, not a
result; internal170-second/outer200-second deadline and cumulative budgets apply.

```sh
python3 -B tools/resource_group.py --delegate -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-overlap-small-pair1-01 --lab query --stage query --seconds 200 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 16 -- python3 -B tools/bench/labs/catalog/coupled_overlap_small.py --freeze docs/experiments/benchmarks/data/catalog-overlap-freeze-01 --out docs/experiments/benchmarks/data/catalog-overlap-small-pair1-01 --seconds 170
```
