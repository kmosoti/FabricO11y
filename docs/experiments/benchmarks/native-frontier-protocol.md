# Native ownership and query admission experiments

Prospective registration, 2026-10-09. The owner authorized continuation of the
[cross-system research](cross-system-sweep-findings.md). This round tests two
small native mechanisms suggested by source inspection and the measured
construction/retention gap. It is exploratory research, not qualification or
authorization to change production defaults, wire formats or durability order.

## Model and nominations

Peak memory depends on simultaneously live owners. In the current bounded
sealer, source rows survive successful conversion into an independently owned
Arrow RecordBatch and remain alive during Parquet encoding. Releasing the source
payloads sooner may lower peak without additional spill work. Its null is that
conversion or unrelated staging sets the peak. The memory lab compares this
ownership change against the unchanged lifetime at a fixed 8 MiB sort-run cap;
the prior run-cap confirmation remains separate evidence.

Query cost includes predicate work for records that cannot enter a page's
bounded result heap. The query lab tests cheap canonical-key/page admission
before text matching and payload cloning. Its null is that decoding dominates,
or that extra checks slow broad/low-churn queries. Every invoked source reader,
raw digest check, typed validation, snapshot boundary and final answer remains
unchanged. This changes admission work within existing readers; it introduces
no new pruning metadata or physical row ordering.

Shared signal staging remains a competing, unimplemented candidate. Inspection
shows that estimated resident-row credits do not bound decoded inputs, spare
capacity, stable-sort workspace or writer transients. Establish the simpler
ownership effect first; a shared policy needs its own spill/fan-in experiment.
No more than these two prototypes are nominated in this round.

## Resource accounting

Allocate at most 7200 additional serialized execution seconds under the unchanged
28800-second combined frontier and 86400-second original campaign. Charge all
original campaign jobs, the first cross-system continuation, the completed
matched sweep, and this new round. Historical unused round time is not a new
independent allowance. At registration, the ledger reports 9572.577377 seconds
of frontier remaining; spending the full round leaves 2372.577377 seconds.

Every workload and validator uses `python3 -B tools/resource_group.py --`, then
`tools/bench/labs/cross_system/run_native_job.py`. Keep the enforced 16/20 GiB
memory high/max, zero swap, 1800-second service deadline, readiness lock and
two Rust build jobs. Each admitted job is at most 1700 seconds. These are
containment limits, not a host-memory reservation. No remote workload is included.

Use the mounted data drive for build caches, decoder installations, frozen
binaries and scratch. Active scratch is at most 8 GiB with 16 GiB free reserve.
New retained evidence at `data/native-frontier-01` is at most 1 GiB: memory
512 MiB, query 384 MiB and coordinator 128 MiB. Existing evidence stays intact
and separately charged. Each workload reserves room for its complete bounded
failure: at most 512 MiB raw and 192 MiB compressed, within its lab allocation.
If preservation cannot fit, retain original scratch and stop; do not discard it
to admit another cell. Build failures may exceed the fixture envelope only in
the existing shared build cache, whose paths and disposition must be recorded.

## Prospective comparisons and evidence

The detailed supplements are [memory ownership](native-ownership-proposal.md)
and [query admission](native-key-first-proposal.md). Each must be registered
before its first comparison. Compile-time opt-ins leave normal defaults
unchanged. Freeze exact source files, working-tree identities, compiler flags,
binary hashes, protocol hashes, commands and seeds before running matched arms.
Use balanced arm order. Initial screens require fresh held-out confirmation
before any nomination for later integration; a failed cost guard is a result,
not permission to change the threshold. A semantic failure stops that lab and
preserves its complete fixture.

Record wall/build time, CPU, requested live heap where instrumented, whole-worker
RSS, cgroup peak/swap/OOM, bytes written, output identity, query answers and work
counters separately. Name each measurement boundary. Observer attribution and
plain timing must remain distinguishable. No rates from these finite offline
fixtures establish server throughput, ACK latency, deployment capacity or alpha
readiness. Do not multiply gains from different datasets or earlier rounds.

Memory's unchanged logical ledgers are differential controls; the isolated
Python/Arrow physical filter reconstruction is independent. Query keeps the
independent Python full-answer oracle, complete continuation chains, canonical
ties and existing corruption/missing-evidence behavior. Any new checker must
reject a representative negative control. Preserve exact failed commands and
counterexamples even if a corrected retry succeeds.

## Completion

Consolidate favorable and adverse cells, identify the mechanism supported by
the measurements, update the research queue/current state, and run the fast
verification profile plus manual documentation checks. Bun hooks and CI remain
disabled. Bind fresh receipts to the actual revision/configuration and job;
historical passing receipts cannot substitute for this round's checks.

Successful generated fixtures may be removed after their required semantic
checks and complete regular-file path/size/SHA-256 inventory. Preserve failure
bytes with authenticated readback before removal. Remove owned frozen binaries,
decoder installations and transient units at closeout; retain the shared build
cache and report it explicitly. Record ledger use, resource peaks, cleanup
outcomes and unresolved uncertainty. Research nominations remain separate from
production/default promotion.
