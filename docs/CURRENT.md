# Current project state

## Implemented

FabricO11y collects, durably forwards, retains and queries Linux telemetry. It is
usable for bounded development trials; no current deployment profile is qualified
and no release tag has been produced.
The [product contract](PRODUCT-CONTRACT.md) defines the promises and the
[verification matrix](formal/verification-matrix.md) maps each invariant to checks
and remaining limits.

| Component | Current behavior |
| --- | --- |
| Spindle (`fabric-node`) | Host metrics, selected log files and loopback OTLP/HTTP trace intake; durable FAB1 Spool, exact retry, visible collection gaps, output-rate cap and last-valid remote configuration. |
| Delivery | Verified TLS, per-Spindle credentials, ordered Batches, deduplication and ACK after grouped two-sync journal commit. A quiet sender closes its group after 2 ms. |
| Storage | Bounded external-merge sealing to Zstd Parquet, manifest publication before checkpoint/reclaim, retention by age and bytes. The normal writer uses aligned sorted groups and disk-backed completed pages; estimated input targets admit a single oversized row. No whole-server memory bound is established. |
| Query | Logs, metric points/rates and spans across journal and Segments, snapshot pagination, freshness and completeness. Legacy queries default to Scan. Scoped console queries use Walk with storage-owned source maps and authorization filtering before row selection. Unpaginated rates remain outside the console API. |
| Console and access | Leptos/WASM PWA with local passkeys, scoped human/workload/delegated-agent access, live queries and Spindle controls. New enrollment requests resident credentials with required user verification; independent resident-key browser acceptance remains pending. Finite native and browser checks are recorded below; full release acceptance remains open. |
| Self-observation | Every production server CLI launches and supervises a dedicated local Spindle. Both emit bounded diagnostics through ordinary Spool/TLS/ACK delivery. Edge Spindles retain their configured destination. |
| Packaging | Debian and RPM artifacts include the console, Apache-2.0 license and dependency notices, with static systemd services and a shared slice. Historical Debian acceptance passed seventeen baseline checks and three mutation controls; current exact-candidate lifecycle trials are in progress. |
| Architecture | Pure `no_std` semantic core, effect ports, application transitions and adapters; executable layer and purity gates. |

See the [system view](architecture/system.md), [operations guide](operations.md)
and [architecture index](architecture/README.md) for behavior and configuration.

## Results that shape the design

Measurements below belong to their linked workloads and revisions. Heap, RSS and
cgroup memory are different quantities; isolated gains cannot be multiplied into
a service-capacity claim.

| Investigation | Result | Consequence |
| --- | --- | --- |
| [Bounded sealer](experiments/benchmarks/ingestion-memory-run-01.md), [encoded-page correction](experiments/formal/encoded-page-memory-run-01.md), [readiness continuation](experiments/formal/readiness-continuation-results.md) | Combined aligned-input/disk-PageStore opt-in passed eight cells with three pairs each: 37.3 MiB steady256 heap and 39.5 MiB bigrows64; high-entropy probe 40.07 MiB. Supplemental row/page scratch observations completed. Seven cells were 1.7–11.9% slower; bigrows64 was 2.7% faster on the shared host. | Adopted as defaults, with unflagged recovery tests, loaded-binary equivalence and all 17 fast checks passing. The companion-inclusive full soak passed all ten gates. |
| [Walk](experiments/benchmarks/query-walk-run-01.md), [tail blocks](experiments/benchmarks/block-tail-run-01.md), [text filters](experiments/benchmarks/text-filter-run-01.md) | Large selective-query wins, including a 614→28 ms tail shape. | Keep hierarchical pruning and delayed payload work. Scan remains default pending broader acceptance. |
| [Fixed-demand plans](experiments/benchmarks/query-plan-run-01.md) | Three small-profile pairs favored Walk: median 42.2% lower server CPU and 64.1% lower sampled phase-peak RSS. | Workload-specific evidence; no universal plan winner. |
| [Service/recovery](experiments/benchmarks/service-recovery-research-findings.md) | Two 20-Spindle cases each recovered 60,000 exact logs; Scan/Walk peak RSS 165.7/92.9 MiB. Eighteen selected storage fault/recovery cases passed. | Short bounded service behavior is supported; long-term stability is still open. |
| [Pressure and remote edges](experiments/benchmarks/hammer-reference-findings.md) | Four repaired cells each recovered 1.26 million exact logs. Final Walk peak RSS 285.61 MiB. Real remote collectors and the eight-worker simulator delivered exactly. | Journal identity races were fixed. One missing outer receipt prevents strict campaign closeout; the two-worker simulator retained backlog and misreported timing. |
| [Synthetic RCA](experiments/benchmarks/rca-journal-findings.md) | Fourteen journal/restart cells, 350 exact complete chains and 30 rejected corruptions. | Logs, metrics and traces support tested investigations; published-storage RCA, real application exporters and causal interpretation remain unmeasured. |
| [Self-observation](experiments/formal/server-self-observation-findings.md) | Native TLS delivery, restart identity, source pinning, refusal cases and process cleanup checked under a 4 GB combined cap. Tiny debug fixture: 19.9/8.5 MiB server/Spindle peak RSS. | Dedicated Spindle is implemented. Diagnostic files before Spool commit remain best-effort; colocation cannot observe total host loss. |
| [Cross-system trials](experiments/benchmarks/cross-system-sweep-findings.md), [continuation](experiments/benchmarks/cross-system-continuation-findings.md), [native candidates](experiments/benchmarks/native-frontier-findings.md) | Smaller sort runs and borrowed rows reduced allocations in selected fixtures. Larger-input models, timing guards and one RSS guard failed. | Retain the mechanisms and counterexamples; no storage-engine migration, allocator tuning or rejected candidate became a default. |

The [experiment index](experiments/README.md) retains protocols and failures.
The [research synthesis](research/cross-system-source-synthesis.md) and
[coupled model review](research/coupled-performance-review.md) separate upstream ideas, measurements and hypotheses.

## Invariants and reliability

Delivery, Spool, control, history/query, architecture, sealer, observation encoding
and query semantics are tracked in the [verification matrix](formal/verification-matrix.md).
The independent Python delivery/query/rate oracles remain the semantic reference.
Properties, corpus replay, simulation and negative controls supplement them.
The continuation records eleven bounded Kani proofs, named mutants, transport
TLC and four 60-second fuzz targets with 11,370,454 executions without a crash;
finite checks do not prove every execution. The [invariant audit](experiments/formal/invariant-consolidation.md)
retains minimized launcher, control-publication and Spool-exhaustion defects.

Recent repairs cover journal identity during rotation, publication/reclaim
coverage races, snapshot metadata after publication, HTTP admission before body
allocation, cancellation and startup ownership leaks, oversized-line progress,
exact integer counter rates and reserved diagnostic credentials. Their minimized
failures and checks are linked from the matrix and experiment index.

## Completed readiness continuation

The [readiness continuation](experiments/formal/readiness-continuation-results.md)
has completed the combined builder campaign, recovery/shutdown checks, dependency
policy and finite [installation acceptance](experiments/formal/installation-acceptance-run-02.md).
The [full companion-compatible R2 soak](experiments/benchmarks/soak-run-02.md)
passed all ten gates and its frozen evidence is archived. The
[normal writer](experiments/formal/bounded-writer-default-run-01.md) now enables
aligned groups and disk-backed completed pages: unflagged 27 bounded tests,
13 kill cuts plus no-hit, and loaded-binary equivalence passed. The data-only
runner guard also passed 66 focused tests. Both owned freezes were archived and
removed. All 17 final fast checks and all three manual documentation checks
passed. This completes the registered continuation queue; evidence, failed
attempts and exact revision limits remain in the linked records.

## Release preparation

The [bounded release plan](milestones/release-readiness.md) tracks Debian/Fedora
central and edge deployment, 100 GB retained telemetry with separate working
space, local passkeys, scoped human/workload/AI access, and a Leptos/WASM PWA.
Apache-2.0 licensing, dependency notices and a
[private security-reporting route](../SECURITY.md) are included. No release tag
has been published. The
[release progress page](https://github.com/kmosoti/FabricO11y/wiki/Release-readiness-progress)
links candidate results, failures and the remaining acceptance queue.

Debian build 04 and RPM build 03 contain console build 09, with all 637 common
payload files matching. The console fixes a real 200% zoom pagination overflow;
150 staged Chrome assertions passed, including rejection of an injected overflow.
The latest native change requests resident credentials for all four enrollment
paths. Its two focused regressions passed on `829babe`; independent browser
inventory checks and a nonresident-enrollment negative control are implemented.
New exact packages and their browser acceptance are pending. Earlier credentials
remain usable through explicit principal sign-in. Virtual devices do not
establish physical passkey or mobile compatibility.

The first corrected full million-row Segment trial,
`release-query-segment-seed1-successor-02`, passed all eight gates on Debian
build 04/RPM build 03: 200/200 independent-oracle answers, exact recovery and
companion custody, and query p99 at most 504.340 ms before restart and 447.509 ms
afterward. This confirms the bounded scoped-evidence cache through production
HTTPS on that package pair. The earlier 10.27-second five-page failure and
library comparisons remain in the
[cache report](https://github.com/kmosoti/FabricO11y/wiki/Experiment-benchmarks-scoped-evidence-cache-run-01).
These results are not automatically attributed to the resident-enrollment successor.

Full soak `production-soak-full-03` failed after 1,587.618 seconds: its aggregate
storage census could not traverse temporary directories owned by a concurrent
rootless package build. The harness stopped rather than ignoring uncounted disk
usage. No Fabric crash was observed. Container/package builds now complete and
clean up before measured service fixtures start. This failed run and earlier
interrupted runs retain their original results and cleanup receipts in the
[campaign record](https://github.com/kmosoti/FabricO11y/wiki/Experiment-benchmarks-production-access-campaign-run-01).
The resident-enrollment packages will receive a fresh full soak. The final
query/main/pressure/outage matrix, Debian/Fedora installation and lifecycle,
four cross-family forwarding cells, and full packaged browser/PWA acceptance
remain queued. Historical Fedora installation/reboot/removal evidence belongs
to its original `c06688c` candidate, including the
[Btrfs identity repair](decisions/ADR-0028-preserve-btrfs-log-identity-across-reboots.md).

Firefox's actual virtual-passkey diagnostic completed 127 assertions after two
fixture repairs: software transport selection and CTAP 2.1 credential-ID handling.
Those causal diagnostics retain their exact scope; standalone resident-key
acceptance is still pending. The repaired Chrome fixture also needs its full
cursor-expiry, recovery and installed-PWA lifecycle run on the successor package.

[Hosted CI 38072930139](https://github.com/kmosoti/FabricO11y/actions/runs/38072930139)
at `8597c42` passed all 20 enabled fast checks, whole-workspace dependency policy,
WASM/build/contrast checks, Firefox shell checks, 139 actual Chrome assertions,
the API-cache negative control and packaging checks. A fresh CI run on the
resident-enrollment change is in progress. CI does not replace the longer
exact-package browser or service campaigns. Bun CI/hooks remain disabled;
documentation checks run manually.

The [research wiki](https://github.com/kmosoti/FabricO11y/wiki) owns research and
result reports. Product/operator documentation, architecture, registered protocols
and executable evidence inputs remain versioned here under the
[ownership policy](documentation-policy.md#canonical-ownership). The
[migration manifest](wiki-migration.json) pins 151 migrated reports; subsequent
candidate reports use the same canonical wiki and evidence index.

## Assumptions and risks

- Durability assumes successful sync calls are honored by the filesystem/device;
  physical power loss has not been tested.
- Exact-byte identity uses SHA-256; collisions are assumed infeasible.
- Optional pruning structures require sound producer metadata; corrupt optional
  indexes fall back to exact reads. Silent readable Parquet corruption is not
  universally detected by per-query whole-file hashing.
- Layer checks enforce crate boundaries, not every module or runtime interaction.
- Earlier standalone-server benchmarks predate the dedicated Spindle; new service
  measurements must account for its work and diagnostic traffic.
- Query-tail decoding, retained metadata, worker concurrency and allocator
  retention can dominate process memory despite bounded sealer payload buffers.
- The historical standalone soak failed its RSS-growth gate; the passing R2
  trial includes the companion and is not a causal single-variable comparison.
- A pre-containment verification process caused a global workstation OOM on
  2026-10-04; its allocation mechanism remains unverified.
- Same-host producers/harnesses compete with the server and can distort latency.
  Remote delivery and synthetic RCA are not deployment qualification.

Broader qualification work follows the [capability ledger](QUALIFICATION.md#capability-ledger):
default-scale retention and exporter pressure, published-storage RCA and real
applications, and reconciliation of target-profile delivery evidence. These
remain separate from the completed readiness continuation. Application adapters
remain [proposed use cases](research/application-use-cases.md).

## Resource envelope

All builds, tests and experiments use the [resource launcher](CONTRIBUTING.md#resource-containment)
and the mounted data drive at `/run/media/kmosoti/data/FabricO11y`. The laboratory
ceiling is 20 GiB with no swap; the server experiment cap is 4 GB and total storage
ceiling 100 GB. Remote hosts use their own smaller verified budgets. Preserve
failure evidence and remove owned scratch. Bun CI/hooks remain disabled;
documentation checks run manually.
