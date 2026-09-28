# Milestone: architecture foundation

Status: complete on this branch, pending the owner's review and merge. This record is the milestone's baseline, migration map and
completion report. It is not a release record and is not qualification evidence.

## Baseline

| Item | Value |
| --- | --- |
| Source branch | `alpha/controlled-linux-collection` (historical source branch; kept until its work is integrated) |
| Base SHA | `9b3a2b43f8780f8e5a00f8aa832f453eda42a4c1` (`Record the stopping point and what was not run`), re-resolved with `git fetch origin` on 2026-09-28; unchanged from the reviewed head |
| Working branch | `claude/fabrico11y-architecture-foundation-iqbw82`, fast-forwarded from `main` (`43d709b`, an ancestor of the base) to the base SHA. The session's designated push branch replaces the requested name `milestone/architecture-foundation`; the milestone name is used everywhere else |
| Working tree at start | clean (`git status --short` printed nothing) |

### Workspace shape at the base

```text
fabric_o11y (root package)   lib: Event model, FOL2 EventLog, buffer, generator,
                              alpha::{frame, journal, host, log_source, node, sender}
                              bins: fabric_o11y (demo), fabric-node, fabricctl
crates/fabric-server         Fabric Server: HTTP/TLS, commit thread, dedup, control,
                              Parquet segments, sealer, retention, query
                              depends on fabric_o11y for FrameLog and Batch
tools/*                       excluded research packages and Python harnesses
```

There was no dependency rule, no pure core, and the server linked the whole
node library (ureq, libc, the Linux sampler) to reach the frame log.

### Checks at the base (this container, rustc 1.94.1, Python 3.11.15, Bun 1.3.11)

| Command | Exit | Result |
| --- | --- | --- |
| `cargo fmt --all --check` | 0 | clean |
| `cargo test --workspace --locked --all-features` | 0 | 73 passed, 0 failed |
| `python3 -B tools/alpha/test_runner.py` | 0 | 18 tests OK |
| `python3 -B tools/alpha/test_workload.py` | 0 | 5 tests OK |
| `python3 -B tools/alpha/test_delivery_oracle.py` | 0 | 40 tests OK |
| `python3 -B tools/alpha/test_query_oracle.py` | 0 | 49 tests OK |
| `bun tools/docs/check.mjs` | 0 | Documentation checks passed |
| `bun tools/docs/check.test.mjs` | 0 | 40/40 acceptance probes |
| `bun install --cwd tools/docs --frozen-lockfile` | 1 | environment unavailable: Bun 1.3.11 cannot parse the Bun 1.4 lockfile |
| `python3 -B tools/docs/test_hooks.py` | 1 | environment unavailable: needs the frozen install above; not a hook failure |
| `python3 -B tools/telemetry/test_contract.py`, `test_cli.py` | 0 | OK |

The query oracle suite was not in Rust CI at the base; CI ran the runner,
workload and delivery-oracle suites only.

### Outstanding qualification at the base

Taken from the stopping point recorded in the base `docs/CURRENT.md` and the
phase ledger. None of it is resumed by this milestone.

| Work | State at base |
| --- | --- |
| Registered history trials (1,000,000-record query latency, freshness at 1,000 identities) | not run |
| Journal-only versus segment-backed comparison | not run |
| Outage-and-drain qualification | interrupted: the first trial was stopped mid-run and produced no result |
| Stress and burst qualification | not run |
| Running-installation acceptance (needs root on a disposable host) | not run |
| Soak | not run |
| Release readiness, tag | not performed |

### Source-of-truth conflicts found at the base

- The root README described a single-process FOL2 prototype and said real
  collectors, network ingestion and a query engine were future work; the
  branch implements all three.
- `docs/ALPHA.md` (contract and phase ledger) and `docs/ALPHA-PLAN.md`
  (ordering and open decisions) overlapped, and both were release-stage
  documents rather than capability documents.
- `docs/CURRENT.md` mixed the product state, the stopping point, research
  results and early learning stages in one page.
- Completion criteria required cross-family language-model review; an
  unavailable model blocked phase promotion.
- `docs/architecture.md` (the blueprint) is a proposal but was reachable as if
  it were a peer of the current architecture views.

## Workspace shape after

```text
crates/fabric-core     core              no_std; SpindleId, StrandId, next_sequence, delivery decision
crates/fabric-ports    ports             DurableJournal, Clock
crates/fabric-app      app               commit_group (delivery use case)
crates/fabric-frame    adapter support   FAB1 frame log, version-one Batch envelope
crates/fabric-server   composition root  HTTP/TLS, journal adapter, control, segments, sealer, query, main
fabric_o11y (root)     composition root  src/spindle (Spindle runtime, Spool, host, logs, sender),
                                         fabric-node, fabricctl, FOL2 demonstration
xtask                  tooling           check-layers, check-core-purity, checks, mutants
```

`fabric-server` no longer links the root package except in its end-to-end tests (a documented dev-only exception).

## Migration map

| Before (base `9b3a2b4`) | After |
| --- | --- |
| `src/alpha/frame.rs` | `crates/fabric-frame/src/frame.rs` (moved unchanged apart from test scratch paths and a doc link) |
| `src/alpha/journal.rs`: `Batch`, `Cursor`, caps, `validate` | `crates/fabric-frame/src/envelope.rs` (moved unchanged) |
| `src/alpha/journal.rs`: `Journal` | `src/spindle/spool.rs`: `Spool` |
| `src/alpha/node.rs`: `Node` | `src/spindle/runtime.rs`: `Spindle` |
| `src/alpha/{host,log_source,sender}.rs` | `src/spindle/` |
| `crates/fabric-server/src/store.rs` decision loop | `fabric_core::delivery` + `fabric_app::delivery::commit_group`; `Store` keeps the effects |
| `tests/alpha_{journal,log_source,node}.rs` | `tests/{spool,log_source,spindle}.rs` |
| `examples/alpha_{spool_dump,node_sim,native_dump}.rs` | `examples/{spool_dump,spindle_sim,native_dump}.rs` |
| `tools/alpha/*` | `tools/qualification/*` (seven files byte-identical; the rest differ only in path strings) |
| `docs/ALPHA.md` | `docs/PRODUCT-CONTRACT.md` and `docs/QUALIFICATION.md` (numbers, defaults and gates carried verbatim) |
| `docs/ALPHA-PLAN.md` | `docs/ROADMAP.md` (settled decisions and open steps) |
| `docs/architecture/node.md`, `docs/diagrams/alpha-node.mmd` | `docs/architecture/spindle.md`, `docs/diagrams/spindle.mmd` |
| `docs/architecture/system.md`, `docs/diagrams/system.mmd` (demo) | `docs/architecture/fol2-demo.md`, `docs/diagrams/fol2-demo.mmd`; new product `system.md`, `system.mmd`, `layers.mmd` |
| `AGENTS.md` documentation policy sections | `docs/documentation-policy.md` (unchanged text); `AGENTS.md` is now the compact operational contract |

Historical evidence was not rewritten. Experiment records, review archives and ADR-0010 keep their names and content; Markdown link targets in them were updated to follow moved files, and nothing else. Two oracle specifications (`DELIVERY_ORACLE.md`, `QUERY_ORACLE.md`) and three oracle scripts changed only in path strings and link targets; their rules are unchanged. The removed documents remain readable with `git show 9b3a2b4:docs/ALPHA.md` and `git show 9b3a2b4:docs/ALPHA-PLAN.md`.

Frozen identifiers that keep a release-stage spelling because protocol bytes or the runner's safety checks depend on them are listed in [qualification](../QUALIFICATION.md#frozen-harness-identifiers).

## Accepted nomenclature

Spindle (host runtime role) and Strand (`(SpindleId, generation)` lineage) are canonical; every other name stays functional ([ADR-0017](../decisions/ADR-0017-name-the-spindle-and-the-strand.md)). The executable stays `fabric-node`; wire and persisted names (`node_id`, `/v1/admin/nodes`, `streams.json`) are unchanged.

## Decisions recorded

[ADR-0015](../decisions/ADR-0015-adopt-a-hexagonal-architecture.md) hexagonal architecture, [ADR-0016](../decisions/ADR-0016-keep-a-pure-semantic-core.md) pure `no_std` core, [ADR-0017](../decisions/ADR-0017-name-the-spindle-and-the-strand.md) Spindle and Strand, [ADR-0018](../decisions/ADR-0018-accept-work-on-executable-evidence.md) executable evidence instead of model review, [ADR-0019](../decisions/ADR-0019-keep-release-maturity-in-tags.md) release maturity in tags. No existing ADR was renumbered; ADR-0013's text is unchanged apart from a link target.

## Behavior preservation of the delivery extraction

1. Accepted behavior: the ADR-0013 table as implemented in `Store::commit` at `9b3a2b4`.
2. Guards: the independent delivery oracle, the server's end-to-end delivery tests, the fault harness.
3. A frozen transcription of the base loop is compared with the new use case on every group of up to two offers over three Strands in three journal modes and every three-offer group over two Strands; no disagreement.
4. Counterexample kept: `last + 1` overflow at `u64::MAX` (debug panic, release wrap) is now `next_sequence` returning `None`.
5. The old loop was removed only after the differential test, the workspace tests and the fault reruns passed.

Fault reruns on the extracted kernel, same command as the historical record (release build of this branch; three Spindles, 20 s): `clean`, `server-kill`, `node-kill`, `outage` for seeds 1 and 2 all exit 0 with the oracle passing; `--mutate drop-recovered` exits 1 with `ACKED-DURABLE` as intended. Output: [delivery-fault-rerun.txt](../experiments/benchmarks/data/architecture-foundation/delivery-fault-rerun.txt).

## Semantic mutants

Registry: [xtask/mutants.json](../../xtask/mutants.json). Runs are kept under [data/architecture-foundation](../experiments/benchmarks/data/architecture-foundation/delivery-fault-rerun.txt).

- Run 01 (`semantic-mutants-run-01.txt`): 7 caught, 3 caught elsewhere, 2 inconclusive. The non-caught rows were runner defects: the copied tree had no `target/` directory for test scratch files, and Cargo stopped at the first failing test binary before the named test ran. Both were fixed in the runner.
- Run 02 (`semantic-mutants-run-02.txt`): 11 caught; **`M-SPOOL-RECLAIM` survived**. Deleting the sealed Spool file that holds the first unacknowledged Batch passed every existing test because no test put a file boundary at `through + 1`. The regression test `reclaim_keeps_the_file_holding_the_first_unacknowledged_batch` now kills it (caught, and only that test fails).
- Run 03 (`semantic-mutants-run-03.txt`), after the fix: all 12 caught by their named test; `cargo xtask mutants` exit 0.

`cargo-mutants` 27.1.0 calibration on `fabric-core` and `fabric-app`: 34 mutants, 27 caught, 6 unviable, 1 missed and classified **equivalent** (`sequence < last` to `<=` after the `sequence == last` branch). Details in the [verification strategy](../formal/verification-strategy.md#mutation-policy).

## Completion report

| Item | Value |
| --- | --- |
| Source branch | `alpha/controlled-linux-collection` (kept; not deleted) |
| Base SHA | `9b3a2b43f8780f8e5a00f8aa832f453eda42a4c1` |
| Working branch | `claude/fabrico11y-architecture-foundation-iqbw82` (the session's designated branch, standing in for `milestone/architecture-foundation`) |
| Final SHA | the head of the working branch that contains this record; see `git log` |

### Gates

| Command | Exit | Result |
| --- | --- | --- |
| `cargo xtask check-layers` | 0 | passed, declared and resolved metadata |
| `cargo xtask check-core-purity` | 0 | passed, declared and resolved metadata |
| `no_std` experiment | — | `fabric-core` builds as `no_std` with no dependencies; see the [record](../experiments/formal/core-no-std.md) |

### Verification commands executed on the final tree

`cargo xtask checks --profile fast` exited 0 with all fifteen checks passed (rustc 1.94.1, Python 3.11.15, Bun 1.4.0 installed locally for the hook test). Receipts: [receipts-fast](../experiments/benchmarks/data/architecture-foundation/receipts/test.json).

| Check | Command | Exit |
| --- | --- | --- |
| fmt | `cargo fmt --all --check` | 0 |
| check | `cargo check --workspace --locked --all-targets` | 0 |
| test | `cargo test --workspace --locked --all-features` (111 tests) | 0 |
| clippy on core, ports, app | `cargo clippy -p fabric-core -p fabric-ports -p fabric-app --locked -- -D warnings` | 0 |
| layers, core purity | `cargo xtask check-layers`, `cargo xtask check-core-purity` | 0, 0 |
| qualification runner | `python3 -B tools/qualification/test_runner.py` (18 tests) | 0 |
| workload and rate oracle | `python3 -B tools/qualification/test_workload.py` (5 tests) | 0 |
| delivery oracle | `python3 -B tools/qualification/test_delivery_oracle.py` (40 tests) | 0 |
| query oracle | `python3 -B tools/qualification/test_query_oracle.py` (49 tests) | 0 |
| docs, checker probes, hooks | `bun tools/docs/check.mjs`, `bun tools/docs/check.test.mjs`, `python3 -B tools/docs/test_hooks.py` | 0, 0, 0 |
| telemetry | `test_contract.py`, `test_cli.py` | 0, 0 |
| TLA+ delivery (extended) | `cargo xtask checks --only tla-delivery` with TLC v1.7.1 (SHA-256 verified) | 0: Safe passes; EarlyAck and LostCopy give their expected counterexamples |
| TLA+ transport (extended) | `cargo xtask checks --only tla-transport` | 0 |
| delivery fault harness (extended) | eight scenarios and one negative control, above | 0 each; negative control 1 as intended |
| semantic mutants (extended) | `cargo xtask mutants` | 0: 12 of 12 caught by the named test |

### Negative controls executed

- Layer gate fixtures: core to adapter, core to app, ports to adapter, app to adapter, renamed, optional feature-activated, target-specific, build, development and transitive forbidden edges; unassigned member and stale policy entry; a dev-only exception that must not excuse a normal edge. Each fails with its category.
- Purity gate fixtures: `rand` (randomness), `tokio` (async runtime), renamed `ureq` (HTTP client), unreviewed crate, unreviewed dev crate, build script, feature, missing `#![no_std]`, `extern crate std`, static atomic, a wrapper crate linking `tokio` (transitive); an allowlisted pure crate passes.
- Repository-level: `ureq` injected into `fabric-core` fails the purity gate (`PURITY_DENIED_DEPENDENCY`); `fabric-server` injected into `fabric-core` fails the layer gate (`LAYER_FORBIDDEN_EDGE`); `std::time::SystemTime` in the core fails to compile; `extern crate std` compiles but fails the purity gate.
- Oracle mutation controls (unchanged, in the suites above): delivery oracle `test_mutant_drop_one_acked_recovered_record`, `test_mutant_duplicate_one_recovered_record`, `test_mutant_replace_one_recovered_bytes`, `test_mutant_node_forgets_unacked_batch_early`; query oracle `TestPaginationMutants`, `TestRateMutants` (for example `test_wrong_rate_at_reset`); rate oracle `test_rate_oracle_rejects_overrun_and_balance`.
- Fault harness `--mutate drop-recovered` fails with `ACKED-DURABLE`.

### Oracle revisions

The oracles' logic is unchanged; their files moved and changed only in path strings and link targets. At the final tree: `delivery_oracle.py` blob `5d27dfb`, `query_oracle.py` blob `ac04517`, `rate_oracle.py` blob `53c94f2` (identical to base).

### Historical evidence

No experiment record, run output, hash list or review archive was rewritten; only Markdown link targets were updated. Historical results remain attributed to the revisions they name, and the [capability ledger](../QUALIFICATION.md#capability-ledger) does not relabel them as evidence for this branch.

### Qualification that remains outstanding

History query latency, freshness and journal-versus-Segment comparison (not run); outage and drain (interrupted, no result); stress and burst (not run); soak (not run, unregistered); running installation (not run); release readiness (not performed).

### Known architectural limitations

- Both composition roots still contain adapters and domain policy: control transitions, query and rate semantics, retention eligibility and collection cursor rules are not in the core, so the layer gate cannot protect them.
- One documented exception: `fabric-server` end-to-end tests depend on the root package (dev only).
- The purity gate's source guard is syntactic.
- Receipts are unsigned and written locally or by CI; they prove structure, not execution.
- The fault-harness transcript is not yet mapped to TLA+ actions.

### Known surviving mutations

None unexplained. One `cargo-mutants` survivor is classified equivalent (above). The semantic mutant `M-SPOOL-RECLAIM` survived once and is now caught by a new regression test.

### Unchecked behavior

Physical power loss and device fsync honesty; direct journal/Segment and before/after-seal metamorphic comparisons; property-based tests; multi-host networks; everything listed as not run in the ledger. The [verification matrix](../formal/verification-matrix.md) lists unchecked behavior per claim.

### Future milestones

Verification foundation, semantic kernels, history qualification, delivery and recovery qualification, Linux installation qualification, release readiness ([roadmap](../ROADMAP.md)).

### Statements

- Architecture foundation: **COMPLETE** for the scope of this milestone (the remaining kernel extractions are assigned to the semantic-kernels milestone).
- Behavioral regression checks: **PASS**.
- Qualification: **OUTSTANDING**.
- Release: **NOT PERFORMED**. No tag was created, nothing was merged, and the source branch was not deleted.
