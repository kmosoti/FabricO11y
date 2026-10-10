# Native RCA journal pilot protocol

Registered scope: execute one prepared RCA case through the real Fabric server
and unchanged independent query oracle. The owner authorized proceeding from
the [application use-case plan](../../research/application-use-cases.md).
This is finite native evidence preparation for investigations, not a causal
engine, deployment qualification, browser adapter or general metric receiver.
Register this protocol separately before executing the driver.

## Claim, inputs and boundaries

H1: the late-span fixture is delivered exactly, a held initial page chain remains
at its original population, fresh queries incorporate the late Batch, and a
graceful restart preserves the same answers. H0: a query substitutes, omits or
alters evidence, incorporates late rows into the held snapshot, or changes after
restart. Correctness is a gate; no latency or memory improvement can offset a
failure. Query timing here is diagnostic, with too few samples for a service SLO.

Use only `rca-05` from the authenticated
`data/hammer-reference-01/query/rca-preparation-01/packet` bundle and plan `walk`.
Two initial Batches contain the API and dependency observations; the third
Batch supplies the delayed child span. Replay an initial Batch with identical
identity and bytes to exercise deduplication. No seal padding is admitted:
these small inputs exercise journal-backed queries, not published Segments.
Do not change the fixture, independent oracle, wire format, sync order or Rust.

The binary/source binding is
`data/hammer-reference-01/memory/identity-build-01/build/build.json`, using the
retained release server and `examples/server_dump`. Verify complete Rust and
manifest source membership and hashes plus binary hashes before starting.
Archive the driver and invoked helper hashes, command, environment and outcomes.

## Procedure and acceptance

1. Start an isolated TLS server and enroll the two fixture identities through
   existing interfaces. Record the exact producer bytes/identities and actual
   HTTP acceptance/ACK responses and local monotonic/wall-clock observations.
2. Deliver the two initial Batches and replay the same initial identity/bytes.
   Capture all pages for the eight registered queries. Separately hold the first
   page of an initial trace query with its snapshot/page token.
3. Deliver the delayed Batch. Complete the held chain, which must still contain
   exactly the initial two spans. Fresh trace queries must contain three. Run
   all eight full query chains for the fresh after-late population.
4. Stop the server gracefully and obtain a fresh-process durable dump. Require
   exact producer/recovered identity and bytes with no duplicate logical commit.
   Use actual persisted receive timestamps for oracle inputs, never invented
   receive/ACK observations. Compare initial chains to the initial population,
   fresh after-late chains to all three records.
5. Restart the same server state, run all eight full chains again, stop it and
   verify durable recovery again. Every native chain must match the unchanged
   independent query oracle, including relevant envelope and page invariants.

Run three actual rejection controls through the unchanged oracle: omit a
matching row, alter a metric value, and include the late child in an initially
held chain. Each must produce a nonempty violated-check result. Do not change
the oracle's expected outcomes or pass a claimed rejection without executing it.
Case fixture labels and controller truth are not a blinded investigator trial;
no interpretation/causality success rate is measured.

Cap each phase at 32 query HTTP requests, including the held-chain requests in
their respective phase. Cap total requests, response bytes and retained evidence
in the driver. Report query counts/elapsed time/response bytes and per-request
latency; send-to-ACK and ACK-to-first-observed-query are different measurements.
The latter is an observation upper bound, not the precise publication instant.
Capture process CPU/RSS, cgroup peak/events/swap, disk use, server exits and
cleanup. Preserve errors and failed controls without relabelling the attempt.

## Admission and containment

Use the existing pressure ledger: 7,200 s round, 36,000 s frontier and 86,400 s
campaign, all earlier charges preserved. No new allocation. Admit this pilot
for 180 s and reserve 32 MiB query evidence only after the previous pressure
workload stops. The driver has a 160 s deadline including a cleanup reserve.
Live owned fixture scratch is at most 8 MiB; response/retained evidence must fit
the reservation. Stop instead of silently expanding any bound.

Run beneath `python3 -B tools/resource_group.py --delegate -- ...`: parent memory
high/max 16/20 GiB, no swap; server memory high/max page-aligned down from 3/4
decimal GB, CPU quota two equivalents, tasks 256 and no swap. Descendants enter
the enforced group before executing project code. Scratch stays under the
launcher's `FABRIC_SCRATCH_ROOT` on the mounted data drive. The owner storage
ceiling remains 100 GB. No remote work or uncontained fallback.

The native wrapper points the existing ledger at this protocol; it does not
reset budgets. Planned command:

```sh
python3 -B tools/resource_group.py --delegate -- \
  python3 -B tools/bench/labs/rca/native_job.py \
  --id rca-native-01 --lab query --seconds 180 --reserve-mib 32 -- \
  python3 -B tools/bench/labs/rca/native.py \
  --out docs/experiments/benchmarks/data/hammer-reference-01/query/rca-native-01
```

Authenticate retained evidence before removing owned scratch, stop all owned
processes and remove empty child cgroups. Failure evidence must be preserved
before cleanup, with resource observations and unresolved limitations retained.
If a failure requires changed acceptance, register a new protocol before retry;
implementation fixes preserving the same checks need a fresh run identifier.

Only after the pilot's controls and resource checks pass may a separately
registered follow-up expand cases, plans, publication or application adapters.
Manual documentation checks remain bounded and do not enable hooks or CI.
