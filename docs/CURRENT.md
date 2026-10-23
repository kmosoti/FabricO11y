# Current project state

The owner lifted the alpha/package acceptance pause on 2026-10-22 for the
[Blade/G15 release placement](experiments/formal/release-placement-protocol.md).
The persistent execution queue is `results/release-execution/queue.json` on Blade;
publication still requires final owner authorization. Historical pause and
engineering records below retain their original scope. No new acceptance
result is claimed by resumption.
The split-host adapter is under verification in `milestone/release-execution`.
It extends the existing Guest lifecycle with a G15 KVM edge and owned SSH
forwarding. Finite adapter checks do not complete native acceptance.
The first split bootstrap failed before Fabric offers: Fedora cloud-init
reported degraded completion with hostname warnings, and zram activated after
the fixture swapoff. Both original outer receipts confirm shutdown; the G15
peak was below its 2 GiB cap with zero OOM events. The
[retained receipt fixture](experiments/formal/fixtures/remote-bootstrap-stop.json)
keeps the cleanup/completion distinction. Provisioning and shutdown repairs
require a fresh smoke; no forwarding result is credited.
The next smoke reached exact package installation on both guests, then strict
TLS verification refused the fixture CA's missing key-usage extension. The
fixture now follows the documented CA recipe; its regression requires strict
verification and rejects the original defect. Fresh forwarding acceptance is
still pending. Separately, hosted dependency policy rejects the frozen
candidate's `rustls-webpki 0.103.15` under
[RUSTSEC-2026-0382](https://rustsec.org/advisories/RUSTSEC-2026-0382).
A [minimally changed successor](experiments/formal/release-successor-inputs-protocol.md)
package candidate was constructed; historical artifact and guest evidence retain
their original identities.
Successor product source `1eaf9ad` produced packaged snapshot `531e67c`.
Independent console, Debian and RPM reproducibility, constructor refusal
controls and artifact inspection completed with exit 0. Fresh installation,
browser, service/recovery and soak acceptance remain open. The standalone fuzz
lock now selects the same patched dependency; it does not alter those packages.
The full timing-capture smoke retained all events and completed all 19 gate
records, but file-log ACK latency, visibility upper bounds and edge source-debt
clearing failed. The [cadence/observation registration](experiments/formal/spindle-cadence-protocol.md)
keeps those failures. Shorter idle log/trace polling and per-response query
observations require fresh controls and another compatible package successor;
no timing or backlog threshold is relaxed.
The timing-capture successor is now constructed: product `aaca52d`, packaged
snapshot `c0cc668`. All eighteen constructor cells plus marker preparation exited
0 with confirmed stops; console/DEB/RPM A/B outputs reproduced and inspection
completed. Its tuple was selected for fresh testing. Hosted fast at `1fde8f0` retained an old integration
capacity expectation and a test-code Clippy failure; the harness expectations
were corrected without changing the frozen production code. Fresh fast checks
and every package acceptance gate remain open.
The cadence successor is also constructed: product `108945d`, packaged snapshot
`1cb0c185`, with all nineteen constructor commands exiting 0, confirmed stops,
matching console/DEB/RPM outputs and inspection. Its
[exact tuple](experiments/formal/fixtures/successor-native-inputs.json) is selected
for testing. Fresh split smoke 16 passed custody, query correctness, offered ACK,
all backlog, resource, configuration, outage and drain gates, but all five
timing gates failed visibility. Both host shutdowns were confirmed. Stopped
diagnosis found named observation intervals around 25–26 seconds, exceeding the
query window. The registered management clock-channel repair keeps these
packages and requires fresh controls and a full new cell; runtime acceptance
remains open.
Split smoke 18 completed all nineteen product gates with zero censored visibility
observations, but exited 2 on clock-channel cleanup after offline grading.
Both guests stopped; final clock command receipts were incomplete, so the cell
is not accepted. The registered repair closes clock capture before grading and
preserves original statuses through broken-pipe cleanup. Keep the constructed
`108945d` / `1cb0c185` candidate and repeat the complete affected cell.
The subsequent two-worker full split smoke completed producer, outage and drain,
then refused 790 dropped edge timing events. The
[bounded timing repair](experiments/formal/spindle-timing-capture-protocol.md)
raises only the opt-in event queue to 16,384 while retaining exact overflow
reporting. The old 256-event queue reproduced loss in 129 real Spool commits
at `af7942c` (test exit 101). Repair verification, a new package successor and
fresh acceptance are pending; the `1eaf9ad` artifacts retain their identities.
The successor smoke reached offered work and its outage, then failed during
resource extraction. The [monitor lifetime diagnostic](experiments/formal/cross-monitor-lifetime-protocol.md)
retains the original 330 samples and literal manager states. The monitor's
transient unit now retains its status until collection; failed or missing state
is refused. Fresh split acceptance is still pending. Script failures preserve
their original exit/timeout and output in private artifacts, with stdin hashed.
The next full smoke retained clean monitor completions, then failed on native
edge diagnostics that the timing adapter had incorrectly treated as offered
file logs. The [separate population registration](experiments/formal/cross-family-forwarding-protocol.md#prospective-timing-population-clarification-and-backlog-revision)
keeps those diagnostics under the same timing gates with their actual source
acceptance boundary. Unknown paths and changed offered bytes remain refused.
Fresh controls and a full smoke are required; the failed run is not promoted.
Diagnostic replay also found uncertain trace exports, grouped visibility
brackets and measured work crossing the intentional fault. The
[registered producer and sampling repair](experiments/formal/cross-family-forwarding-protocol.md#producer-and-observation-repair-after-the-retained-split-smoke)
retains all offered records and thresholds, adds bounded transport/observation
workers and separates the full outage workload with a fixed drain guard.
Fresh execution remains required; replay is diagnostic evidence only.
Producer and outage units now retain their original completion status too;
the collector refuses unloaded/default or failed states even when the status
command succeeds. Fresh controls and the full split smoke remain required.
The next smoke correctly refused an original producer exit 1 and preserved an
observer HTTP 400. Its stopped-image diagnostic recovered 294 queue-overflow
rejections and no missing admitted exports. The
[bounded successor](experiments/formal/cross-family-forwarding-protocol.md#producer-and-observation-repair-after-the-retained-split-smoke)
buffers the fixed full offer population inside the unchanged producer cap and
reuses existing named queries in bounded concurrent clock envelopes. Observer
errors persist immediately and stop further collection. Fresh execution is
still required; no failed smoke or diagnostic is acceptance.
The following smoke correctly refused observer HTTP 503 at the legacy
endpoint's existing two-query admission boundary. The
[prospective configuration](experiments/formal/cross-family-forwarding-protocol.md#producer-and-observation-repair-after-the-retained-split-smoke)
reserves one slot for each reader and explicitly selects the already available
Walk history plan. Product admission, defaults and query semantics are unchanged;
the eight-request overload case remains a rejection control. Fresh full
execution remains required.
The next split smoke retained a concurrent-append inspection refusal with both
hosts stopped. Drain sampling now records exact retryable inspection attempts
within the original deadline; contention cannot count as a drained snapshot.
Fresh split acceptance remains pending. A later full smoke refused four
dropped edge timing events after successful producer/outage completion. The
registered next attempt uses one asynchronous trace transport worker, retaining
all 1950 offers and the existing deadlines. That attempt failed its original
320-second producer deadline. The prospectively registered two-worker transport
retains both earlier failures and all gate expectations; fresh acceptance remains
unverified.

Alpha and package acceptance were owner-paused under [issue #59](https://github.com/kmosoti/FabricO11y/issues/59). Verification-engineering continuation is authorized and tracked in [PR #102](https://github.com/kmosoti/FabricO11y/pull/102); its provenance pins remain product source `b1e5874`, frozen package source `43150db`, `origin/main` baseline `f001175`, and engineering profile anchor `31cf809`. The Rust capture reader is at `681fcc5`, with native caller wiring at `bf1efa9`. These pins do not resume acceptance or claim a future check outcome.

Owner stop checkpoint (2026-10-18): the native producer now applies pause2,
waits for a stable acknowledged prefix, captures the independent stopped state
and final inspection, then requests resume3 while the node remains stopped.
The distinct reboot uses active3/pause4/resume5. Effect controls exercise the
actual guest lifecycle; they do not establish native journal authenticity.
The independent profile change is isolated in commit `d81c26b`; its default
static-profile path and source/ACK/query rules remain intact. The rejected
cached-view draft, setup failure, causal refusals and corrected controls remain
on the data drive. The 20 fast checks at `02f5b6c` predate this caller integration
and are not a full-profile result for the current branch.

The Rust capture reader now admits a pinned manifest and original JSON members,
then composes the existing version2 projection and cutoff verifier. Focused Rust
capture, projection and cutoff controls ran 16 tests with exit 0 at `681fcc5`; the
receipt is `results/service-query-query-gap-01/custody-capture-controls-01.json`.
The complete adapter invocation at `75a4bba` ran 147 tests with exit 0, including
the actual Rust helper after independent archive/custody admission. The immutable
Fedora stopped-bundle replay still rejects its missing exact-byte ACK.
The complete 20-check fast profile and all three documentation commands ran
with exit 0 at `c033613` in `custody-caller-fast-profile-02`. That revision fixes
the review-discovered `1e999` helper-report overflow; the preceding regression
run reproduced its erroneous acceptance. The helper and incremental source
bundle are retained in `results/coordination-custody-integration-01/artifacts-c033613/`;
the [execution record](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-primitive-composition-results-01)
keeps commands, failures, source identities and resource receipts.
The host Python transport and independent
Python archive/custody oracle remain pending independently validated migration.
Combined engineering verification in [PR #105](https://github.com/kmosoti/FabricO11y/pull/105)
found a coverage failure at `a2b39fc`: the host transport's global file-size limit
killed three instrumented Rust helpers with `SIGXFSZ` while they flushed LLVM
profiles. The correction bounds each protocol stream independently, retains
actual exit status, and stops the helper group before reaping its leader.
Independent side-file and quiet-descendant controls reject the former transport.
The instrumented adapter suite ran 155 tests with exit 0 on the corrected
working tree, and LLVM merged its profiles successfully; this focused result
does not replace final combined verification. Commands, patch identity and
failure evidence are retained under `results/rust-engineering-integration-01/`.
Native caller wiring is implemented. A real native capture requires a clean
harness revision present in `origin/main` and the existing pinned owner/helper
admission; no matching capture or acceptance has run. The
earlier DATA-only draft and its failures remain historical evidence. Do not resume
alpha acceptance or rebuild frozen product packages for these harness changes.
The persistent queue is `results/primitive-composition-01/queue.json`; the
earlier archived Rust helper and source bundle are under
`results/native-cutoff-adapter-01/artifacts-02f5b6c/`. That earlier private compiler
target and its documentation dependencies were removed after archiving.

Hosted browser defect receipts now have a Rust grader,
`cargo xtask check-browser-defect`, preserving both historical cleanup
dispositions and the registered API-cache/enrollment assertions. Its
[adoption protocol](experiments/formal/browser-supervisor-protocol.md#rust-defect-receipt-grader-adoption)
explicitly distinguishes stricter JSON/type admission from legacy parity.
The existing workspace test gate runs the CLI controls. At `1d9b633`, all
20 fast checks, same-source Rust receipt verification and the three
documentation commands exited 0. Six focused CLI test functions, ten retained
Python CI tests and the deliberately broken cleanup-proof guard were checked;
the original source was restored and passed. Earlier diagnostic/lint failures
remain in the [migration record](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-rust-ci-tooling-migration-results-01#browser-defect-grader-continuation).
No new hosted browser or product acceptance is claimed for this grader.

CI tool installation now has one pinned Rust entrypoint, `cargo xtask
install-tool`; the Python `tools/resource_group.py` remains the required outer
containment launcher. It enforces a fixed 1500-second deadline and 2 GiB DATA
headroom floor, uses inherited `CARGO_HOME` and an owned private target, and
retains logs/receipts while cleaning the target. At `6144147`, the combined
migration run exited 0: 48 library tests, 9 installer tests, 3 receipt CLI tests,
5 collector composition tests, 14 retained Python CI checks and Clippy.
Injected removal of the duplicate-ID guard and target-identity checks each
reached the intended failing assertion; restored source passed its controls.
Earlier compile/lint and fixture failures remain in the
[execution record](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-rust-ci-tooling-migration-results-01),
which also records the complete fast-profile result and its source revision.
No actual package install or performance result is claimed. See the
[installer boundary](architecture/engineering-evidence.md#pinned-ci-tool-install).

Engineering tooling now uses the Rust `xtask verify-receipts`
command, sharing bounded descriptor-owned artifact reads with evidence preflight.
The [architecture boundary](architecture/engineering-evidence.md#native-verification-receipts)
and [strict representation policy](formal/verification-receipt-contract.md)
describe its limits. The CI workflows have adopted this command; the original
Python comparator remains reproducible from immutable history. Local
complete-profile and independent refusal controls ran
at `7fc4723`; PR #80 merged as `06ccc0c` after complete hosted verification of
merge candidate `c129013`. Synthetic verifier fixtures do not credit product
acceptance. Alpha acceptance is owner-paused for verification engineering.
The [console provenance correction](experiments/formal/console-provenance-protocol.md)
separates frozen product source from newer harness source; its controls and
warm measurements are recorded in the
[finite results](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-console-source-provenance-results-01).
At `76d9f2b`, focused Rust/legacy adapter, packaging, UI and cleanup checks and
Clippy exited 0, including the unchanged snapshot-inventory and copy-race
regressions after their recorded RED results. Full required checks gate adoption.
Remaining exact-package trials retain their
original gates, including actual two-hour soak exposure.

The experimental Rust storage census now has finite classification and cleanup
race controls plus a measured warm metadata comparison. Its
[architecture and limits](architecture/engineering-evidence.md#native-storage-census)
separate these checks from caller adoption and native owner acceptance. A real
shared-target wrong-executable failure is retained; the corrective production
build and comparison used a fresh DATA target. The pinned owner census bridge
completed both original command-template namespace controls and six refusals;
these controls did not execute the actual owner walkthrough.

[Main/query acceptance #42](https://github.com/kmosoti/FabricO11y/issues/42)
closed after six main and four query trials, 800 independent query answers and
stopped positive/forged-binding/canonical closeout. The interrupted segment attempt
remains preserved. The first exact-package Chrome baseline then stopped on an
undefined readiness predicate after offline owner recovery. The
[registered regression](experiments/formal/browser-supervisor-protocol.md)
reproduced that NameError before the scoped helper correction merged in PR #81.
The retry stopped before browser launch on the frozen console/newer harness
Cargo.lock join. The native provenance correction has finite source controls;
a fresh actual browser retry remains required after the engineering pause. A
native entry/release barrier now precedes legacy browser child binding. Its
Linux composition fixture and real-browser transport-state diagnostic completed
at harness source `7508a989`; the native release-ordering controls and two
AbortSignal cancellation witnesses were observed, with actual HTTP responses
still subject to the original header checks. This finite harness evidence grants
no exact-package browser/PWA acceptance credit. The
[canonical finite results](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-console-source-provenance-results-01)
retain the raw trial records and limits.

Replacement product `e078ba23` is packaged reproducibly as minimized source
`28bcdf7` ([#40](https://github.com/kmosoti/FabricO11y/issues/40)). Fresh Debian03
and Fedora04 baseline/lifecycle cells completed with all four custody checkpoints
and confirmed cleanup; registered controls and stopped closeout completed
[#41](https://github.com/kmosoti/FabricO11y/issues/41). Earlier custody,
removal and startup failures remain preserved in their protocols and raw records.

Retained-query status now appends visible gap and unavailable-source counts while
preserving the existing coverage wording and exact evidence disclosure. The
deterministic incomplete-versus-gap input is preserved in the
[gap-summary fixture](experiments/formal/fixtures/query-gap-summary-regression.json).
This source change has not yet been built or checked; the frozen product remains
unchanged, so a new product build and browser rerun are required before it can
affect packaged behavior.

The native simulator-publication and browser-continuity prerequisites have finite
certificates. [S1-03](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-r3-s1-native-headroom-run-03)
completed 70 seconds of actual offered work and stopped headroom grading. This is
not the required 7,200-second FULL soak. Main/query, pressure/cross-family/outage,
exact-package browser/PWA and fresh-owner acceptance remain tracked by
[#33](https://github.com/kmosoti/FabricO11y/issues/33). Public Orca 51.1 environment
controls do not substitute for actual console announcements; key-format and
controlled-time checks still need both engines and exact package/ELF bindings.

The [Rust evidence preflight](architecture/engineering-evidence.md) is a read-only
consistency aid. At `6ac6932`, four library controls, twelve integration controls
and the registered native paired experiment exited 0 with confirmed stops.
The superseded Python aid is removed; independent product oracles are unchanged.
Its [contract](experiments/formal/engineering-evidence-protocol.md) and
[finite results](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-engineering-evidence-results-01)
preserve the distinction between structural consistency and acceptance.
The [two-runner verification pilot](experiments/formal/verification-shards-protocol.md)
completed both shards and the independent final validators at the same tested
source `faf148ec`. That historical run used the Python receipt validator. Its
finite wall time was 41.98% lower while combined runner time was
10.32% higher. The workers had different CPU, cache and setup conditions, so
this observation does not establish a universal speedup. The first aggregation
failure remains preserved; the corrected DATA-backed collector and unchanged
historical Python validator are recorded in the
[completed paired-shards results](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-verification-shards-results-01).
The original [failed aggregation](https://github.com/kmosoti/FabricO11y/actions/runs/39090541570)
is retained alongside the corrected result.
Current workflow adoption replaces the Python final-validator invocation with
the Rust `cargo xtask verify-receipts` command. The exact checked-in contract
fixture supplies expected decisions; the historical Python baseline and
differential remain historical evidence only. Preserve all ten registered
extended commands and their final current-source validation. Its current-head
hosted check remains required before merge.
No current deployment profile is qualified or release tag published.

## Implemented

FabricO11y collects, durably forwards, retains and queries Linux telemetry. It is
usable for bounded development trials; no current deployment profile is qualified
and no release tag has been produced.
The [R3 runtime integration](experiments/benchmarks/soak-runtime-protocol.md)
separates actual offered exposure, native UI continuity, a stopped raw checkpoint
and independent grading. S1-03 supplies finite measured headroom; FULL admission,
true two-hour exposure and stopped final grading remain required.
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
  mount guards have finite source and native evidence. S1-03 supplies actual finite headroom; FULL admission, exposure and stopped
  grading remain pending. The
  [patched helper/native publication](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-patched-simulator-native-publication-results-01)
  and [finite UI continuity](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-native-browser-continuity-mechanism-results-01)
  capability evidence does not qualify a soak;
  R2 retains its 5,460-second exposure and historical meaning. No new soak
  acceptance is recorded.

- The [fresh-owner walkthrough](experiments/formal/owner-walkthrough-protocol.md) has byte-preserving guide extraction, protected command transport and accumulated phase budgets. Its native caller is implemented; fresh constructor, browser and complete two-direction walkthrough acceptance remain pending exact candidate and merged-guide admission. The [31 source controls](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-owner-driver-source-results-01) cover bounded production snapshots, source/custody joins and cleanup failures; they do not establish native acceptance.
- The TLS dependency is pinned to `rustls 0.23.46` to address [RUSTSEC-2026-0349](https://rustsec.org/advisories/RUSTSEC-2026-0349.html). Provider and feature selections are unchanged. The previous `8fb6f413` candidate remains historical evidence. Replacement product `e078ba23` was frozen as minimized package source `28bcdf7`; independent console builds, Debian/RPM package reproducibility and package provenance completed [#40](https://github.com/kmosoti/FabricO11y/issues/40#issuecomment-6206477070). The [replacement package record](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-replacement-tls-package-results-01) retains exact commands, hashes, failures and cleanup. Fresh acceptance gates #41–46 remain incomplete. No dependency-policy exception applies.
- The [fixed-query caller prerequisites](experiments/formal/query-caller-run-01.md) retain an unpaced-page HTTP 429 and the obsolete fixture-only oracle corpus. The repaired disposable query smoke exited 0 with all eight gates; four full query successors remain pending. Six main cells retain their original `8484493` results under prospectively selected `clearing-v2`; older sampled-v1 edge diagnostics failed in five cells. No broader qualification follows.
- An attacker spread over many IPv6 /48s or IPv4 addresses can still fill the 1,024-entry peer table; a denial at a new second costs up to two access-state syncs (clock watermark and denial record), though at most one denial is stored per second ([#47](https://github.com/kmosoti/FabricO11y/issues/47)).
- Exact shipped units ran successfully in fresh Debian 13/Fedora 44 baseline and lifecycle cells ([#41](https://github.com/kmosoti/FabricO11y/issues/41)); independent adversarial probes of every added hardening directive remain outside that finite installation result.
- The [installation verifier controls](experiments/formal/installation-release-protocol.md#prospective-removal-checker-counterexample-controls) require both observed removal states to be inactive or failed with systemctl exit 3 and reject unreadable Fedora audit evidence. Revision-bound source-only execution evidence belongs to the [checker results wiki](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-installation-checker-prerequisite-results-01); guest acceptance remains separate.
- The [stopped-custody upgrade witness](experiments/formal/installation-release-protocol.md#prospective-stopped-custody-upgrade-witness) registers independent framing/Cursor and exact pending/unread-source conservation controls. Its raw snapshot/conservation adapter is implemented; verification and native integration remain pending. No new package lifecycle outcome is recorded.
- The [native custody reader](experiments/formal/installation-native-custody-protocol.md) admits full diagnostic roots and checks append/rotation, fixture paths, common protobuf containers and raw query conservation. Its [60 source controls](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-custody-native-reader-source-results-01/0364b1c) ran at `30ef218`. The [producer/caller](experiments/formal/installation-native-custody-caller-card.md) now implements stopped installed-root capture, complete raw/source archives, independent process/unit/cgroup observations, candidate ACK windows, immutable owner transfer admission and four lifecycle checkpoints. All 70 source controls ran at `5d6ea844` with exit 0 and no skips ([source record](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-native-custody-fourth-checkpoint-source-results-01)); fresh native acceptance remains incomplete after the retained first replacement capture failure. No new package lifecycle acceptance is claimed.
- The browser checks added for the role matrix (`live_browser_check`, bridge smoke, packaged console) are CI-only and have no recorded result.
- The Rust workspace test gate contains a fixed child invocation for the 11 existing Python qualification adapter modules. Those modules passed 89 tests directly at `24ff5b8` (DATA terminal `/run/media/kmosoti/data/FabricO11y/results/release-browser-entry-01/existing-python-terminal-01.json`). The Rust wrapper test then ran at `f5b63d23e55681e9e257ef888185cb83559bf16c`, exited 0, and invoked the same 89 tests; its terminal is `/run/media/kmosoti/data/FabricO11y/results/service-opening-candidate/release-browser-entry-final-python-wrapper-01.json` (SHA-256 `e9078f70ac5c27e1978a2d279680597ef0ed6cfb1ed344c813017a52e687f830`). The sparse logical-size control needs at least 6 GiB of DATA scratch, plus separate compiler allowance. See the [gate contract](architecture/engineering-evidence.md#qualification-adapter-test-gate).
- The existing [signed browser security supplement](experiments/formal/console-access-protocol.md#prospective-signed-browser-security-supplement) depends on capturing the completed registration request. A no-network Bun diagnostic of frozen harness `73cccefc` observed its old wrapper throw before dispatch for `fetch(Request)` without init; URL-plus-init dispatched. The candidate passed pinned-Bun Request-only capture, URL-plus-init, Request-plus-init override, and original-body-readability controls at source SHA `667337d4`. Chrome03 then showed that constructing the inspection request from the original `Request` consumed its body before forwarding. The clone-first correction passed Chrome06's six cases and 16 predicates at source SHA `3e588bbb5361722116a2f5fe07ca46d30efb0816bc0dd21be91a569201967af9`; the original Request was still unused at fetch dispatch. Chrome04 used stale operands and Chrome05 used a truncated extracted script; both were setup failures with no candidate behavior verdict. Chrome06 returned DELETE 200, reaped both groups with empty kernels, recorded Bun exit 0 and intentional ChromeDriver SIGTERM (-15), and removed scratch. These finite harness/API diagnostics do not establish product authentication or browser acceptance. The [captured browser evidence](architecture/engineering-evidence.md#browser-security-request-capture) records the terminal and cleanup details. The exact-source Rust control passed at `24ff5b8`. The hosted workflow preserves the ordinary positive Chrome command/artifact at `ci-live-browser`, adds a separate positive `--security-probes` command/artifact at `ci-live-browser-security`, and keeps the API-cache negative unchanged. Its earlier hosted positive result recorded 96 security checks and does not cover the restored ordinary command; earlier normal-mode coverage remains historical, and the result for this split is pending.
- The [serial session-state and cleanup controls](experiments/formal/console-session-clock-protocol.md#two-serial-state-fixtures-and-browser-cleanup-registration) retain [actual registration failures](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-browser-session-fixture-registration-red-results-01): four missing-checker assertions and five finalizer defects. The explicit AB/C checker has [source evidence](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-browser-session-source-results-01) with unchanged session semantics. The [delegated ownership registration](experiments/formal/console-session-clock-protocol.md#delegated-browser-launch-ownership-amendment) separates cleanup from native monotonic request binding and browser acceptance, which remain pending. Both fresh states share one 240-second deadline. The [owned cleanup source repair](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-browser-owned-cleanup-source-results-01) executes all 13 controls without skips at `336949e`; cgroup behavior is modeled. Native launch ownership and later-fork reaping are now integrated. At `0152363`, the [native cleanup controls](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-browser-native-cleanup-results-01) exited 0 for all 13 source controls and four real-kernel cases, with confirmed stop; this does not establish browser acceptance.
- The [R3 browser supervisor](experiments/formal/browser-supervisor-protocol.md) preserves the 1 GiB browser budget while isolating its child wait domain. Its seven native lifecycle/ancestor controls and exact-revision source/documentation follow-up ran at d6bb252, as recorded in the [finite results](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-browser-supervisor-results-01). Current R3 source carries the admitted native helper path and SHA-256 through `ProductionAccess`, `CandidateFixture`, `ConsoleBridge`, and `BrowserSupervisor` to the shared launch call. CandidateFixture snapshots the verified helper into its owned scratch tree, records the full caller/snapshot binding and passes the snapshot to ConsoleBridge. Closeout now requires that receipt record to match the constructor-bound binding and the snapshot path/hash in the immutable input map. The targeted source regression observed the missing-keyword `TypeError` at the real `supervise` call; native supervisor fixture controls remain separately tracked. The frozen `5220e27` source used its earlier matching launch signature and remains unchanged. Current-main release commands, non-soak `ProductionAccess`, bridge smoke, native cleanup fixture, Firefox trust fixture and owner walkthrough require and forward the explicit helper pair. The owner walkthrough checks that pair against its admission and records it; the native storage census binds the exact owner command's canonical DATA helper path and lowercase SHA-256 without reading the file. The shared caller defect RED is preserved, and owner-census RED observed the old 13-argument guard rejecting the new exact command. The final source controls, helper snapshot mutation controls and existing lifecycle suites are pending. Frozen #43/#46 snapshots remain unchanged. Native R3 caller/helper verification, hosted checks, native session request binding and browser acceptance remain pending. A prior existing R3 prerequisite unit failed because helper admission preempted its missing-certificate condition; the fixture now supplies a matching owned helper so that test reaches its intended certificate refusal, pending rerun.
- Firefox now requests its driver-returned BiDi URL and uses the bounded Rust `xtask browser-bidi-viewport` command, launched through the existing hash-pinned entry helper, to set 390 by 844 CSS pixels. The caller checks actual classic-JavaScript geometry and DPR; the protocol ACK itself is not geometry evidence. The focused adapter tests, scoped Clippy, 18 bridge controls and 74 UI controls passed. On the final packaged diagnostic, the positive run exited 0 with 344 checks and clean process groups; it observed 390 by 844 at CSS zoom 200%, detected the injected overflow, restored desktop geometry and cleaned its fixture. The registered cache-negative exited 1 after 78 checks for the intended worker-cache cause, and the unchanged grader exited 0. Earlier packaged attempts retain the missing-endpoint and default-confirm failures; a separate controlled probe isolated the Firefox-only `unhandledPromptBehavior: {"confirm":"ignore"}` requirement. These finite engineering diagnostics do not claim browser/package acceptance. The [native viewport and confirmation results](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-console-source-provenance-results-01#native-bidi-viewport-and-confirmation-controls) preserve the revision-bound controls and failures.
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

The R3 soak caller creates the simulator output directory exclusively with mode
0700 before fixture setup. Its native witness still independently requires a
canonical private owned parent; existing outputs are refused. This harness
precondition supplies no exposure or endurance acceptance.

All builds, tests and experiments use the [resource launcher](CONTRIBUTING.md#resource-containment)
and its storage root `FABRIC_STORAGE_ROOT`. The laboratory ceiling is the smaller of
20 GiB and 75% of RAM with no swap; the server experiment cap is 4 GB and the total
storage ceiling 100 GB. Remote hosts use their own smaller verified budgets. Preserve
failure evidence and remove owned scratch.

Trusted Firefox owner-walkthrough bridge support is prospective: isolated public
CA NSS trust, exact profile-only hostname mapping and supervised1GiB browser
budget are implemented in the harness. At `04349aa`, 23 finite source controls
and the trusted-origin, wrong-SAN and untrusted-CA native controls exited 0 with
confirmed stops; see the [revision-bound results](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-firefox-trust-results-01).
No owner-walkthrough/package acceptance follows from these mechanism controls. See the [registered owner protocol](experiments/formal/owner-walkthrough-protocol.md).

- Packaged legacy administration migration now has a prospective isolated browser supplement (`tools/ui/browser_migration.py`): exact CLI enrollment/query, stopped original state/prefix witnesses, actual resident owner/reader transition and separate normal-start format1 refusal. Source controls and both-engine package cells remain not run; see [registered protocol](experiments/formal/browser-migration-protocol.md).

- Native installation custody admission initially targeted the completed TLS replacement e078ba2 (canonical package source28bcdf7), preserving historical8fb evidence. The metadata-only same-code fixture constructor02 completed with exact payload/metadata comparisons. That candidate's first Debian attempt reached guest acceptance exit 0, then lifecycle grading refused an unexpected or failed Spool input before the custody snapshot (outer exit 2); the stopped failure remains under [#41](https://github.com/kmosoti/FabricO11y/issues/41). The next frozen candidate is query-gap `b1e5874` with canonical package source `43150db`; its fresh #41 attempt 38792 exited 2 before VM creation because the then-current admission still pinned e078/28bc. A source-pinned rebind now joins the new DEB/RPM receipts, candidate admission, canonical source and console receipt while preserving every guest/source/ancestry check. Runtime568 and runtime366 remain unchanged historical inputs; fresh A and M descendants preserve the Debian03/mutation and Fedora04/refusal runtime variants respectively, and satisfy the unchanged `origin/main` ancestry gate only after ordinary PR merge. Candidate source controls and fresh #41 positive, mutation and refusal cells remain pending; no query-gap native acceptance is claimed. See the [query-gap binding](experiments/formal/installation-native-custody-caller-card.md#query-gap-candidate-identity-prospective-revision-7).

- The guest-placement loader now resolves the installation custody helper from its own repository paths, including dynamic imports without an install PYTHONPATH. The isolated production-shaped counterexample failed with custody_native ModuleNotFoundError on0eed764 (direct QEMU CLI help succeeded). At `525373dc`, all three production CLI help commands passed with ambient `PYTHONPATH` removed, and the isolated loader test passed (exit 0); the [red and green record](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-guest-loader-import-counterexample-results-01) retains exact commands and stopped receipts. No guest/package acceptance follows from importability.

The replacement Debian native03 full lifecycle actually exited0 with independent
four-phase custody and removal admission. Fedora03 initial acceptance exited0,
but its first post-start drain query saw connection-refused and the old readiness
loop aborted before its40-second bound. Preserve the overall2/incomplete transfer
and original stopped guest. The separately registered initial-listener correction
retries only that exact first-query refusal under the same absolute deadline;
all raw attempts, partial-page/other-error refusal, late-success deadline and
final full-row/ACK gates remain required. Focused source/native checks are pending;
no complete replacement #41 qualification is claimed.

The cadence candidate108945d (package source1cb0c18) remains selected. Its first
fresh Debian native lifecycle retained an overall exit2 after baseline guest
acceptance0: stopped capture rejected the intentional `coverage-unknown` pause
sidecar. The raw timestamp falls within the captured pause/application interval;
this diagnostic does not convert that failed cell into acceptance. The
[registered pause-marker witness](experiments/formal/installation-native-custody-protocol.md#prospective-native-pause-marker-witness)
now joins private raw bytes, exact paused profile, boot and original commands;
markers without that proof and staged/unsafe inputs remain refused. At24cc517,
124 native source controls,74 cross-family controls and documentation checks
exited0 with confirmed outer shutdown. A literal positive case on the prior
8c1fef9 reader reproduced `unexpected or failed Spool input`. Additional original
command/conservation controls atb33559f and fresh exact-package native cells
remain subject to their own receipts; no complete installation or release
acceptance is claimed by this harness repair.
