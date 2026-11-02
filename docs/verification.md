# Verification

| Tier | Checks |
| --- | --- |
| Fast | Formatting, compilation, tests and repository configuration |
| Medium | Fast checks, Clippy, separate doctests and dependency-tree inspection |
| Extensive | Medium checks, Jujutsu archive/exclusion/rewrite tests and candidate refusal tests |

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
