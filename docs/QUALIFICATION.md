# Qualification

This page owns how a [product contract](PRODUCT-CONTRACT.md) claim becomes *qualified* for the target operating profile: the registered workload, the decision rule, the harness boundary, and the ledger of what has and has not been run. It migrated from the release-stage contract and completion plan at base `9b3a2b4`; seeds, durations, identity counts, query mix, percentiles, thresholds, budgets and measurement windows are unchanged. Release maturity is not qualification: a release is a tag on a revision whose required gates have recorded evidence ([ADR-0019](decisions/ADR-0019-keep-release-maturity-in-tags.md)).

## Evidence states

A claim has exactly one of these states per revision. They are not a ladder; a green test suite is never "qualified".

| State | Meaning |
| --- | --- |
| Implemented | Code exists that is intended to satisfy the claim. |
| Tested | A named automated check exercises the claim and passed on the stated revision. |
| Measured | A registered protocol ran and its numbers are recorded, whether or not they pass. |
| Qualified | Every gate of the registered protocol passed on the stated revision under the target profile. |
| Not run | The gate exists and has never produced a result. |
| Interrupted | A run started and was stopped before producing a result; it stays interrupted until a new run is recorded. |
| Failed | A run produced a result that violates a gate. |
| Inconclusive | A run finished but its result cannot decide the gate (for example, a perturbation flag). |

Historical results belong to the revision they ran on. A later refactor inherits none of them; the [verification matrix](formal/verification-matrix.md) says which current checks still exercise each claim.

## Registered workload and decision rule

Run each of 10, 100 and 1000 distinct simulated Spindle identities. Per identity offer 2 logs/s (mean body 512 bytes, half repetitive and half seeded high entropy) and 32 metric points/15 s. Use three fixed seeds `0xA11FA001`, `0xA11FA002`, `0xA11FA003`, one trial per seed and tier, 15 s warmup plus 120 s measured. The largest tier offers about 4,133 records/s. The schedule is open loop: report offered, admitted, committed and backlog separately. Simulated identities do not represent 1000 deployed processes or networks. Separately report native Spindle process overhead. Preflight all source and output footprints.

Per trial report CPU, peak RSS, offered/admitted/committed throughput, durable ACK and query p50/p99, observation-to-query freshness, recovery time, and **all live bytes** in Spindle Spool, journal, Segments, indexes, temporary files and test logs. The gate is zero oracle mismatches, missing ACKed records or retry-created logical duplicates under declared process-crash/retry and successful-sync assumptions; no growing backlog at offered rate; durable ACK p99 ≤ 1 s; observation-to-query p99 ≤ 5 s; registered host/time/log/metric queries p99 ≤ 2 s over an independent 1,000,000-record fixture; central peak RSS ≤ 2 GiB with four logical CPUs; one real Spindle buffers 30 minutes and drains within 10 minutes after reconnect. The native-Spindle gate is peak RSS ≤ 64 MiB on the registered source workload. Deliberate 5× bursts test bounds/correctness, not steady latency. Include overload, auth rejection and concurrent management/query. Central control additionally requires a healthy apply within 30 s at each tier.

Each invocation has a 5 GiB live-data ceiling, 50 MiB retained-evidence ceiling and 2 h wall limit. Stop on breach and mark incomplete. Tiers and trials run sequentially. Keep manifests, seeds, source hashes, environment, latency histograms, command exits and minimized counterexamples, not bulk telemetry, in Git. Disposable data stays under ignored `target/alpha-*` (a frozen harness identifier, below); cleanup verifies ownership and rejects symlinks or path escape. A checker must be tested with representative injected rate, duration and disk-budget violations. A failed or unrun gate is never passed.

Measure both individual and grouped commits using **identical durability semantics**, with a nominal 50 ms or 1 MiB coalescing threshold. Compare journal-only and Segment-backed reads with the same query and complete live-byte accounting. Historical [Stage 6](experiments/benchmarks/local-log-stage6.md) and [layout results](experiments/benchmarks/research-costs-run-01.md) are prior research, not a baseline for this profile.

If an architecture change requires changing a registered protocol, register a new protocol revision before running the new measurement. A smoke test is not a qualification result, and a performance comparison is not deterministic proof.

## Harness boundary

The [seeded source](../tools/qualification/workload.py) streams exactly the registered log and metric **offer shapes** for a tier and seed; each log body is 512 ASCII bytes, alternating repetitive and seeded high-entropy content. It records scheduled times, and `--paced` follows those deadlines while recording actual emission lag. It writes disposable NDJSON fixture records, not OTLP payloads or accepted application telemetry. Its byte cap is preflighted and checked before each write. The independent [rate oracle](../tools/qualification/rate_oracle.py) checks every second's closed-form offered counts, admission/commit/backlog/gap arithmetic, exact source fields and generated values against one of the three frozen seeds; injected overrate, source corruption and missing-source controls fail. The [runner](../tools/qualification/runner.py) validates finite limits at its API and CLI, marks each invocation in progress before launch, requires fresh gated artifacts, and sets `TMPDIR` inside its owned output tree. On Linux it uses an invocation token, a subreaper and pidfds to find, stop and reap tagged descendants even if they enter a new session. It samples elapsed time, tagged descendant RSS and all regular bytes under the owned tree. It checks limits again after child exit and reserves room for its result.

The runner is a polling watchdog, not a hard filesystem quota for arbitrary child commands; a fast writer can transiently exceed the cap before detection, especially if it writes and deletes between samples or writes outside the owned tree. The source's cooperative write cap and the runner's measured `peak_live_bytes_sampled` have different guarantees. Descendant discovery assumes trusted commands inherit the invocation environment token; deliberate environment scrubbing is outside this supervisor contract. The rate CSV is a source accounting claim; the exact source-file oracle checks generated fixtures but does not prove real-Spindle offer timing or end-to-end admission. Fleet qualification must compare source offers to server commits and timestamps using an independent executable oracle and apply its separate no-growing-backlog gate.

The fixed two-second, ten-identity fixture for seed `0xA11FA001` has 360 records, 49,196 NDJSON source bytes and SHA-256 `a6e5fca651aabb25fa954c51903fd35f837bc7e244712c311f6ee9765d164f15`. The independent test fixes those values, the first literal record, source identities and the closed-form rate. This fixture is a checker control, not a measured fleet trial.

### Frozen harness identifiers

The harness moved from `tools/alpha/` to [tools/qualification/](../tools/qualification/) in the architecture-foundation milestone. Diffs are limited to path strings; `runner.py`, `workload.py`, `rate_oracle.py`, `native_phase1.py`, `test_runner.py`, `test_workload.py` and `test_delivery_oracle.py` keep their exact blob hashes. The example binaries the harness launches were renamed (`alpha_spool_dump` → `spool_dump`, `alpha_node_sim` → `spindle_sim`, `alpha_native_dump` → `native_dump`). Registered protocols and run records written before the move keep their original command text; substitute the new paths when resuming them.

These identifiers keep their historical spelling because generated bytes or the runner's safety checks depend on them. Changing any of them is a verifier change that needs a new protocol revision:

| Identifier | Why it is frozen |
| --- | --- |
| `fabric-alpha-v1:` seed prefix in `workload.py`, `rate_oracle.py` and `examples/spindle_sim.rs` | Input to every generated high-entropy body; the fixture SHA-256 above depends on it |
| `.fabric-alpha-owned` containing `fabric-alpha-runner-v1` | The runner's ownership marker; cleanup refuses directories without it |
| `target/alpha-*` output directories | The runner and tier scripts refuse any other output location |

## Tooling classes

| Class | Purpose | Location |
| --- | --- | --- |
| Qualification tooling | Tests product profile claims under registered protocols | [tools/qualification](../tools/qualification/) runner, workload and tier scripts |
| Independent oracles | Decide semantic correctness independently of the Rust implementation | [delivery oracle](../tools/qualification/DELIVERY_ORACLE.md), [query oracle](../tools/qualification/QUERY_ORACLE.md), [rate oracle](../tools/qualification/rate_oracle.py) (Python, frozen before the code they grade) |
| Research tools | Explore hypotheses and candidate mechanisms; results do not become product behavior without an ADR | `tools/storage-probe`, `tools/layout-probe`, `tools/transport-sim`, `tools/seal-probe`, `tools/bench` |
| Development tools | Repository mechanics | `xtask` (layer, purity and check registry), `tools/docs` |
| Agent telemetry | Observes the development workflow; establishes no product correctness | `tools/telemetry`, `formal/agent-telemetry` |

## Capability ledger

States are for the revision named in each linked record, not for the current head. The phase names in record file names are historical provenance.

| Capability | Gate | State | Evidence |
| --- | --- | --- | --- |
| Harness | Seeded source, runner, rate oracle and their negative controls | Tested (current CI) | [baseline](experiments/benchmarks/alpha-phase0-baseline.md), [review response](experiments/benchmarks/alpha-phase0-review-response.md) |
| Spindle local collection | Native RSS ≤ 64 MiB, exact replay, restart, source/cursor/reset/failure tests | Measured and passing on `c51d3a8` (three seeds, VmHWM ≤ 2.7 MiB). Promotion was withheld then only for cross-family review, which is no longer a gate; the Spool has changed since, so it is not re-qualified on the current head | [native run 02](experiments/benchmarks/alpha-phase1-native-run-02.md), [close-out review](experiments/formal/alpha-phase1-closeout-review.md) |
| Delivery under process faults | Zero delivery-oracle mismatches under server kill, Spindle kill, outage | Tested on the recorded revision; rerun in this milestone (see [milestone record](milestones/architecture-foundation.md)) | [fault run 01](experiments/formal/alpha-phase2-delivery-faults.md) |
| Delivery at ten real Spindles | ACK p99 ≤ 1 s, no growing backlog, both commit modes with identical durability | Measured and passing on `71fef99` (ACK p99 ≤ 79 ms) | [delivery run 01](experiments/benchmarks/alpha-phase2-delivery-run-01.md) |
| Fleet delivery and control at 10/100/1000 identities | ACK p99 ≤ 1 s, central RSS ≤ 2 GiB, apply ≤ 30 s | Measured and passing on `4921e5e` (ACK p99 ≤ 74 ms, apply ≤ 5.4 s) | [fleet run 01](experiments/benchmarks/alpha-phase3-fleet-run-01.md) |
| History query latency | Registered queries p99 ≤ 2 s over 1,000,000 records | Measured and passing under [revision 2](experiments/benchmarks/history-protocol-r2.md) on `63bbeac`, four-CPU host: every graded answer oracle-exact, p99 ≤ 481 ms with Segments, ≤ 1,515 ms journal-only. Not qualified: revision 1 on the target profile has not run | [history protocol](experiments/benchmarks/alpha-phase4-history-protocol.md), [history run 01](experiments/benchmarks/history-run-01.md) |
| Freshness | Observation-to-query p99 ≤ 5 s at 1,000 identities | Measured and passing under [revision 2](experiments/benchmarks/history-protocol-r2.md) on `63bbeac`, four-CPU host: p99 ≤ 1.78 s in each Segment-mode trial. Not qualified on the target profile | [history run 01](experiments/benchmarks/history-run-01.md) |
| Journal-only versus Segment reads | Latency and live bytes, same queries | Measured and passing under [revision 2](experiments/benchmarks/history-protocol-r2.md) on `63bbeac`, four-CPU host: Segments about 3 to 4 times faster at p99 and 242 MiB against 304 MiB live | [history run 01](experiments/benchmarks/history-run-01.md), [ADR-0020](decisions/ADR-0020-store-sealed-history-as-parquet-segments.md) |
| Outage and drain | 30 min buffered, drain ≤ 10 min, exact delivery | Interrupted: the first trial was stopped mid-run and produced no result | [outage protocol](experiments/benchmarks/alpha-phase5-outage-protocol.md) |
| Burst, rejection, concurrent management | Oracle-exact, bounded backlog, RSS ≤ 2 GiB | Not run | [stress protocol](experiments/benchmarks/alpha-phase5-stress-protocol.md) |
| Soak | Within the 2 h invocation limit | Not run; no protocol registered | — |
| Packaging | Reproducible `.deb`, sysusers dry run, collision refusal, `systemd-analyze verify` | Tested (static) | [packaging static checks](experiments/formal/alpha-phase5-packaging-static.md) |
| Running installation | The installation acceptance list in the [product contract](PRODUCT-CONTRACT.md#linux-installation-contract) | Not run (needs root on a disposable host) | — |
| Release readiness | Every gate above has a recorded command, exit and evidence | Not performed | [roadmap](ROADMAP.md) |

Promote a capability only after recording the command, exit status and evidence for every gate. If a gate fails, preserve a concise counterexample and the next action in a linked record. No language-model review is a gate ([ADR-0018](decisions/ADR-0018-accept-work-on-executable-evidence.md)); earlier review records remain historical evidence of the defects they found.

## Fast checks versus qualification

The [check registry](../xtask/checks.json) lists every required repository check with its scope, tools and related contracts; `cargo xtask checks --profile fast` runs the fast set and writes verification receipts. Qualification protocols above are never part of the fast profile: they run only under explicit scope, and a missing environment is reported as not run.
