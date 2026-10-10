# Contributing

Start with [CURRENT.md](CURRENT.md), the [product contract](PRODUCT-CONTRACT.md), the affected [architecture view](architecture/README.md) and the source. [AGENTS.md](../AGENTS.md) holds the same rules for coding agents; the [documentation policy](documentation-policy.md) applies to everyone.

## Reporting bugs

Use [GitHub Issues](https://github.com/kmosoti/FabricO11y/issues/new) for ordinary
defects and usability feedback. Include the commit or package version and
checksum, OS/browser versions, relevant resource limits, a minimal synthetic
reproduction, expected behavior, observed behavior and whether restart changes
the outcome. For performance reports, include workload size, CPU, RSS versus
cgroup memory, and the measurement interval.

Share only the configuration fields and redacted diagnostics needed to reproduce
the issue. Server access state, Spool/journal files and browser profiles can
contain credentials or telemetry. Security defects follow the
[private reporting instructions](../SECURITY.md).

## Architecture

Code lives in layers: `fabric-core` (pure `no_std` decisions), `fabric-ports` (effect contracts), `fabric-app` (use cases), `fabric-frame` (adapter support), and the composition roots `fabric-server` and the root package ([system view](architecture/system.md)). `cargo xtask check-layers` and `cargo xtask check-core-purity` enforce the dependency rule and the core's purity ([ADR-0015](decisions/ADR-0015-adopt-a-hexagonal-architecture.md), [ADR-0016](decisions/ADR-0016-keep-a-pure-semantic-core.md)). Wire and persisted bytes never change with a type move.

## Milestones, branches and releases

Work belongs to a milestone and its `milestone/<capability>` branch ([roadmap](ROADMAP.md)). Release maturity is only a tag such as `v0.1.0-alpha.1` on a revision whose required gates have recorded evidence ([ADR-0019](decisions/ADR-0019-keep-release-maturity-in-tags.md)). Never rewrite published history.

## Evidence and verification

Acceptance rests on executable evidence, not on anyone's approval ([ADR-0018](decisions/ADR-0018-accept-work-on-executable-evidence.md)); the [verification strategy](formal/verification-strategy.md) says which technique answers which question.

- The Python delivery, query and rate oracles are independent of the Rust code and stay that way. A test written with an implementation is not an independent oracle.
- Changing the product contract, an oracle, a negative-control expectation, a formal invariant, a registered protocol or verification policy is a trust-boundary change: its own commit, with its reason.
- A checker ships with a defect it must reject. Semantic mutants in [xtask/mutants.json](../xtask/mutants.json) run with `cargo xtask mutants`; no unexplained surviving mutant may threaten a claimed contract.
- A defect found by any means becomes a minimized, deterministic regression fixture.
- Say "passed" only for a check that ran; report not run, interrupted, failed, inconclusive and environment unavailable plainly.

## Checks

`cargo xtask checks --profile fast` runs every required fast check listed in the [registry](../xtask/checks.json) and writes receipts under `target/verification/receipts`; a missing tool yields INCOMPLETE (exit 3), never a pass. The `extended` profile adds the semantic mutants, the cargo-mutants audit, delivery fault runs, TLA+ models, Kani proofs, a fuzz smoke run, the dependency policy and a coverage report; [extended CI](../.github/workflows/verification.yml) runs it. Registered qualification protocols ([qualification](QUALIFICATION.md)) never run in CI; they need explicit scope. [Rust CI](../.github/workflows/rust.yml) runs the fast set. During experimentation, Bun documentation checks and hook probes are optional through `cargo xtask checks --profile documentation`; the dedicated documentation workflow and Bun setup in Rust CI are removed by owner direction.

Documentation tooling needs Bun 1.4.0 and Python 3: `bun install --cwd tools/docs --frozen-lockfile`, then `bun tools/docs/check.mjs`. The [checker](../tools/docs/check.mjs) verifies relative links and heading fragments, Mermaid syntax, marked diagram copies, JSON syntax and duplicate ADR numbers; it performs no network access or writes and does not decide whether prose is true. Canonical diagrams live in `docs/diagrams/*.mmd`; a page may repeat one after a `<!-- diagram: relative/path.mmd -->` comment, and the checker requires the copy to match. When changing the checker or hooks, also run `bun tools/docs/check.test.mjs` and `python3 -B tools/docs/test_hooks.py`.

## Resource containment

Run project workloads, including builds, tests, mutants, experiment harnesses,
validators and documentation checks, through the repository launcher:

```sh
python3 tools/resource_group.py -- cargo xtask checks --profile fast
python3 tools/resource_group.py -- cargo xtask mutants
python3 tools/resource_group.py -- python3 tools/bench/run_performance_refinement.py --help
```

The whole command tree shares a systemd user service with `MemoryHigh=16G`,
`MemoryMax=20G`, `MemorySwapMax=0`, group OOM termination and a 30-minute deadline.
These are workstation protection limits, not product capacity promises. The
launcher checks its actual cgroup v2 limits before executing the command; missing
systemd user access or unenforced limits mean NOT RUN, with no uncapped fallback.
Launch workloads sequentially: multiple capped services still add their budgets.
Use the mounted data drive at `/run/media/kmosoti/data/FabricO11y`: the launcher
sets `CARGO_TARGET_DIR` to its `cargo` directory, `FABRIC_SCRATCH_ROOT` to `scratch`,
and `TMPDIR`, `TMP` and `TEMP` to a unique owned directory per invocation.
It refuses to run if the data drive is unmounted. Harnesses with explicit scratch
paths must use `FABRIC_SCRATCH_ROOT`; an environment variable cannot redirect a
hardcoded path. `/tmp` on the Fedora workstation is tmpfs.
The qualification runner accepts direct `target/alpha-*` outputs only beneath
`FABRIC_SCRATCH_ROOT`, including a frozen mini-tree's nested `target` directory;
repository `target` outside that root is rejected before creation or launch.
Use the tested data-backed freeze commands in the
[qualification runbook](qualification-runbook.md), not legacy `freeze.sh`.

Success requires a matching completion receipt from the command inside the
cgroup and a matching `systemd-run` exit. A disconnected service bus or missing
completion is incomplete even if the outer transport returns zero. After verified
success and service termination, the launcher removes its owned temporary directory. On failure or interruption it moves that directory to the
drive's `evidence` directory after stopping the service; if stopping cannot be
confirmed, it retains the directory in place. It never deletes shared build
caches or unrelated files. Each invocation records command, exit, elapsed time,
storage, containment limits, command-return cgroup resource samples and
cleanup/retained-evidence paths under
`target/resource-containment/runs`. Harnesses must retain measured observations,
exactness results and meaningful failures in their experiment records before
removing owned trial scratch. Review retained failure files before pruning them.
Resource samples precede final service shutdown; they are not a final
descendant-lifetime peak. Higher limits or longer runs require explicit owner scope. Read-only
inspection is exempt.

On 2026-10-04 at 10:21:20, the Fedora kernel reported a global OOM and killed
`query_spec-bcbf` with 25,047,920 KiB anonymous RSS during the interrupted mutant
continuation. Probe-only address-space limits did not contain this verification
process. Preserve that interruption as failure evidence; do not count a cgroup
OOM, timeout or resource refusal as a successful verification result.

## Remote resource discipline

The existing remote-host contract names `digitalocean-01` and `digitalocean-02`,
accessed as `dev` through OpenSSH over Tailscale. Host availability, installed
project state and resources must be inspected before each remote workload;
these names do not establish current capacity. Keep the base access
infrastructure unchanged.

On each host, inspect CPU quotas, available RAM/swap, disk space, ancestor cgroup
limits and active services. Choose and verify host-appropriate cgroup v2 memory,
swap, CPU, task and deadline limits for the complete project process tree before
launch. Refuse an uncapped fallback. The workstation's 20 GiB allowance applies
only locally; its launcher has a workstation-specific data-drive path. Use an
owned disk-backed directory on the remote host, not an assumed copy of that path.
Do not run loads on `digitalocean-01` while its webserver is serving.

Remote machines can supply edge-source traffic to a receiver in a scoped
forwarding experiment. Record source/receiver identities, revision, workload,
network path, limits, exact delivered records, durable ACK/custody results,
latency and resource observations on both ends. At completion, stop owned
processes and transient services, retain useful observations and failure
fixtures, and remove owned temporary data on both machines; record the cleanup
outcome. Do not change installed defaults, open public ports, or claim deployment
qualification from an exploratory forwarding run.

## Verification tools

Each tool answers one question ([verification strategy](formal/verification-strategy.md), [ADR-0021](decisions/ADR-0021-add-property-model-fuzz-and-simulation-checks.md)). Use the one that fits the change; every new check ships with a registered mutant or control that makes it fail.

| When you change | Also run | Tool and where it lives |
| --- | --- | --- |
| a kernel in `fabric-core` | `cargo test -p fabric-properties`; `bash formal/kani/check.sh` | proptest in [fabric-properties](../crates/fabric-properties/tests/); Kani harnesses in [proofs.rs](../crates/fabric-core/src/proofs.rs), compiled only under `cfg(kani)` |
| a decoder of untrusted bytes, or frame/journal recovery | `cargo test -p fabric-fuzz-targets --test corpus`; `bash fuzz/smoke.sh 60` | cargo-fuzz targets in [fuzz/](../fuzz/), bodies in [fabric-fuzz-targets](../crates/fabric-fuzz-targets/src/lib.rs) |
| delivery, HTTP handlers or the commit path | `cargo test -p fabric-sim` | turmoil simulation in [fabric-sim](../crates/fabric-sim/tests/delivery.rs) |
| `Cargo.toml` or `Cargo.lock` | `cargo deny --locked check` | cargo-deny policy in [deny.toml](../deny.toml) |
| anything | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Clippy on every crate |

Rules for these tools:

- **State properties from the contract, not the code.** A property that restates the implementation can only agree with it. Cite the ADR or view each property comes from.
- **Keep test-only crates in the verification layer.** proptest, turmoil and HTTP clients go into `fabric-properties`, `fabric-fuzz-targets`, `fabric-sim` or a new crate assigned to `verification` in [layers.json](architecture/layers.json); never into a product crate, and never into `fabric-core`, whose dependency policy stays empty.
- **Use stable Kani features only.** Write plain `#[kani::proof]` harnesses with `kani::any()`; do not use function contracts, autoharness or other `-Z` features until they stabilize. State each harness's bound when it has one.
- **Keep what the tools find.** A fuzz crash goes into `fuzz/regressions/<target>/` with its fix, so the stable replay keeps it fixed. A Kani or property counterexample becomes a named regression test and an entry in [counterexamples.json](formal/counterexamples.json). Never widen a generator, lower a bound or loosen a property to make a failure go away.
- **Coverage is a report.** `cargo llvm-cov --workspace --summary-only` shows what ran; do not add tests only to raise it.
- **Tool setup.** `cargo install --locked cargo-deny cargo-llvm-cov cargo-fuzz kani-verifier && cargo kani setup`; fuzzing also needs `rustup toolchain install nightly`. The extended profile reports a missing tool as INCOMPLETE (exit 3), never as a pass.

## Skills and hooks

Repository skills in `.agents/skills` are ordinary Markdown: [fabric-architecture](../.agents/skills/fabric-architecture/SKILL.md), [fabric-lesson](../.agents/skills/fabric-lesson/SKILL.md) and [fabric-experiment](../.agents/skills/fabric-experiment/SKILL.md). [.codex/hooks.json](../.codex/hooks.json) runs [read-only handlers](../.codex/hooks/run.py): `SessionStart` supplies a bounded copy of `CURRENT.md`. The Bun documentation `Stop` hook is disabled by owner direction during experimentation; the checker remains available manually. Hooks need runtime trust in Codex (`/hooks`). The same configuration holds silent [agent-telemetry](../tools/telemetry/README.md) observers that write under ignored `.local/`; validate them with `python3 -B tools/telemetry/test_contract.py` and `python3 -B tools/telemetry/test_cli.py`.

## Editor tools

[.vscode/extensions.json](../.vscode/extensions.json) recommends [Foam](https://marketplace.visualstudio.com/items?itemName=foam.foam-vscode), [Mermaid Preview](https://marketplace.visualstudio.com/items?itemName=vstirbu.vscode-mermaid-preview), [Mermaid Visual Diff](https://marketplace.visualstudio.com/items?itemName=orhymed.mermaid-visual-diff) and [CodeTour](https://marketplace.visualstudio.com/items?itemName=vsls-contrib.codetour). None is required; all navigation uses ordinary Markdown links.
