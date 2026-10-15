# Current project state

The separate signed browser security cell is implemented for virtual fixture
keys, completed-ceremony replay, cookie replacement and real reader expiry.
Its [registered controls](experiments/formal/console-access-protocol.md#prospective-signed-browser-security-supplement)
and the separate [UI response diagnostics](experiments/formal/console-access-protocol.md#prospective-ui-response-handling-supplement)
are implemented. At `b211398`, 32 affected source tests and all three documentation
checks exited 0 through separately contained runs, with stops confirmed. The first
combined run remains interrupted: its strict storage census raced launcher scratch
cleanup even though the inner checks exited 0. Native signed-key and diagnostic UI
cells have not run; exact-package browser acceptance remains open.

The standalone console bridge smoke now selects the owned browser supervisor,
with a fresh held1GiB browser budget, finite deadline and bounded close. Failed
scratch is retained. Focused ownership controls and the actual replacement
candidate smoke remain required; this caller change supplies no acceptance.

## Implemented

FabricO11y collects, durably forwards, retains and queries Linux telemetry. It is
usable for bounded development trials; no current deployment profile is qualified
and no release tag has been produced.
The prospective [R3 runtime integration](experiments/benchmarks/soak-runtime-protocol.md)
adds a separate two-hour measurement path, native UI event capture and stopped
checkpoint/separate grading. Its live native trace and supervised browser
bindings, continuous-refresh controls and real S1 headroom remain prerequisites;
implementation alone supplies no new soak acceptance evidence.
The separately registered [lifecycle guardian amendment](experiments/benchmarks/soak-guardian-protocol.md)
separates empty workload accounting, aggregate accounting before guardian exit,
and independent whole-service stop. Finite guardian controls establish that
topology on the local host; live R3 integration and actual phase headroom remain
separate prerequisites, with no candidate soak acceptance implied.
The [product contract](PRODUCT-CONTRACT.md) defines the promises and the
[verification matrix](formal/verification-matrix.md) maps each invariant to checks
and remaining limits.

| Component | Current behavior |
| --- | --- |
| Spindle (`fabric-node`) | Host metrics, selected log files and loopback OTLP/HTTP trace intake; durable FAB1 Spool, exact retry, visible collection gaps, output-rate cap and last-valid remote configuration. |
| Delivery | Verified TLS (the configured CA and the dialed name only), per-Spindle credentials, ordered Batches, deduplication and ACK after grouped two-sync journal commit. A quiet sender closes its group after 2 ms. One rule interprets every server answer; an ACK beyond the Spool's durable sequence is refused. |
| Storage | Bounded external-merge sealing to Zstd Parquet, manifest publication before checkpoint/reclaim, retention by age and bytes. The normal writer uses aligned sorted groups and disk-backed completed pages; estimated input targets admit a single oversized row. No whole-server memory bound is established. A journal group the stream table cannot absorb quarantines the commit thread (in memory; a restart replays). |
| Query | Logs, metric points/rates and spans across journal and Segments, snapshot pagination, freshness and completeness. Legacy queries default to Scan. Console queries (owner and reader, all telemetry, at most 1,000 rows and 24 hours) use Walk with storage-owned source maps; the now-redundant scoped-filtering code remains. Unpaginated rates stay outside the console API. |
| Console and access | Leptos/WASM PWA with a local owner passkey, expiring read-only reader tokens for scripts and AI clients, and Spindle tokens: three fixed roles ([ADR-0029](decisions/ADR-0029-use-three-fixed-roles-for-the-first-release.md)) at one route guard. Normal `serve` requires an HTTPS origin and RP ID, needs no administrator token and refuses `/v1/admin/*`; `serve-legacy` is migration only. Enrollment requires resident credentials and user verification; finite Chrome checks include independent credential inventory. The exact-package and physical-device release matrix remains open. |
| Hardening | Console peer admission (including an IPv6 /48 cap), 503 on a poisoned lock, absolute OTLP deadlines, no-follow descriptor-relative log opening, one credential-file policy, audit denials capped apart from other events and eight added unit sandbox directives; see [security hardening](architecture/security-hardening.md). |
| Self-observation | Every production server CLI launches and supervises a dedicated local Spindle. Both emit bounded diagnostics through ordinary Spool/TLS/ACK delivery. Edge Spindles retain their configured destination. |
| Packaging | Debian and RPM artifacts include the console, Apache-2.0 license and dependency notices, with static systemd services and a shared slice. Exact frozen `a5dbda7a` packages passed fresh Debian/Fedora baseline and upgrade/reboot/removal cells, with six named mutation controls detected; broader release acceptance remains open. These cells establish normal service operation under the added unit directives; their individual adversarial enforcement remains unverified. |
| Architecture | Pure `no_std` semantic core, effect ports, application transitions and adapters; executable layer and purity gates. `ports` and `app` may link no external crate, and a requirement satisfied by `[patch]` counts as the member edge. |
| Repository | Historical tooling, experiment data and the FOL2 demonstration live in the [research archive](https://github.com/kmosoti/FabricO11y-research); dead Rust and experiment selectors are removed. The launcher's storage root is `FABRIC_STORAGE_ROOT` with host-scaled memory caps. Workflow actions are pinned by SHA, the fuzz nightly by date, and `tools/ci/check-vendor.py` compares vendored crates with crates.io. |

See the [system view](architecture/system.md), [operations guide](operations.md)
and [architecture index](architecture/README.md) for behavior and configuration.

## Release scope and open work

The release epic ([#33](https://github.com/kmosoti/FabricO11y/issues/33)) tracks the
remaining work. Owner decisions ([#34](https://github.com/kmosoti/FabricO11y/issues/34)):
tag `v0.1.0-alpha.1`; Debian 13 and Fedora 44 (pinned at the freeze on 2026-10-12, before Fedora 45
shipped); physical-device and mobile passkey checks are deferred as a known
issue; reader queries are not individually audited. The
[release plan](milestones/release-readiness.md) covers Debian/Fedora central and edge
deployment, 100 GB retained telemetry with separate working space, local passkeys,
owner and reader access, and the PWA; Apache-2.0 licensing, dependency notices and a
[private security-reporting route](../SECURITY.md) are included. No new
qualification is claimed here: repository CI receipts are integration evidence, not
package, device or deployment acceptance.

Known limits and open checks:

- The prospective [R3 soak](experiments/benchmarks/soak-protocol-r3.md) requires
  7,200 seconds of actual offered work, nine registered cohorts, measured
  non-exposure headroom and a stopped raw checkpoint before separate grading.
  The observation checks, guarded rename, held workload accounting and preservation
  mount guards have finite source and native evidence. Live S1/FULL integration,
  patched simulator helper authority, boundary timing and full admission remain pending;
  R2 retains its 5,460-second exposure and historical meaning. No new soak
  acceptance is recorded.

- The [fresh-owner walkthrough](experiments/formal/owner-walkthrough-protocol.md) has byte-preserving guide extraction, protected command transport and accumulated phase budgets. Its native caller is implemented; fresh constructor, browser and complete two-direction walkthrough acceptance remain pending exact candidate and merged-guide admission. The [31 source controls](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-owner-driver-source-results-01) cover bounded production snapshots, source/custody joins and cleanup failures; they do not establish native acceptance.
- The [fixed-query caller prerequisites](experiments/formal/query-caller-run-01.md) retain an unpaced-page HTTP 429 and the obsolete fixture-only oracle corpus. The repaired disposable query smoke exited 0 with all eight gates; four full query successors remain pending. Six main cells retain their original `8484493` results under prospectively selected `clearing-v2`; older sampled-v1 edge diagnostics failed in five cells. No broader qualification follows.
- An attacker spread over many IPv6 /48s or IPv4 addresses can still fill the 1,024-entry peer table; a denial at a new second costs up to two access-state syncs (clock watermark and denial record), though at most one denial is stored per second ([#47](https://github.com/kmosoti/FabricO11y/issues/47)).
- Exact shipped units ran successfully in fresh Debian 13/Fedora 44 baseline and lifecycle cells ([#41](https://github.com/kmosoti/FabricO11y/issues/41)); independent adversarial probes of every added hardening directive remain outside that finite installation result.
- The [installation verifier controls](experiments/formal/installation-release-protocol.md#prospective-removal-checker-counterexample-controls) require both observed removal states to be inactive or failed with systemctl exit 3 and reject unreadable Fedora audit evidence. Revision-bound source-only execution evidence belongs to the [checker results wiki](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-installation-checker-prerequisite-results-01); guest acceptance remains separate.
- The [stopped-custody upgrade witness](experiments/formal/installation-release-protocol.md#prospective-stopped-custody-upgrade-witness) registers independent framing/Cursor and exact pending/unread-source conservation controls. Its raw snapshot/conservation adapter is implemented; verification and native integration remain pending. No new package lifecycle outcome is recorded.
- The browser checks added for the role matrix (`live_browser_check`, bridge smoke, packaged console) are CI-only and have no recorded result.
- The [serial session-state and cleanup controls](experiments/formal/console-session-clock-protocol.md#two-serial-state-fixtures-and-browser-cleanup-registration) retain [actual registration failures](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-browser-session-fixture-registration-red-results-01): four missing-checker assertions and five finalizer defects. The explicit AB/C checker has [source evidence](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-browser-session-source-results-01) with unchanged session semantics. The [delegated ownership registration](experiments/formal/console-session-clock-protocol.md#delegated-browser-launch-ownership-amendment) separates cleanup from native monotonic request binding and browser acceptance, which remain pending. Both fresh states share one 240-second deadline. The [owned cleanup source repair](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-browser-owned-cleanup-source-results-01) executes all 13 controls without skips at `336949e`; cgroup behavior is modeled. Native launch ownership and later-fork reaping are now integrated. At `0152363`, the [native cleanup controls](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-browser-native-cleanup-results-01) exited 0 for all 13 source controls and four real-kernel cases, with confirmed stop; this does not establish browser acceptance.
- The [R3 browser supervisor](experiments/formal/browser-supervisor-protocol.md) preserves the 1 GiB browser budget while isolating its child wait domain. Its seven native lifecycle/ancestor controls and exact-revision source/documentation follow-up ran at d6bb252, as recorded in the [finite results](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-browser-supervisor-results-01). Supervisor failure requires whole-campaign stop/reaping; the native session request binding and browser acceptance remain pending.
- In cloud containers the `xtask` tests `check_logs` (needs the launcher) and `evidence` (needs full git history) fail, and `tests/control.rs` fails under umask 022; these are environmental.

The [simulator accounting source controls](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-simulator-accounting-source-results-01) execute 33 methods without skips; the sealed helper produced the registered missing-begin native red after four actual ACKs. A separate opt-in helper compiled successfully. Its [eight finite native witness diagnostics](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-simulator-native-controls-results-01/dc17be3) exited 0 at `6bd5250`, with confirmed stops; full source/build admission checked all 463 frozen compilation inputs. The [initial-thread syscall observer](experiments/benchmarks/simulator-native-trace-protocol.md), ten literal parser controls and eight native publication/fault controls are registered but unrun. Seven Rust tests, atomic publication certification and full exposure remain pending. Frozen package/helper bytes are unchanged.

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
exact integer counter rates, reserved diagnostic credentials, audit eviction by
denial floods, poisoned locks and one IPv6 network filling the peer table. Their
minimized failures and checks are linked from the matrix and experiment index.
Verification fixtures check finite pagination progress before complete drain, and
interrupted registry checks retain partial output without a completion receipt
([verifier protocol](experiments/formal/verifier-boundedness-protocol.md)).

## Candidate evidence to date

These results belong to the candidates they name; none qualifies the release.

- **Console reproducibility.** Separate console builds of one commit differed in four files because ImageMagick stamped time metadata into the generated icons ([#55](https://github.com/kmosoti/FabricO11y/issues/55)). Icons are now generated with `-strip`, and console and package builds refuse a PNG with time or text metadata (`CX-CONSOLE-ICON-TIMESTAMP`). The owner chose to fix this before release, so the `a5dbda7` packages are superseded: a new candidate is frozen and rebuilt from two separate console builds, and its gates rerun. Results on `a5dbda7` packages remain recorded for that candidate only.
- **Owner walkthrough tooling.** The [prospective protocol](experiments/formal/owner-walkthrough-protocol.md) binds exact candidate bytes, strictly older native fixture metadata, complete payload/header/dependency identities, bounded owned command groups and explicit interrupted closeout receipts. Revision-bound source checks and retained counterexamples belong in the [constructor source record](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-owner-fixture-source-results-01). Native fixture transformation, final merged guide pins and the fresh owner walkthrough remain pending.
- **Service-opening repair.** Frozen `ca84d982` failed Debian lifecycle `rel-deb-life-01`: required `RestrictSUIDSGID=yes` denied `openat2`, and the dedicated Spindle/server exited. The [failure record](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-installation-release-run-02) retains exact packages and guest controls. A descriptor-relative `openat` walk replaces that syscall without relaxing hardening or link refusal. Its native child-filter regression failed before the repair (exit 101) and passed after; eleven Linux security tests passed, including pinned-parent replacement and exact error propagation. The [amendment](experiments/formal/service-log-opening-protocol.md) registers the checks. PR #53 merged with six commits preserved. Frozen `a5dbda7a4fd6465c94f6505b3214cbcbc2130c82` was rebuilt reproducibly with matching console assets; matching main Rust/security and identical-tree PR extended evidence passed. [Exact installation run 03](experiments/formal/installation-release-run-03.md) passed four fresh positive cells and detected all six package controls on Debian 13/Fedora 44. Old candidate failures remain failed; this does not qualify deployment or authenticated UI behavior.

- **Readiness continuation.** The [continuation](experiments/formal/readiness-continuation-results.md) completed the combined builder campaign, recovery/shutdown checks, dependency policy and finite [installation acceptance](experiments/formal/installation-acceptance-run-02.md). The [full companion-compatible R2 soak](experiments/benchmarks/soak-run-02.md) passed all ten gates. The [normal writer](experiments/formal/bounded-writer-default-run-01.md) passed 27 unflagged bounded tests, 13 kill cuts plus no-hit, and loaded-binary equivalence; all 17 final fast checks and three manual documentation checks passed on that revision.
- **Packages and console.** Debian build 04 and RPM build 03 contain console build 09, with all 637 common payload files matching. The console fixed a real 200% zoom pagination overflow; 150 staged Chrome assertions passed, including rejection of an injected overflow. A later change requests resident credentials for all four enrollment paths (two focused regressions passed on `829babe`; independent inventory checks and a nonresident-enrollment control are implemented). New exact packages and their browser acceptance are pending. Virtual devices do not establish physical passkey or mobile compatibility; Firefox's virtual-passkey diagnostic completed 127 assertions after two fixture repairs, and standalone resident-key acceptance is pending.
- **Query at scale.** `release-query-segment-seed1-successor-02` passed all eight gates on Debian build 04/RPM build 03: 200/200 independent-oracle answers, exact recovery and companion custody, query p99 at most 504.340 ms before restart and 447.509 ms after. It is not attributed to the resident-enrollment successor; the earlier 10.27-second five-page failure remains in the [cache report](https://github.com/kmosoti/FabricO11y/wiki/Experiment-benchmarks-scoped-evidence-cache-run-01).
- **Failed and open.** Full soak `production-soak-full-03` failed after 1,587.618 seconds: its storage census could not traverse directories owned by a concurrent package build, and the harness stopped rather than ignore uncounted disk use. No Fabric crash was observed. The failed and interrupted runs keep their results in the [campaign record](https://github.com/kmosoti/FabricO11y/wiki/Experiment-benchmarks-production-access-campaign-run-01). The replacement exact-package installation/lifecycle matrix is complete. The first #42 disposable smoke failed before measurement on enrollment HTTP 429: each bridge command made two protected requests but was paced at 0.15 seconds. [Command admission run 01](experiments/formal/console-command-admission-run-01.md) records the independent red schedule regression, 50 passing harness tests and a passing repaired native smoke on unchanged packages. The separate harness change retains all session checks/refusals and decision rules; the full main/query matrix, pressure/outages, cross-host cells, fresh full soak and packaged browser/PWA acceptance remain queued. Historical Fedora evidence belongs to its original `c06688c` candidate, including the [Btrfs identity repair](decisions/ADR-0028-preserve-btrfs-log-identity-across-reboots.md).
- **Hosted and local gates.** The hosted `eb5b096` candidate passed fast and focused security workflows and all ten extended commands, but final receipt completeness failed after fuzzing rewrote its stale dependency lock; the lock now matches and fuzz trials require unchanged lock bytes. The local `851d50b` delivery gate exited 0 (eight trials, unchanged oracle, strict loss control rejected) through the explicit legacy adapter, and a finite Chrome trial completed 144 assertions on debug binaries; neither qualifies packages or devices. The [integration review](https://github.com/kmosoti/FabricO11y/wiki/Release-integration-review) holds commands, candidate identities and cleanup observations. Bun CI/hooks remain disabled; documentation checks run manually.

The [research wiki](https://github.com/kmosoti/FabricO11y/wiki) owns research and
result reports; its [release progress page](https://github.com/kmosoti/FabricO11y/wiki/Release-readiness-progress)
links the acceptance queue. Product/operator documentation, architecture, registered
protocols and executable evidence inputs stay here under the
[ownership policy](documentation-policy.md#canonical-ownership); the
[migration manifest](wiki-migration.json) pins 151 migrated reports.

## Assumptions and risks

The fresh-owner walkthrough has a byte-preserving
[guide extractor](../tools/qualification/owner_guide.py) for its two admitted
operator guides. It archives exact commands and source spans; it does not execute
the guide or establish walkthrough acceptance. Conditional choices, linked
callpoints, prose actions and native observations remain runner responsibilities
under the [registered protocol](experiments/formal/owner-walkthrough-protocol.md).

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
applications, and reconciliation of target-profile delivery evidence. Application
adapters remain [proposed use cases](research/application-use-cases.md).

## Resource envelope

All builds, tests and experiments use the [resource launcher](CONTRIBUTING.md#resource-containment)
and its storage root `FABRIC_STORAGE_ROOT`. The laboratory ceiling is the smaller of
20 GiB and 75% of RAM with no swap; the server experiment cap is 4 GB and the total
storage ceiling 100 GB. Remote hosts use their own smaller verified budgets. Preserve
failure evidence and remove owned scratch.
