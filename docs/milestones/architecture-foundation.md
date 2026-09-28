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
