# Bounded verifier witnesses and interruption evidence

Revision 1, registered 2026-10-11. This is a scoped verifier correctness
investigation, not deployment qualification or an ingestion benchmark.

## Origin and fixed contract

Hosted run `38156896105` timed out during the unchanged compiler mutation audit.
An initial isolated diagnostic at `1777e66` replaced `after_page` with `true`:
the selected mutant timed out in five seconds and the cgroup peaked at 1.9G.
The registry lost that hosted command's output because capture waited for exit.
These are historical diagnostic observations, not a completed comparison under
this protocol. The cause of the hosted timeout is not fully attributable from
its missing audit transcript.

Keep all existing query answers, original complete-drain assertions, generated
mutations, equivalence reasons, named semantic controls, Python oracles and
receipt-completeness requirements. Product query/delivery behavior, dependency
policies, symbolic bounds and resource deadlines remain unchanged.

## Hypotheses and decision rules

H1: A test-only pagination preflight rejects invalid progress before complete
client drain. Given distinct finite row keys and deterministic pure page
semantics, at most `rows.len() + 1` pages suffice. Compare continuation and row
keys directly rather than calling the cursor predicate under test. On valid
pages, execute the original complete drain and all existing assertions.

Null: The representative broken cursor still times out, grows without bound,
survives, or an original positive assertion fails. H1 requires the same
`after_page -> true` mutation to be caught by a test failure; timeout is not
success for this selected diagnostic. The complete audit retains its original
treatment of compiler timeouts and its complete mutation/equivalence set.

H2: During interruption, both output streams persist before a check exits.
Use a child that flushes distinct stdout/stderr sentinels and then blocks.
Terminate its owned process group after a disk-backed readiness signal.
Require both partial logs and the running marker, with no completion receipt
or completed aggregate log. Existing success/failure fixtures must still match
the same complete stdout-then-stderr bytes and SHA256.

Null: Any sentinel is missing, a completed receipt is fabricated, a descendant
is leaked, or an existing complete output/hash assertion changes.

## Execution and evidence

Run through `python3 -B tools/resource_group.py -- COMMAND ...`; use the mounted
data drive and owned `FABRIC_SCRATCH_ROOT`. Selected mutation diagnostics use
a 4 GiB cgroup and the explicit five-second mutation test window. The complete
audit uses a 6 GiB local cgroup or the existing verified hosted ancestor, with
the unchanged 30-minute launcher deadline. No swap or uncapped fallback.

Commands:

- `cargo test --locked -p fabric-core -p xtask`
- `cargo mutants -p fabric-core -p fabric-app --test-package fabric-core --test-package fabric-app --regex 'after_page -> bool with true' --timeout 5 --jobs 1 -o OWNED_DATA_OUTPUT`
- `cargo xtask checks --only cargo-mutants --receipts OWNED_DATA_RECEIPTS`
- `cargo xtask checks --profile fast --receipts OWNED_DATA_RECEIPTS`, then the existing final receipt validator

Mutation subprocesses retain worker-local targets, incremental compilation,
test opt-level 1, checked arithmetic and debug assertions. Record exact source
revision or working-tree diff, commands/exits, mutation classifications, log
digests, duration, CPU, cgroup peak, storage, stop status and cleanup. Cost
observations are diagnostic: compiler/cache differences prevent attributing a
whole-process speedup or heap reduction to this small fixture alone.

Preserve failed/interrupted inputs and traces before cleanup. Publish bounded
results and limitations in the [integration review](https://github.com/kmosoti/FabricO11y/wiki/Release-integration-review)
and [agent execution review](https://github.com/kmosoti/FabricO11y/wiki/Agent-execution-review).
Register minimized defects in [counterexamples](../../formal/counterexamples.json).
