# Contributing

Use Rust beta-2026-10-18 (1.100.0-beta.7), native Cargo commands and Jujutsu 0.46.0 with a Git
backend. Initialize an existing Git checkout with `jj git init --colocate` and
configure your Jujutsu identity before committing.

Write repository utilities and tests in Rust under `xtask`. Keep CI bootstrap
scripts under `.github`.

## Changes

Follow the product constraints in [the direction document](docs/direction.md).
Keep changes focused, preserve existing behavior unless the change requires
otherwise, and make errors and resource limits explicit. Add tests for meaningful
behavior and failure cases, and keep specification changes separate from their
implementation. Update project documentation when setup, behavior or contribution
procedures change.

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
cargo xtask candidate capture /tmp/fabric-candidate.txt
cargo xtask candidate verify /tmp/fabric-candidate.txt
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
cargo xtask prove aeneas
cargo xtask prove selftest
cargo xtask fuzz replay /tmp/fabric-fuzz-replay
cargo xtask fuzz smoke /tmp/fabric-fuzz-smoke
cargo xtask fuzz scheduled /tmp/fabric-fuzz-scheduled
cargo xtask profile permissions /tmp/fabric-perf-permissions
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

## Algorithm candidate evaluation

The bounded task contract is in `verification/search-contract.md`; competent
baselines and inefficient starters live under `xtask/fixtures/search/`.
Create a JSON manifest with `task` (membership, topk or intervals), `strategy`
(serial, fresh, retained or baseline), `trial`, `family`, `rationale`, optional
`parent` with `parent_source_sha256`, `source`, `algorithm`, `representation`, `ownership`, `data_flow` and
`falsification`. Structural choices and the falsification experiment are required.

```sh
cargo xtask search register /tmp/proposal.json /tmp/fabric-algorithm
cargo xtask search evaluate /tmp/fabric-algorithm /tmp/development.json development
cargo xtask search evaluate /tmp/fabric-algorithm /tmp/final.json heldout
cargo xtask search compare /tmp/baseline.json /tmp/final.json
cargo xtask search status /tmp/fabric-algorithm selected /tmp/final.json
```

Registration captures repository identity and immutable source/contract digests.
Compilation and execution run in Bubblewrap without host credentials or network.
The contract prohibits candidate I/O, environment inspection and unsafe code;
this is an evaluation boundary, not a complete sandbox for hostile native code.
Hold final cases away from development sessions and select before final evaluation.
Changing candidate source or the contract invalidates registration. Comparison
rejects failed results and mismatched evaluator, compiler, contract, task or split.
The first comparison result is the explicit baseline. Inspect per-workload timing
ratios and resident memory, including preprocessing and output allocation. Keep
tradeoffs and failed alternatives in local artifacts rather than a composite score.
Status changes append an evidence record; evaluated/rejected/retained/selected
states preserve earlier records. Retained and selected states require correct
evaluation evidence for the same source and contract.

## Admission measurements

Capture and verify the current source before measuring it. The benchmark
requires its matching verification receipt and measures a copied release
executable under `perf`; compilation is outside the measurement boundary.

```sh
cargo xtask candidate capture /tmp/fabric-benchmark-source.txt
cargo xtask candidate verify /tmp/fabric-benchmark-source.txt
FABRIC_BENCH_RECORDS=20000 cargo xtask benchmark /tmp/fabric-admission-benchmark
cargo xtask benchmark compare /tmp/baseline-profile /tmp/candidate-profile
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
