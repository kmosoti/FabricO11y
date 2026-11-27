# Contributing

Use Rust 1.100.0, native Cargo commands, and Jujutsu 0.46.0 with a Git
backend. Initialize an existing Git checkout with `jj git init --colocate`,
configure your Jujutsu identity, and install the commit template before committing:

```sh
git config commit.template .github/.gitmessage
```

Write repository utilities and tests in Rust under `xtask`. Keep CI bootstrap
scripts under `.github` and linked architecture rationale under `knowledge/`.

## Two-branch stacked workflow

Create isolated colocated Jujutsu changes and export only the bookmarks you
intend to publish. The first pull request targets `develop`. Each dependent
pull request targets its predecessor. Publish under the authorized collaborator
identity. GitHub CLI 2.97.0 and `github/gh-stack` 0.3.0 support this bridge:

```sh
jj git push --remote origin --bookmark stack/topic/01-change --bookmark stack/topic/02-followup
gh stack link --base develop <lower-pr-url> <upper-pr-url>
```

Open the PRs with their intended bases before linking them by URL. Review each
diff, wait for `verify` and `native-acceptance` on its current merge candidate,
and review policy, schema, authorization and proof changes independently.
GitHub can request a CODEOWNERS review on develop. Only main enforces the
owner's pull request review. On a lower-layer amendment, rebase
its dependents and rerun their evidence. Integrate one layer at a time by rebase,
then retarget the next PR and recheck its actual base and head. Require an
independent review for agent-authored work.

### Current-base integration evidence

The integration controller serializes landings and runs the validator from a
clean, trusted `develop` checkout, never from the candidate. The version 1 input
shape is illustrated by [the synthetic fixture](xtask/tests/fixtures/integration-v1.json);
its IDs are examples, not evidence. Collect facts independently using the
authenticated GitHub API, Git objects and hosted Actions job logs; candidate-provided
facts and review assertions are not trusted. Store the evidence outside the candidate.

1. Query the live repository and PR: require an open, non-draft PR targeting
   `develop`, capture its current base and head, and reject conflicts or an
   unavailable merge preview. Inspect the preview Git commit and tree: its two
   ordered parents must be the live base and head. Independently inspect the head
   tree and require it to equal the preview tree. Collect the ordered commit
   chain from the exact base through the head; each commit must have exactly
   one parent, the preceding commit. Reject missing, divergent or merge-commit
   chains. Derive changed paths from
   the base to the preview tree, including both old and new paths for renames.
2. Identify the implementation author independently of the GitHub account that
   posts the PR. Obtain a review in a separate context by a distinct reviewer,
   bound to repository, PR, base, head and preview tree. Record the reviewer,
   implementation author, independent context, affirmative approval and review
   receipt reference.
   Review the complete diff for protected semantics, including changes outside
   the inventory; set `semantic_review_required` when that inspection identifies
   protected semantics, and record the specific semantics reviewed when relevant. A shared
   public GitHub identity used to post work is not itself independent approval.
3. Identify the `pull_request` run of `.github/workflows/development-checks.yml`
   for the candidate, using the latest applicable run and its latest attempt,
   and the same run attempt's successful `verify` and
   `native-acceptance` jobs. Record their unique job IDs and the checkout commit
   from each hosted job's checkout log, not the run summary's head SHA. Correlate
   the run's source branch/head and checkout preview with the PR and ordered
   parents when the run API's PR association is empty. The hosted jobs check
   those parents. Supply only facts verified by the controller.
4. Run `cargo xtask integration verify /absolute/path/to/facts.json`. Inventory
   matches require a nonempty semantic review scope. The baseline covers
   `registry/`, `proofs/`, `verification/`, `.github/`, `.codex/`,
   `admission-kernel/`, `docs/contracts/`, `schemas/`, `spec/`, `specs/`, filenames
   containing `auth`, `schema`, `policy`, `spec`, `contract`, or `gate`
   (case-insensitive), path components named
   `auth`, `authorization`, `schema`, `schemas`, `policy` or `policies`, and
   `AGENTS.md`, `CONTRIBUTING.md`, root Cargo manifests/lock and toolchain,
   architecture/direction documents, and the integration validator and its
   dispatcher and manifest. This inventory is a conservative floor, not a
   complete classifier of semantic changes.
5. While holding the integration serialization lock, re-query the PR, develop,
   merge preview, latest applicable run and attempt, and review immediately
   before landing. If any bound fact
   changes, stop and repeat collection, review and checks. Land one PR using
   `gh pr merge <number> --rebase --match-head-commit <verified-head>` and
   verify the actual resulting develop tree equals the tested preview tree and
   that its history from the verified base is linear before proceeding. Stop and
   investigate any mismatch; a rebase creates different commit IDs. If an actor can update
   develop outside the lock, serialization is not assured: stop and coordinate
   that actor. The head guard alone is not a base compare-and-swap.

GitHub requires the named checks on `develop`; the trusted controller enforces
this procedure and independent review. The validator checks consistency of
supplied facts, not their authenticity or cryptographic independence of models.
Only the owner review and `main-release` gate apply to the protected main
promotion. A changed base or head invalidates the evidence rather than carrying
review or checks forward.

Only the owner promotes `develop` into `main` and publishes releases. Follow the
[promotion runbook](docs/release-promotion.md).

Export `TMPDIR=/data/tmp`, `CARGO_BUILD_TARGET_DIR=/data/cargo-target`, and
`FABRIC_DATA_DIR=/data/fabrico11y` locally. Hosted containers use disk-backed
`/var/tmp`.

## Changes

Follow the product constraints in [the direction document](docs/direction.md) and
[architecture specification](docs/architecture.md).
Keep changes focused, preserve existing behavior and Sans-I/O boundaries (`admission-kernel`, `fabric-core`, `fabric-graph`), make errors and resource limits explicit, and keep optional frontend dependencies out of headless builds. Update `knowledge/` records (`added_by: agent`, `human_ratified: false`) alongside code changes.

## Verification

Run fast checks during iteration and medium checks before submitting changes:

```sh
cargo xtask check --tier fast
cargo xtask check --tier medium
cargo xtask check --tier extensive
cargo xtask check --tier acceptance
```

| Tier | Checks |
| --- | --- |
| Fast | Formatting, compilation, unit tests and repository configuration tests |
| Medium | Fast checks, Clippy, separate doctests and dependency-tree inspection |
| Extensive | Medium checks and isolated Git/Jujutsu candidate, archive, exclusion and rewrite tests |
| Acceptance | Medium checks, architecture, all native proof paths, checker negative controls and fuzz regressions |

The workflow tests use disposable repositories. Extensive checks run in manual
and scheduled CI jobs.

## Candidate verification

Capture and verify a candidate using an absolute artifact path outside the source tree:

```sh
TMPDIR=/data/tmp CARGO_BUILD_TARGET_DIR=/data/cargo-target cargo xtask candidate capture /data/tmp/fabric-candidate.txt
TMPDIR=/data/tmp CARGO_BUILD_TARGET_DIR=/data/cargo-target cargo xtask candidate verify /data/tmp/fabric-candidate.txt
```

Capture records the commit ID, compiler, Jujutsu version and executable identity.
Verification rejects stale captures, creates a separate workspace, compares its
contents with the captured commit and runs acceptance checks with separate build output.
It rechecks the source and writes a receipt beside the capture. Existing verification
directories are preserved and cause verification to stop.

Verification executes candidate code; isolate untrusted candidates without credentials.
Dependency-tree inspection lists dependencies and does not audit vulnerabilities or licenses.
Record the candidate commit, compiler and check results in pull requests.

## Native verification tools

The executable admission kernel is a workspace crate. Its pinned `vstd` dependency
must compile in ordinary Cargo builds as well as with Verus. Verus checks this
source through Cargo; Charon extracts the same crate and Aeneas generates the Lean
definition checked by `proofs/ExtractedAdmission.lean`. The independent historical
Lean model is available through `prove lean` and is not extraction coverage.

On Fedora, `.github/scripts/install-verification-tools.sh` bootstraps the pinned
Verus/Z3, Charon/Aeneas/Lean, OCaml dependencies, extraction/fuzz Rust toolchains
and cargo-fuzz. It installs native Fedora build prerequisites and `perf`; run it
in a development container or an environment where package installation is
authorized. `verification/aeneas.opam.lock` freezes the OCaml dependencies.
The installer validates a managed switch scoped to the lock digest and records
the Aeneas binary digest after a successful build. The first run after switching
from the legacy OPAM layout rebuilds Aeneas and its dependencies. Product builds
use the exact Rust pin in `rust-toolchain.toml`. Verus and Charon use their
independently pinned compiler versions.

Tools default to `$HOME/.local/share/fabrico11y/proof-tools`. Set
`FABRIC_PROOF_TOOLS` to use another installation, or override individual binaries
with `FABRIC_VERUS`, `FABRIC_CHARON`, `FABRIC_AENEAS`, `FABRIC_LEAN` and
`FABRIC_CARGO_FUZZ`. Commands report tool hashes and fail when a required tool is
missing or a checker times out. Extraction retains generated source and logs in
an ignored local artifact directory. Aeneas warnings, opaque application
functions and unfinished Lean proofs do not count as verification.

```sh
cargo xtask architecture
cargo xtask prove verus
cargo xtask prove smt
cargo xtask prove telemetry
cargo xtask prove aeneas
cargo xtask prove all
cargo xtask prove selftest
cargo xtask fuzz replay /data/tmp/fabric-fuzz-replay
cargo xtask fuzz smoke /data/tmp/fabric-fuzz-smoke
cargo xtask fuzz scheduled /data/tmp/fabric-fuzz-scheduled
cargo xtask profile permissions /data/tmp/fabric-perf-permissions
cargo xtask collection install-stack --bin-dir /data/cargo-target/release --data-dir /data/fabrico11y --tmp-dir /data/tmp --listen 127.0.0.1:17440
```

Choose a new absolute artifact directory for each run. Fuzz replay executes the
checked-in regression seeds. Smoke campaigns run 60 seconds per target; scheduled
campaigns run 30 minutes per target, with one worker. Campaigns copy seeds to local
artifacts so generated corpora do not modify source. Minimize discovered failures
with cargo-fuzz, then add the minimized input to the corresponding checked-in
corpus. Fuzzing does not exhaust concurrent schedules.

`perf` permission checks use an owned process and preserve host security settings.
Unavailable counters are recorded as failures with diagnostics. Proof results and
performance measurements are separate evidence. Native checks establish results;
LogicGate integration requires an explicit revision of its evidence contract.

## Admission measurements

Capture and verify the current source before measuring it. The benchmark
requires its matching verification receipt and measures a copied release
executable under `perf`; compilation is outside the measurement boundary.

```sh
cargo xtask candidate capture /data/tmp/fabric-benchmark-source.txt
cargo xtask candidate verify /data/tmp/fabric-benchmark-source.txt
FABRIC_BENCH_RECORDS=20000 cargo xtask benchmark /data/tmp/fabric-admission-benchmark
cargo xtask benchmark compare /data/tmp/baseline-profile /data/tmp/candidate-profile
```

The default matrix covers 64/512-byte ASCII text, 8/64 slots and 1/4 producers
with matching consumer counts, using one warmup and five measured repetitions.
`FABRIC_BENCH_CASES_FILE` accepts an absolute JSON case-file path; cases contain
`name`, `text`, `vector_len`, `slots`, `producers` and `consumers`.
`FABRIC_BENCH_RECORDS` sets successful records per producer per sample. Results include
throughput, accepted-call and retry-inclusive p99 latency, attempts, setup and
reserved storage. Inspect each metric separately. A consumer lease retains
capacity during processing, so consumers and retry policy are part of the workload.

Profiles bind source, acceptance receipt, compiler, executable, contract,
benchmark driver, case file, resource settings and machine topology. Comparison
rejects incompatible artifacts. `FABRIC_PROFILE_SECONDS` sets the profile
timeout (900 seconds by default; zero disables it). Select resource settings
appropriate to the machine and retain them with the evidence. Performance
results do not extend the scope of the kernel proofs.

## Local collection checks

`cargo test -p fabric` includes typed wire fixtures, SDK outage and durable flush,
retry conflicts, transaction rollback, quotas, file rotation/truncation, framing
and persistent adapter tests. `cargo xtask registry` validates the pinned Weaver
registry and checks generated vocabulary/descriptor drift. See
[registry maintenance](registry/README.md) for setup. Native fuzz replay includes
the v2 decoder. Journal capability diagnostics identify absent or inaccessible
live journals; deterministic fixtures cover cursor handling independently.

Changes to collection must preserve the distinct memory-admission and custody
outcomes. The SDK calls the verified delivery-frontier arithmetic; this does not
prove its synchronization adapter, SQLite or source adapters. See
[collection proofs and resource checks](docs/collection-verification.md) for precise
proof scope, memory bounds and the two executable verification programs.

See [representation and distributed custody evaluation](docs/fabric-server-evaluation.md)
for experimental codecs, separate custody seals, reproducible measurements and proof scope.
