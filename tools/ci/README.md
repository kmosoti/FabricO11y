# CI scheduling, reuse and evidence

CI executes the registered checks; a cache hit is not a verification result.
The command definitions and expectations in `xtask/checks.json` stay unchanged.
All project workloads continue through `tools/resource_group.py` with the same
mounted storage, memory/swap limits and bounded process-tree cleanup.

## Scheduling

The main Rust workflow runs on pull requests, main-branch pushes, tag pushes and
manual dispatch. A feature-branch push no longer duplicates its pull-request
merge-candidate run. Each workflow cancels superseded runs of the same PR only;
workflow names are part of the concurrency key, so independent workflows cannot
cancel each other. Already-started runs from older workflow definitions are not
retroactively guaranteed to use these groups.

Formatting runs before browser and proof-tool installation. The fast registry
still runs its full canonical check set. Its WASM compilation, contrast and
packaging checks are not repeated as separate later steps. Default-feature and
all-feature test executions remain distinct; no equivalence is assumed merely
because their crate names overlap. The focused security tests remain enabled.

Extended verification exposes all ten existing registry checks as separate
sequential steps on one runner. Tools and compiled dependencies are reused;
there is no ten-runner cold-build fan-out. Each command uses the existing
30-minute containment deadline, while the overall job remains capped at 120
minutes. Later independent checks can run after a check fails, but a failed
step still fails the job. Missing, interrupted or failed check receipts make
the final completeness check fail. The per-profile test verifies that every
extended check is scheduled exactly once, so a new registry entry cannot be
silently omitted.

## Dependency caching

The Rust cache action is pinned to the reviewed v2.9.2 commit. Its workspace
mapping receives a computed relative path because the action joins the target
to the workspace root. The actual target is the existing mounted
`/run/media/kmosoti/data/FabricO11y/cargo`, not repository `target/`.

Cache only dependency build artifacts, Cargo registry material and installed
Cargo tools selected by the action. Do not cache verification receipts, browser
profiles, TLS fixtures, runtime state, credentials or the whole Cargo home.
Workspace-crate caching and cache-on-failure are disabled. Fork pull requests
may restore an eligible cache but do not save through this configuration.
Fast/focused and extended jobs use separate cache families; compiler, manifest,
lockfile and relevant environment inputs remain part of the action's key.
A miss is a normal cold build, not a reason to skip tests or use stale evidence.

The first successful run populates the applicable cache. Savings must be
measured on subsequent matching warm runs; no speedup is claimed in advance.

## Diagnostics and completeness

`cargo xtask checks` prints and flushes the starting check ID and command,
records `current-check.json`, and retains complete combined stdout/stderr as
`<check>.log` once that check finishes. The receipt names that log and hashes its
exact bytes. A killed check leaves the running marker and has no new successful
completion receipt. Output is still captured by `Command::output`; this change
does not claim byte-by-byte live streaming or bounded capture memory.

`verify_receipts.py` requires every selected registry check, the checked-out
commit, unchanged worktree, exact command/profile, exit zero, passed status and
matching log digest. It rejects missing, stale, interrupted, failed or altered
evidence. The checker validates receipt integrity and coverage, not the semantic
adequacy or independent trust of an unsigned receipt. Raw logs may contain test
fixture diagnostics and must be treated as code-originated output.

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
