# Publication/reclaim between source discovery steps

Status: registered **before execution**; no reproduced
failure claimed. Scope is one deterministic existing-storage counterexample
schedule. Related [paused cuts](catalog-paused-cuts-protocol.md) do not cover this
window. Source inspection nominated a race; this cell must distinguish it.
Independent query oracle, product contract and durability order remain unchanged.

## Hypothesis and real schedule

H1: one catalog acquisition returns exact committed custody coverage OR explicit
`io::ErrorKind::Interrupted` movement followed by exact fresh acquisition, when
publication/reclaim occurs between Segment and journal discovery.
H0: that acquisition omits committed identities or falsely treats coverage as
empty. Expected behavior is H1; an assertion failure confirms this schedule's
counterexample and stays failed until a new source revision is independently run.

Construct one journal with eight committed Groups, three nodes per group, seed
`0xCA7A10A1`, explicit exact Batch bytes and independently constructed row/receive
ledger before IO. Rotate it to owned pending label 1; active journal remains empty.
Keep retention unlimited so intentional eviction cannot explain missing coverage.
Obtain a baseline catalog view and require all expected raw custody identities.

Use a private test callback at precisely this cut: after catalog Segment listing
has selected coverage, before tail `extend` discovers journal files. During that
callback invoke the actual bounded builder, then open actual Store/run actual
sealer to checkpoint/reclaim the journal. Assert Segment 1 durably published,
sealed journal absent and unchanged expected source ledger. Continue the SAME
catalog acquisition, then independently enumerate its selected custody from
Segment tables, eager Groups and deferred tail entries. Compare exact identity,
encoded bytes and multiplicity against the pre-IO ledger. Silent `Ok` with missing
custody must fail without silently requerying. Explicit `Interrupted` is safe
under History's existing three-attempt retry contract: archive that error and
require a fresh catalog view to match exact pre-IO custody. Other errors fail.
This distinction is frozen before measurement, not changed after a failure.

If the gap view omits sources, record its selected Segment labels, oldest group,
tail paths/entry identities and expected identities before asserting. Expected
row ledger remains available to reconstruct affected queries with the frozen
oracle; no oracle normalization or modified availability declaration is allowed.
This is a real source-acquisition test, not an abstract transition simulation.

## Hook boundary and verification

Query PI owns a minimal private view implementation/callback for unit tests;
normal `view` supplies no callback effect. No public constructor, global hook,
service callback, production synchronization or filesystem pin is introduced.
Callback runs synchronously at the specified acquisition cut, representing an
external publisher's filesystem interleaving. Store/sealer must not acquire the
catalog mutex, so the fixture introduces no artificial recursive lock.
The test lives under the catalog's cfg(test) unit module, permitting access to
actual Sources while keeping the hook outside public integration APIs.

Before the failing assertion, archive origin/seed, pre-IO expected Batch and row
ledger, observed selected-source summary, exact selected custody and assertions
that publication/reclaim completed. Preserve full owned state on failure. Run
without callback first as a valid control. Neither a missing cut nor accidental
retention counts as reproducing the race. Root records source/protocol hashes,
exact command/exit and failure receipt; passing later revisions are separate runs.

Planned exact selector, to match final source before admission:
`read_catalog::race_tests::publication_and_reclaim_between_discovery_steps_preserve_coverage`.
Use `cargo test --offline --locked -p fabric-server --lib SELECTOR -- --exact
--test-threads=1 --nocapture` inside the serial coordinator/resource launcher.
Set `FABRIC_STORAGE_EVIDENCE` to a fresh owned evidence directory and route all
state to `FABRIC_SCRATCH_ROOT` on the mounted data drive. Never use /tmp fallback.

Proposed budget: 300-second cell, existing 16/20 GiB high/max and zero swap,
64 MiB owned scratch, 16 GiB free reserve, 8 MiB evidence. Stop on unexpected
control failure, IO error, OOM, deadline/storage limit or surviving process;
retain failure state before cleanup. Correctness/coverage is the acceptance gate;
duration and cgroup/scratch observations are descriptive only.
This single schedule does not prove general concurrency, physical power loss,
or table-specific availability semantics. No O2/O3 completion claim precedes it.
