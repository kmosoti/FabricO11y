# Bounded lightweight verification dispatch

Prospective policy, registered before production integration. The owner requested
Rust tooling and optimized verification while alpha acceptance is paused.
Measured [scheduling results](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-verification-scheduling-results-01)
are bound to their original sources and workload, not this untested integration.

## Allowed composition

Add an opt-in `checks --profile fast --light-workers 2 --receipts NEW_DIR`.
No flag preserves the existing serial API and behavior. Reject another worker
count, another profile, `--only`, a reused output root or a changed cohort command
vector. Require explicit DATA storage and the original enforced outer launcher.
No nested resource launchers, concurrent heavy checks or weakened registry gates.

The cohort is exactly delivery-oracle, query-oracle, workload-rate-oracle,
soak-decision-rule, telemetry-contract, telemetry-cli and ui-palette. Freeze the
original command arrays. Prior measured weights in milliseconds are respectively
310, 322, 887, 87, 142, 2241 and 42. Descending weight with canonical ID ties
orders the bounded work-conserving queue. This is O(N log N) preparation and O(N)
dispatch with O(N) metadata and at most two live children; it is a heuristic for
changed durations, not a universal optimal scheduler. Reassess classification if
these tests' resource behavior changes; a source ID is not proof of future cost.

Run the registry prefix serially, the cohort at its first selected position, then
every remaining noncohort check serially in original order. ui-palette is moved
from its later independent position into the cohort. All selected IDs execute
exactly once, including after another check fails. Preserve original commands,
property coverage, oracles, proof bounds and negative outcomes. New checks are
never implicitly added to the lightweight cohort.

## Publication and failure boundaries

Capture one source/tool context. Each worker has unique staging receipts and
TMPDIR under launcher DATA scratch. Keep a canonical batch running marker while
the cohort is active. Reuse the existing recorded command executor; join/reap
every started child after failure. Aggregate Failed before EnvironmentUnavailable
before Passed. Panic, spawn, log/publication or source-identity failure is nonpass.

Publish complete ordinary receipt/log files from owned staging only, with
exclusive creation and no overwrite. Preserve failed/incomplete artifacts and
markers; never reuse historical receipts. Remove the canonical marker only after
successful publication. Before returning success, validate the complete selected
fast inventory with the native verifier and recheck current clean source. The
unchanged independent Python receipt verifier remains a final cross-check.
No partial subset, suppressed child error or canceled batch can become success.

## Required controls before adoption

Test exact-once IDs, worker bound, deterministic weights/ties, command drift,
invalid flags, fresh-output enforcement, stable source, and status precedence.
Inject actual exit-7 and spawn/termination failures, require other independent
jobs to finish, and retain incomplete markers. Check missing/altered receipts,
changed logs and wrong-source evidence with the independent reference. Exercise
the final integration against the full unmodified fast registry; compare complete
semantic inventories before interpreting performance. Preserve every failed
attempt and actual launcher stop. Prototype timing is not integration timing.

Required full checks and measured matched integration runs precede enabling this
path in CI. Policy, implementation and workflow adoption are separate reasoned
commits. This does not change product durability, certify arbitrary code, resume
alpha acceptance or authorize a release/tag.
