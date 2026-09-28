# Milestone: architecture foundation

Status: in progress. This record is the milestone's baseline, migration map and
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
