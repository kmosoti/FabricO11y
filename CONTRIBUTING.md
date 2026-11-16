# Contributing

Use stable Rust (>= 1.100.0), native Cargo commands, and Jujutsu 0.46.0 with a Git
backend. Initialize an existing Git checkout with `jj git init --colocate`,
configure your Jujutsu identity, and install the commit template before committing:

```sh
git config commit.template .github/.gitmessage
```

Write repository utilities and tests in Rust under `xtask`. Keep CI bootstrap
scripts under `.github` and linked architecture rationale under `knowledge/`.

## Two-Branch Stacked Workflow

- **`develop` (day-to-day integration):** Develop atomic linear commits on stacked Jujutsu bookmarks (`stack/<topic>/01-...`). Before landing onto `develop`, PRs must pass `development-checks.yml`, include focused Mermaid diagrams in [`docs/architecture.md`](docs/architecture.md) when boundaries/dataflows change, and receive an independent standard/medium-reasoning review (preferring an alternative model family). Rebase-only linear history is required.
- **`main` (human-gated release):** Updated exclusively by promotion PRs from `develop`, gated by `.github/workflows/main-human-gate.yml` (`main-release` environment). Autonomous agents never merge into `main` or approve `main-release`.
- **Storage discipline:** Export `TMPDIR=/data/tmp`, `CARGO_BUILD_TARGET_DIR=/data/cargo-target`, and `FABRIC_DATA_DIR=/data/fabrico11y` locally (or `/var/tmp/...` in ephemeral containers) — never write scratch or target artifacts to RAM-backed `/tmp`.

## Changes

Follow the product constraints in [the direction document](docs/direction.md) and
[architecture specification](docs/architecture.md).
Keep changes focused, preserve existing behavior and Sans-I/O boundaries (`admission-kernel`, `fabric-core`, `fabric-graph`), make errors and resource limits explicit, and keep UI/frontend scope out of the repository. Update `knowledge/` records (`added_by: agent`, `human_ratified: false`) alongside code changes.

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
authorized. `verification/aeneas.opam.lock` freezes the OCaml dependency set.
Application builds continue to use `rust-toolchain.toml`.

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
