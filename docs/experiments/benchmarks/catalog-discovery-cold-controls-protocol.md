# Cold/shared catalog discovery controls and minimal fixture

Status: additive pre-execution registration; no outcomes claimed. The original
eight-group Walk regression and its failed/passing runs stay unchanged. This
cell validates that [candidate discovery repair](catalog-discovery-race-findings.md)
does not depend on a primed tail cache or deep-copied metadata.

Selector `read_catalog::race_tests::cold_and_shared_discovery_cuts_preserve_minimal_custody`.
Four independent owned fixture states: cold clone, primed clone, cold shared,
primed shared. Each reduces seed `0xCA7A10A1` producer fixture to one Group and
one Batch (six logs, one metric, three spans). Pre-IO exact encoded Batch ledger
is constructed independently of Segment/replay output. One no-cut catalog view
and unchanged independent full-query oracle controls validate each source ledger.
The target cold catalog itself must not be refreshed/viewed before the cut;
the separate control object cannot prime its derived state. Cold means cold
derived catalog, not a flushed OS cache.

At the same private callback gap after Segment listing/before tail extend, actual
bounded builder publishes label 1 and actual Store/sealer checkpoint/reclaim
removes its journal under unlimited retention. Target acquisition must return
exact custody OR explicit Interrupted followed by an exact ordinary fresh view.
Other errors and silent missing coverage fail. Record variant, cut, error/result,
selected custody and each independent grader input/output. H1: all four variants
satisfy the existing movement/exactness contract; H0: any loses coverage or
returns unexpected error. This also preserves a one-Batch deterministic fixture.

The shared variant only changes metadata ownership. No physical IO lease,
retention entitlement or catalog semantics are modified. Production repair and
original regression expectation remain untouched by this additive test.

Root admits serially after source/protocol hashing with fresh ID
`catalog-discovery-cold-controls-01` and recovery evidence directory. Command:
`cargo test --offline --locked -p fabric-server --lib SELECTOR -- --exact
--test-threads=1 --nocapture`, within coordinator/resource launcher, 300 seconds.
All scratch is owned mounted data-drive FABRIC_SCRATCH_ROOT; existing 16/20 GiB
high/max, zero swap, 64 MiB intended scratch, 16 GiB free reserve, 16 MiB evidence.
Stop on unexpected baseline/negative outcome, missing cut, IO error, OOM,
deadline/storage ceiling or survivor. Preserve failure before cleanup, record
actual exits/resources/hashes; do not rewrite earlier failure outcomes.
No general concurrency, hot/cold disk performance, source corruption or
qualification inference follows from these four selected schedules.
