# Q4 descriptor reuse controls

Status: registered before controls. Candidate is an opt-in immutable descriptor
container, with shared Manifest ownership on both timing arms. Default discovery
remains owned/lazy. No persistent format, oracle or query contract changes.
The [preparation](coupled-query-preparation.md) describes the mechanism.

Candidate checks fresh published labels before acquisition and after tail
extension; hits do not skip the original discovery cut. Selected descriptors
retain query frontier/floor filtering and ordering. At most four cache snapshot
holders, four retired containers and one current container are admitted, with
fallback to lazy discovery. Structural limits are 256 descriptor/vector capacities,
256 entries per Manifest map and 4,096 bytes per string/path capacity. An 8 MiB
target-stdlib charge model additionally gates reuse; it is not a proven allocator
or server RSS bound. Shared manifests are conservatively recharged per generation.
Metadata does not pin files or authorize expired pages. No claim against arbitrary
same-label mutation or label reuse is added.

H1: selected candidate lifecycle schedules preserve coverage, explicit retry,
retention and release. H0: one schedule changes exact answers or leaks a holder.
Five additive controls exercise cache hit during publication/reclaim, unchanged
Segment labels with appended journal data, structural/charged-byte overflow and
stale/drop behavior, real retention with Gone, and actual task abort/join release.
Changed/missing/duplicated ledger controls must be explicitly rejected; assertion
inequality alone is not independent semantic grading.

Exact initial command, 8 MiB aggregate reserve, no timing nomination:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-q4-controls-01 --lab query --stage query --seconds 300 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 8 -- cargo test --offline --locked -p fabric-server --lib --all-features coupled_ -- --nocapture
```

Stop timing admission on failure; preserve trace and use a fresh ID after a
causal repair, unchanged expectations. Optional-index fallback and full-chain
integration must also exercise the candidate before any performance screen.
Source archive and exact test counts, CPU/RSS/cgroup observations and cleanup
belong in the run receipt. Performance fixture changes need a separate registered
supplement; no original CQ2 result is replaced.
