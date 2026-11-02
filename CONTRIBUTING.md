# Contributing

Use Rust 1.100.0-beta.7, native Cargo commands and Jujutsu 0.46.0 with a Git
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
```

| Tier | Checks |
| --- | --- |
| Fast | Formatting, compilation, unit tests and repository configuration tests |
| Medium | Fast checks, Clippy, separate doctests and dependency-tree inspection |
| Extensive | Medium checks and isolated Git/Jujutsu candidate, archive, exclusion and rewrite tests |

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
contents with the captured commit and runs medium checks with separate build output.
It rechecks the source and writes a receipt beside the capture. Existing verification
directories are preserved and cause verification to stop.

Verification executes candidate code; isolate untrusted candidates without credentials.
Dependency-tree inspection lists dependencies and does not audit vulnerabilities or licenses.
Record the candidate commit, compiler and check results in pull requests.
