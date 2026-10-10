# Milestone: release readiness

Status: **Option B selected; execution plan prepared, candidate not yet frozen.**
Owner decision: 2026-10-10. The objective is a bounded application release for
development and small deployments. Release maturity appears in the eventual
version tag and package metadata, following [ADR-0019](../decisions/ADR-0019-keep-release-maturity-in-tags.md).

This plan defines the work to prepare that release. It does not declare new
measurements passed, publish a release, or supersede existing qualification
protocols. Register the executable profile and its decision rules in a separate
policy commit before measurement. The broader [qualification ledger](../QUALIFICATION.md)
continues to own broader deployment claims; its current all-gates release rule
must be explicitly reconciled with the owner's selected narrower release scope.

## Release promise

One operator-controlled central server and outbound Spindles on remote machines,
with Debian-based and Fedora-based Linux supported in the finite matrix below.
The initial release architecture is x86_64, with systemd and unified cgroup v2;
other CPU architectures require their own artifacts and acceptance and are not
implied by a distribution-family label. One production server, its mandatory
dedicated Spindle, remote edge Spindles, the HTTP API and `fabricctl` are in scope.
Supported workflows are selected
file logs, host metrics, local OTLP/HTTP protobuf trace intake, durable forwarding,
journal and Segment queries, retention, restart/replay and self-observation.
Scan and the bounded writer remain defaults. Keep edge destinations unchanged.

The release validates the two workloads below on a declared host; simulated
identities are not equivalent to that many native processes or remote networks.
Report the hardware, filesystem, kernel and exact placement. Distribution-family
build compatibility does not establish runtime acceptance on every derivative.
Pin the Debian 13 and one Fedora release/image in R0; list those tested versions
in release notes. An unpinned or untested Fedora version is not an accepted cell.

| Central server | Remote edge Spindle | Required evidence |
| --- | --- | --- |
| Debian 13 x86_64 | Debian 13 x86_64 | Exact package installation, authenticated forwarding and remote recovery |
| Debian 13 x86_64 | Pinned Fedora x86_64 | Same, including Fedora SELinux enforcing |
| Pinned Fedora x86_64 | Debian 13 x86_64 | Same, including Fedora server/companion sandbox |
| Pinned Fedora x86_64 | Pinned Fedora x86_64 | Same, including SELinux enforcing at both ends |

Central and edge machines must be distinct OS hosts connected through an actual
network; loopback identities and same-host synthetic producers do not fill these
cells. Either supported family may host the server. Reuse the two installed
systems and exact artifacts where safe; this is four finite forwarding cells,
not a fleet-scale or every-distribution qualification campaign.

The owner-confirmed intended shipped default is **100 GB of retained telemetry**,
decimal `retention_bytes=100000000000`, configurable by the operator. Byte
retention counts sealed Segments and deletes whole Segments oldest first; the
existing configurable `retention_s=86400` age limit remains, with whichever
limit expires first controlling deletion. Journal, sealing workspace and Spindle
Spools remain separately bounded and require declared additional disk headroom.
This is not an aggregate 100 GB hard limit on every server/edge file. The current
parser and example still default to 20 GiB of Segments and a separate 20 GiB
journal ceiling. Register the contract/default change in R0, then update parser
defaults, shipped examples and boundary/restart/query regressions in R2. This
planning change does not implement or verify the requested default.
Never reclaim unacknowledged Spool data or unpublished journal coverage to meet
a storage target; pressure outcomes must preserve custody and visible gaps.

Features outside the release gate include a UI, a general OTLP receiver,
browser SDKs, Nomos integration, distributed storage, new indexing/encoding
algorithms and capacity claims above the measured profiles. Keep them in research.
Physical power-loss behavior remains outside the existing successful-sync and
process-crash assumptions. No fidelity, custody or completeness rule is relaxed.

## Candidate and evidence

Reconcile the existing dirty worktree into a reviewable `milestone/release-readiness`
candidate without discarding work or committing private fixtures. Separate policy,
implementation and documentation changes. Preserve meaningful failure fixtures.
Pin source revision, lockfile, toolchain, features, configuration and build command.
Build without experiment selectors or allocation instrumentation.

Produce a Debian-compatible `.deb` with the existing isolated build workflow and
a Fedora-compatible `.rpm` with a pinned, isolated build workflow still to be
implemented. Record both package and extracted binary hashes and their runtime
dependency floors. Service acceptance uses each family's exact package;
native harnesses must use its exact binaries, with test helpers built from the
same source/toolchain. A locally rebuilt equivalent binary is supporting evidence,
not a substitute for testing the deliverable. Keep each candidate's receipts separate.

The [completed continuation](../experiments/formal/readiness-continuation-results.md)
provides strong preparation evidence: corrected builder memory, fault/recovery
checks, installation acceptance, the companion soak and final verification.
Those results retain their original source and artifact identities. Rerun the
release gates on the candidate; do not repeat unrelated exploratory benchmarks.

## Workload registration

The following is the proposed finite matrix to turn into an executable protocol
in step R0. Any harness additions need independent expected values and negative
controls before candidate trials. Freeze commands and metric definitions then;
do not invent missing results from the current harness output.

| Dimension | Development | Small deployment |
| --- | --- | --- |
| Simulated edge identities | 10 | 100 |
| Logs | 2 × 512-byte records/s/identity; alternating repetitive and seeded high-entropy bodies | Same per-identity rate |
| Metrics | 32 points/15 s/identity | Same per-identity rate |
| Main trials | 3 seeds; 15 s warmup + 120 s measured per seed | Same |
| Seeds | `0xA11FA001`, `0xA11FA002`, `0xA11FA003` | Same |
| Production processes | Server and dedicated Spindle, plus one real edge Spindle | Same; remaining identities simulated |
| Trace source | Real edge's loopback trace intake; 10 traces/s, 3 spans/trace | Same aggregate trace rate |
| Placement | 2 CPU equivalents for server/companion; generators on separate CPUs | Same hardware and placement |

Both labels describe load levels, not independently qualified machine sizes.
Account for the real edge's host metrics and diagnostics and the companion's
traffic separately; include all accepted records in custody checks. Trace source
identities, counts, timestamps, parent relationships and query expectations must
be frozen independently of server output. Remote-machine forwarding is required;
no particular cloud vendor or droplet is required. Keep the six local main trials
for reproducible load comparisons and add the four remote acceptance cells rather
than multiplying the entire benchmark/soak matrix by every OS combination.

Each remote cell uses one native edge for 120 s after 15 s warmup, with the
registered per-identity logs/metrics and trace source above, then a bounded 60 s
network interruption and drain of at most 120 s. Check exact committed identities,
ACK custody, retries, and query answers before and after reconnection. Apply B3
steady latency gates and B5 edge/server RSS limits to each cell; do not mix WAN
samples with local benchmark samples. Register timing clock mapping, independent
source fixture, interruption method, all-count accounting and per-host cgroup
limits before execution. Three longer B6 outage trials remain separate.

Before the full matrix, use one disposable smoke to verify all paths. The measured
main trials use normal sealing; a separate fixed-fixture query matrix guarantees
coverage of both journal-only and published Segment reads, including restart.

## Release gates

Every required cell must pass individually. Do not average away a failed seed,
pool unlike query latencies, or exclude rejected requests from load accounting.

| ID | Required evidence and pass condition |
| --- | --- |
| B1 — Artifact | Frozen source/build/configuration manifests; `.deb` and `.rpm` plus binary checksums and dependency floors; fresh install of each exact package on its pinned OS. |
| B2 — Semantics | Independent delivery and query oracles report zero missing ACKed records, retry-created duplicates or mismatched answers. Logs, metric values/rates, trace identity/parents, pagination and completeness are checked across journal, publication and restart. |
| B3 — Steady service | In each profile/seed: durable ACK p99 ≤ 1 s, observation-to-query p99 ≤ 5 s, healthy configuration apply ≤ 30 s, and no growing backlog. Register the backlog-window rule and sample counts before runs; report offered/admitted/committed/rejected/retried counts separately. |
| B4 — Query | A fixed 1,000,000-record logs/metrics fixture, registered query kinds and unchanged Python oracle: p99 ≤ 2 s per query kind. Three seeds with Segments, one journal-only seed; both storage modes use identical declared contents. Add a separately graded trace fixture; do not infer trace latency from log queries. Freeze distributions, query order and repetitions in R0. |
| B5 — Resources | Enforced cgroup limits; no OOM or unaccounted children; server RSS ≤ 2 GiB and real edge RSS ≤ 64 MiB on the registered source workload. Record server and companion separately and together, CPU, swap, I/O, live storage and cleanup. |
| B6 — Recovery and pressure | Three real-edge outage trials: 30 minutes buffered and drain ≤ 10 minutes with exact delivery. All four remote cells pass their steady and reconnection checks. Required process-kill, scoped storage-failure, full-Spool, configured rollover/retention-boundary, TLS/auth rejection and 5× burst controls pass. Storage accounting includes every path and allowance fixed in R0; retained-window, old-page and restart answers match independent expectations. Burst latency is reported separately from steady gates. Each scenario has a bounded workload, exact expected outcome and clean termination. |
| B7 — Sustained operation | Fresh companion-aware [R2 soak](../experiments/benchmarks/soak-protocol-r2.md) on candidate binaries: all ten original gates plus custody/containment and freeze checks. The short smoke's documented seal-only failure is never a full-soak pass. |
| B8 — Installation | All 17 [installation checks](../experiments/formal/installation-acceptance-protocol.md) and the three existing mutation controls, including effective memory/task enforcement, pass on the exact `.deb` in the disposable Debian guest. The registered Fedora adapter preserves the same security/custody outcomes on the exact `.rpm`, adds SELinux enforcing checks, and tests both server/companion and standalone edge roles. Lifecycle additions below pass for both families. |
| B9 — Repository and security | Required fast checks, manual documentation checks, dependency policy and affected invariant/fault/model regressions pass. Triage security findings; no known exploitable authentication bypass, credential exposure or blocker remains in the supported configuration. |
| B10 — Operator workflow | Fresh central-and-remote-edge setup from the shipped guide reaches log, metric and trace queries without source edits on both families. Verify certificate trust/SANs, enrollment/revocation, firewall guidance, narrow log access, configurable storage, stop/restart, recovery, uninstall/data preservation and documented reset/compatibility policy. Ship known issues and bug/security reporting instructions. |

For B3, capture observation, source acceptance, Spool commit, send attempts, durable
ACK and query visibility as distinct events. Include failures, censoring and
retry waiting in the report. The policy must state the ACK population and must
not turn successful-attempt latency into creation-to-ACK latency. Missing timing
or custody evidence is incomplete, not a pass. Use monotonic clocks where possible;
record any clock mapping required for end-to-end timestamps.

Installation and lifecycle acceptance must additionally exercise repeated install,
account/group collision refusal, upgrade with existing configuration/credentials/
Spool/history, reboot, stop/start, removal preserving data and an explicitly
requested destructive reset. RPM removal has no assumed Debian purge equivalent;
define the operator reset procedure and verify its exact owned-path scope.
Freeze the previous-package fixture and compatibility policy in R0; if no prior
release exists, use a pinned predecessor fixture and label it as such. Record
whether services are enabled/restarted by each package action and ensure removal
leaves no running companion. Existing installation evidence explicitly excludes
reboot and upgrades; it does not fill these new cells.

On Fedora, keep SELinux enforcing throughout acceptance. Verify executable,
configuration, credential, state and authorized-source labels, HTTPS connectivity,
host metric visibility and denied-source gaps under the systemd sandbox. Record
AVC denials and their cause; do not disable SELinux, broaden privileges or generate
an unrestricted allow policy to manufacture a pass. Any necessary labels/policy
and operator source-access instructions become reviewed package deliverables.
Both families retain the static `fabricolly` user/group, vendor/admin path split,
capability-free units, effective cgroup bounds and managed companion contract.

## Resources and schedule

All builds, tests, validators and workload descendants use
`python3 -B tools/resource_group.py -- COMMAND`. Use only the mounted data drive
for scratch/caches; archive receipts and failures before owned cleanup.
The 20 GiB laboratory cap and 100 GB aggregate project-storage ceiling stay in
force. The latter is a laboratory budget separate from the intended configurable
100 GB shipped storage default. A trial may not fill a 100 GB fixture inside a
100 GB aggregate budget while retaining builds, guests and archives. Verify the
same rollover algorithm with smaller configured thresholds, check the default
configuration value separately, and register all accounted/transient headroom.
Do not claim default-scale capacity evidence from those smaller fixtures.
Admit each job only after measuring current usage and reserving its expected
build/guest/fixture/archive footprint. Serialize heavy jobs; reuse validated
tool caches and remove owned disposable trees between trials.

The 4,000,000,000-byte experiment ceiling does not raise installed limits.
The packaged server's `MemoryMax=3072M` covers server plus companion; the
standalone node and shared slice retain their [contract limits](../PRODUCT-CONTRACT.md#linux-installation-contract).
Run installed service gates under those limits. The native R2 trial retains its
own stricter-than-laboratory subgroup and original RSS gate. Resource measurements
must identify which boundary was measured; RSS is not total cgroup charge.

Main trial deadline: 1,800 s maximum; 5 GiB live-data budget and 50 MiB compact
retained evidence per trial. Outage trials retain the original 3,000 s runner
limit, with a planned 3,300 s outer deadline. R2 retains the 7,200 s runner and
7,500 s outer deadlines. Register these finite overrides before execution;
this planning task launches no qualification run. Minimum workload time is about
three hours for three outages, the full soak and six short main trials, before
query tests, installation, builds or defect repair. Stop on a budget breach and
retain an incomplete result rather than truncating the required workload.

## Execution queue

| Step | Work | Exit condition |
| --- | --- | --- |
| R0 | Register the bounded release profile, pinned Debian/Fedora x86_64 images, four remote cells and exact matrix; reconcile the ledger's broad release rule and product contract's Debian-only/20 GiB defaults in a policy-only commit. Register decimal 100 GB Segment retention, preserved configurable 24 h age limit, separate journal/workspace/Spool bounds and pressure/headroom semantics. Specify Fedora/lifecycle adapters and negative controls, outage/pressure companion adapters, backlog calculation, trace oracle inputs, query sample counts and deadlines. | No ambiguous gate or undocumented exception; old protocols/results preserved. |
| R1 | Consolidate source, reconcile canonical repository operator/current/roadmap documentation and prepare build provenance. Migrate historical narrative/reports to the initialized wiki with a checked source-to-destination index; preserve versioned protocols, fixtures and code contracts in the repository. | Reviewable candidate revision and pinned build inputs; release-relevant canonical guidance and migrated links verified; only registered packaging/storage scope admitted. Unrelated research migration does not extend the release gate. |
| R2 | Implement the registered storage default in the parser and shipped examples with byte/age boundary, restart and query regressions; implement Fedora packaging/lifecycle support; build both packages; run fast/dependency/affected regression checks and harness negative controls; admit one smoke. | Exact packages identified; checker defects and packaging/storage failures resolved before expensive trials. |
| R3 | Run six main profile trials and the fixed query matrix, the four remote cells, then recovery/rollover/pressure and the three outage trials. | Every B2–B6 cell passes with complete per-host resource/timing/custody evidence. |
| R4 | Run full R2 soak and both families' installed-package/lifecycle acceptance with mutation controls and Fedora SELinux enforcing. | B7–B8 pass on the candidate; all child processes stopped and temporary fixtures cleaned. |
| R5 | Exercise the operator walkthrough from canonical repository documentation; finalize compatibility/reset notes, known issues, checksums and evidence index, verifying release-relevant wiki report links and preserved evidence provenance. | B1–B10 ledger complete; zero open release blockers; artifact hashes still match; operator instructions remain self-contained in the repository. |
| R6 | Publish the selected unused version tag and exact tested `.deb`/`.rpm` under explicit release authorization. | Downloaded artifact checksums verified; release notes and report links available. |

The operator walkthrough should use fresh state and the guide alone; fixes to
instructions are exercised again. Public downloads need an appropriate license,
dependency notices and a security-reporting route. A signed checksum is useful
only with a documented way for consumers to trust its signing key.

## Blockers and change control

The scope additions have implementation gaps, not fresh acceptance evidence:
[build-deb.sh](../../packaging/build-deb.sh) and the
[Debian lifecycle scripts](../../packaging/debian/postrm) exist, while no RPM
builder or Fedora acceptance adapter is present. The
[installation protocol](../experiments/formal/installation-acceptance-protocol.md)
and [guest definition](../../tools/qualification/install/Dockerfile) are Debian
specific and exclude upgrade/reboot coverage. The
[server parser](../../crates/fabric-server/src/config.rs) and
[shipped example](../../packaging/etc/server.conf.example) default to 20 GiB
independent journal and Segment ceilings; the
[sealer](../../crates/fabric-server/src/sealer.rs) applies byte retention to
Segments. The intended 100 GB Segment default and Fedora SELinux behavior remain
unimplemented/unverified; an aggregate disk cap is outside the confirmed promise.
Resolve these gaps through R0–R4 rather than changing the historical results.

Block on a failed required cell, unverified artifact identity, inability to finish
an advertised workflow, known silent corruption/ACKed-data loss, auth bypass or
credential disclosure, failed resource enforcement, or misleading complete query
answers. Missing required evidence also blocks the candidate. Keep other issues
with severity, affected scope, workaround and follow-up owner in the release notes.

A fix creates a new candidate. Record what changed and which receipts remain
applicable by exact inputs. Rerun all directly/transitively affected gates;
storage, delivery, lifecycle or packaging changes require renewed integrated
service/recovery and package acceptance as applicable. Never combine incompatible
candidate results into a green ledger. Pure documentation edits need documentation
and affected walkthrough checks, not a new unrelated performance campaign.

The gate is finite: once B1–B10 pass, archive the evidence, clean owned scratch
and prepare publication. New research or optimization ideas do not extend this
release unless they resolve a defined blocker. Broader profile qualification
continues separately without being relabeled as passed.
