# CI scheduling, reuse and evidence

CI executes the registered checks; a cache hit is not a verification result.
The command definitions and expectations in `xtask/checks.json` stay unchanged.
All project workloads continue through `tools/resource_group.py` with the same
disk-backed storage root, memory/swap limits and bounded process-tree cleanup.
`prepare-resource-host.sh` sets `FABRIC_STORAGE_ROOT` (default `/mnt/fabrico11y`
on the hosted runner's disk) for later steps and sizes the parent cgroup with
the launcher's own `memory_limits`, so CI no longer bind-mounts an owner path.

Hosted setup explicitly delegates CPU, I/O, memory and task controllers through
a hosted-only runtime unit drop-in for `user@UID.service` and records actual availability before project jobs. The
completion harness checks all four before enabling descendants and verifies
kernel readback. Missing controllers fail setup; they are not made optional. Hosted setup refuses
to restart its own user-manager tree. A runtime unit drop-in is required because
systemd 255 accepts DelegateControllers over D-Bus only for transient units.
This addresses the retained PR72 Ubuntu browser startup failure. Its hosted
integration rerun remains required; local source controls cannot establish
Ubuntu delegation.

## Scheduling

The main Rust workflow runs on pull requests, main-branch pushes, tag pushes and
manual dispatch. A feature-branch push no longer duplicates its pull-request
merge-candidate run. Each workflow cancels superseded runs of the same PR only;
workflow names are part of the concurrency key, so independent workflows cannot
cancel each other. Already-started runs from older workflow definitions are not
retroactively guaranteed to use these groups.

The extended verification workflow runs on non-draft pull requests when they are
opened, pushed to or marked ready for review, on merge-queue groups, weekly and on
manual dispatch. Draft pull requests skip it. Keep a pull request in draft while
iterating and mark it ready once validated, so the suite runs once on the final
head. Evidence still binds to the tested head and tree.

Formatting runs before browser and proof-tool installation. The fast registry
still runs its full canonical check set. Its WASM compilation, contrast and
packaging checks are not repeated as separate later steps. Default-feature and
all-feature test executions remain distinct; no equivalence is assumed merely
because their crate names overlap. The focused security tests remain enabled.

Extended verification uses two hosted matrix workers: one runs only
`semantic-mutants`, and the other runs the remaining nine extended IDs in their
original order. Each worker runs its assigned checks serially through the
existing 30-minute containment launcher; matrix fail-fast is disabled and the
overall jobs remain capped at 120 minutes. A required `extended` job runs even
when producers fail, collects exact disjoint shard receipts into DATA-backed
storage, checks producer success and invokes `cargo xtask verify-receipts`
against the current source revision. Missing, stale, interrupted,
failed or altered check evidence cannot pass the aggregate. The Rust scheduling
guard and workspace unit controls check exact ID ownership, worker bounds,
launcher, aggregation and representative mutations.

The completed pilot reduced wall time by 41.98% and increased combined runner
time by 10.32%. Worker CPUs, cache hits and setup differed, so these figures
describe that hosted comparison only; they do not promise a universal latency
or cost reduction. The old serial job is not run a second time as an experimental
workflow; the separate sharding experiment remains available by manual dispatch.

## Pinned actions and toolchains

### Pinned CI tool installation

Use `cargo xtask install-tool <crate> <version>` for the pinned Cargo tool
install path. It preserves inherited `CARGO_HOME`, requires at least 2 GiB
available DATA space, gives Cargo a private owned target, and applies a fixed
1500-second deadline. Timeout cleanup kills the process group and reaps Cargo;
the target is removed and a failed log/receipt retained for review. Outputs live
under `$FABRIC_STORAGE_ROOT/results/ci-tool-install/` as `<crate>.log` and
`<crate>.json`. Invoke this command through the still-required outer
`python3 tools/resource_group.py -- ...` launcher. These controls do not claim
that a package was installed or establish performance. See the
[registered protocol](../../docs/experiments/formal/rust-tool-installer-protocol.md).

Every `uses:` reference names a full commit SHA with its tag in a trailing
comment, so a moved or re-tagged upstream ref cannot change a job silently.
Resolve a replacement with `git ls-remote https://github.com/OWNER/REPO
refs/tags/TAG` and use the peeled `^{}` commit for an annotated tag. The
`dtolnay/rust-toolchain` pin is the commit of its `stable` branch; the
`toolchain: 1.99.0` input stays explicit because a SHA ref carries no channel.
Extended verification installs the dated `nightly-2026-10-11` (rustc
1.101.0-nightly, 2026-10-10) and links the name `nightly` to it, because
`fuzz/smoke.sh` runs `cargo +nightly`. Advance the date deliberately and rerun
the fuzz check.

## Vendored crates

Cargo does not checksum `[patch.crates-io]` path crates and `cargo deny` does
not inspect them. `check-vendor.py` downloads each archive named in
`vendor/provenance.json`, requires its SHA-256, and compares every file with
`vendor/<crate>/`. Only the provenance-declared files may differ: the added
`FABRIC-PATCH.md`, and a `Cargo.toml` equal to the upstream one with just the
declared dependency alias rewritten. Extra, missing, altered or symlinked files,
and a `[patch.crates-io]` table that differs from the provenance paths, fail
the fast workflow. `test_check_vendor.py` seeds each of these defects.

## Dependency caching

The Rust cache action is pinned to the reviewed v2.9.2 commit. Its workspace
mapping receives a computed relative path because the action joins the target
to the workspace root. The actual target is `$FABRIC_STORAGE_ROOT/cargo` on the
runner's disk, not repository `target/`.

Cache only dependency build artifacts, Cargo registry material and installed
Cargo tools selected by the action. Do not cache verification receipts, browser
profiles, TLS fixtures, runtime state, credentials or the whole Cargo home.
Workspace-crate caching and cache-on-failure are disabled. Fork pull requests
may restore an eligible cache but do not save through this configuration.
Fast/focused and extended jobs use separate cache families; compiler, manifest,
lockfile and relevant environment inputs remain part of the action's key.
A miss is a normal cold build, not a reason to skip tests or use stale evidence.

Mutation subprocesses enable incremental compilation in their owned source
copies, where repeated one-file edits otherwise discard compiler work. The
canonical commands, all baseline/negative controls and equivalence expectations
remain unchanged. This is a candidate timeout repair, not a measured speedup or
evidence that a previously interrupted audit passed.

The core/app cargo-mutants audit uses test optimization level 1 while explicitly
retaining debug assertions and overflow checks. It runs the same complete tests
and mutants: exhaustive pure transitions are computed faster, rather than
sampled less often. Semantic memory-bound mutants retain their original profile.

Each cargo-mutants worker overrides an inherited absolute `CARGO_TARGET_DIR`
with the relative `target` path inside its owned source copy. Different mutated
trees must never share compiled crate outputs. The local October 11 audit with
an absolute shared directory reported known tested defects as survivors; its
results are contaminated and cannot establish coverage or equivalence.

The `M-PROP-PAGE` negative control targets the `kernels` property-test binary,
including its unchanged `pages_cover_every_row_once_in_order` generator and
progress assertions. Running every property binary after deliberately breaking
cursor progress can trap the specification's client-drain helper forever before
the named result is collected. The ordinary property gate still runs all binaries.

Group-binding controls remove the production secondary reverse lookup, rather
than editing the frozen legacy reference. Both application and property checkers
include the deterministic third-credential intrusion witness. The remote-config
control bypasses validation at the current `effective` adapter boundary while
preserving source merging. Kani negative controls run their unchanged named
harnesses; the separate `kani-core` gate still verifies all eleven positive
harnesses. This removes unrelated repeated proof work from the 30-minute audit.

The delivery fault fixture uses the explicit legacy administration adapter,
its configured test CA for the mandatory companion, and independent stopped
Spool observations for every producer including that companion. A server crash
kills the owned server/companion process group. This does not establish production
console access: the native browser and scoped security checks own that boundary.
Its negative-control grader requires the actual missing-ACKed-record invariant
with clean process exits; an arbitrary Python failure cannot satisfy the gate.

The first successful run populates the applicable cache. Savings must be
measured on subsequent matching warm runs; no speedup is claimed in advance.

The separate fuzz dependency lock includes the current server graph.
`cargo-fuzz` 0.13.2 has no `--locked` switch: `fuzz/smoke.sh` first resolves
the same manifest using `cargo +nightly metadata --locked`, then rejects any
lock-byte change during the trial. All four targets, committed seeds, 60-second
search windows and 2,048 MiB per-target RSS limits remain unchanged. The initial
October 11 extended run completed its commands but failed receipt completeness
because fuzzing rewrote the stale lock. That run remains failed.

## Diagnostics and completeness

`cargo xtask checks` prints and flushes the starting check ID and command,
records `current-check.json`, and writes separate partial stdout/stderr files
while the check runs. Old streams are truncated before publishing a new running
identity. It retains complete combined stdout/stderr as
`<check>.log` once that check finishes. The receipt names that log and hashes its
exact bytes. A killed check leaves the running marker and has no new successful
completion receipt. The completed log preserves stdout-then-stderr ordering;
partial streams remain diagnostic evidence and never establish completion.
The hosted artifact also keeps raw compiler-mutation outcomes and logs. Final
aggregation still reads complete output into memory; no whole-verifier memory
bound is claimed.

Query-definition tests check strict cursor progress over finite keys before
calling the original complete drain. A broken predicate is rejected before
repeated pages can accumulate. Original positive assertions and the complete
mutation/equivalence set remain. The scoped procedure is registered in the
[verifier protocol](../../docs/experiments/formal/verifier-boundedness-protocol.md).

`cargo xtask verify-receipts` requires every selected registry check, the
checked-out commit, unchanged worktree, exact command/profile, exit zero, passed
status and matching log digest. It rejects missing, stale, interrupted, failed
or altered evidence. Final validation also requires the live worktree to be
clean, so a last check cannot modify inputs after recording its pre-check dirty
flag. The checker validates receipt integrity and coverage, not the semantic
adequacy or independent trust of an unsigned receipt. Its separately authored
acceptance and refusal outcomes are in
[`xtask/tests/fixtures/receipt_contract_cases.json`](../../xtask/tests/fixtures/receipt_contract_cases.json).
Raw logs may contain test fixture diagnostics and must be treated as
code-originated output.

Run the CI contract tests through the existing containment launcher:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/ci/test_ci_contracts.py
```

The Rust receipt/log regression remains part of the canonical workspace tests.

## Recorded diagnosis, not a blanket application verdict

On historical head `39b981e85ec6f639a7e474338a1dd447e11dd217`, run
[38092627826](https://github.com/kmosoti/FabricO11y/actions/runs/38092627826)
spent 98.6 seconds installing cargo-deny before discovering a formatting failure
in 0.5 seconds. Its workspace compile and all-feature test command passed; the
latter took 302.4 seconds including Cargo's build work. Formatting and a deprecated
atomic API rejected by Clippy were actual patch defects, not infrastructure
failures. Those defects were corrected in subsequent revisions.

On head `fd5366e26c6fcf3bbceecb197cff7e1cd541963e`, run
[38093738991](https://github.com/kmosoti/FabricO11y/actions/runs/38093738991)
passed the complete fast registry, dependency policy, console build and Firefox
shell checks, but the native browser test failed waiting for its first 200-row
log page. Its retained UI text shows `HTTP 429: request admission is full`.
The fixture's API helper is paced at 0.1 seconds, while the new private peer
budget is eight requests/second and UI actions share that peer. This establishes
a rate-limit/workload compatibility problem to investigate, not proof of a
query-engine defect or permission bypass. Scheduling changes do not resolve it;
the browser assertion and limiter remain in force.

Keep that failure separate from pipeline overhead. Do not increase admission
limits, suppress the browser check, or relax its expected rows to obtain a green
run. Qualify benign workload pacing and record the actual request sequence before
changing either the application or its verifier. Formal/mutation checks also
remain required; long execution alone does not establish a deadlock or bad code.


Browser defect receipt grading requires the exact registered failure and the
current structured process cleanup observations, including kernel-empty groups,
reaped bound children and own-child ECHILD. A historical cleanup string alone is
insufficient; the retained hosted counterexample and correction are registered
in the [browser protocol](../../docs/experiments/formal/browser-supervisor-protocol.md#hosted-defect-receipt-cleanup-binding-correction).
