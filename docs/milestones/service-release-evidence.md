# Service release evidence

This ledger concerns `v0.1.0-alpha.1`, product source
`84a585d35cced26de170c4b34c0a05aab79fae3b` and packaged snapshot
`34591b7b1390852ad9984339a9fa102c8259d743`. It records finite tested environments,
not deployment qualification. Publication still requires the owner's final decision.
The [release policy](release-readiness.md#release-gates) defines the gates;
[this ledger](#candidate-gates) records their candidate-specific state.

## Candidate identity

| Artifact | Version | SHA256 |
| --- | --- | --- |
| `fabrico11y_0.1.0~alpha.1_amd64.deb` | `0.1.0~alpha.1`, amd64 | `5c40a5171ed2dffb6e6d6d7f387210d34997a651c15bbfb661b037f1b1ec84b3` |
| `fabrico11y-0.1.0-0.alpha.1.x86_64.rpm` | `0.1.0-0.alpha.1`, x86_64 | `a042e92eafd4d6126858e75d46e8a67fd0da7a843b35ceb4544058b8122897c0` |

The product source, packaged snapshot, later harness revisions and guide revision
are distinct identities. Exhaustive source comparison found all 501 packaged
files byte-identical to the frozen source. The current harness matches 498 of
those files; its three differences are the resource launcher and native custody
projection code/tests, with no production src/crates/Cargo/packaging difference. A later harness commit is not the package build source.
The native browser entry helper has SHA256
`2063e2d3318d2610d1654517e6f0f85d6351cdb01c6298fcb854131b625ba9b2`.
That helper was built from harness `25474019099251eb826d193c06102d2ef9fbf2fd`
with Rust `1.99.0 (b940084d7 2026-09-28)` and feature
`verification-sha-openssl`, recorded by `prerequisite-terminal-01`.
Package construction receipts are `alert-presentation-01-deb-a` and
`alert-presentation-01-rpm-a`; immutable source and payload manifests accompany them.

## Candidate gates

| Gate | State | Executed evidence and limits |
| --- | --- | --- |
| B1 Artifact | Complete | Exact candidate build/admission, binary/configuration/license/dependency manifests, package hashes and fresh native installations. Admission SHA256 `c05ce790d2466daab32c30edf2bf12f5b4da280c29951eff2fb893728e0ddf3c`. |
| B2 Semantics | Complete | Six main cells, four million-record query cells with separate trace fixture, native lifecycle and both complete owner directions. Original exits and independent delivery/query/custody grades are 0; no missing ACKed record, retry duplicate or mismatched graded answer. |
| B3 Steady service | Complete | `service-campaign-alert-01/all-six-terminal-01`: 10 and 100 identities, seeds `0xA11FA001`–`0xA11FA003`, each 15-second warmup plus 120 measured seconds. All 13 registered gates true, including ACK p99 ≤ 1 s, visibility p99 ≤ 5 s and healthy configuration application ≤ 30 s. |
| B4 Query | Complete | `query-campaign-alert-01/all-four-terminal-02`: three Segment seeds and one journal-only seed, each 500,000 logs plus 500,000 metric records; separate 1,000-trace/3,000-span fixture. Original and independent grades 0, all registered query kinds p99 ≤ 2 s, before and after restart. |
| B5 Resources | Complete for the registered finite placements | Main/native/owner records verify cgroup controls, zero project swap, process ownership and cleanup. Registered main server RSS ≤ 2 GiB and real edge RSS ≤ 64 MiB. Pressure still needs its own resource verdicts; no whole-server bound for arbitrary workloads is inferred. |
| B6 Recovery and pressure | Pending pressure; explicit partial deferral | Exact-product storage/full-Spool/process-kill/rollover/retention and TLS/auth controls are retained. Fresh pressure smoke has all 13 gates true and independent grade 0. Three full pressure cells remain required. The first full pressure attempt failed malformed-probe status checks and stays failed. Four full cross-family campaigns and three 30-minute outages are deferred. |
| B7 Sustained operation | Deferred, not passed | Owner-approved alpha boundary defers the full 7,200-second R3 soak. Historical or short trials supply no candidate soak credit. |
| B8 Installation | Complete | `native-compatible-alert-03/actual-ten-terminal-01` and original closeout step 03: ten fresh cells, both native positive lifecycles, two producer refusals and six mutation controls. Original commands/grades 0 and cleanup confirmed. Fedora SELinux enforcing. |
| B9 Repository and security | Complete for the candidate-compatible code | Hosted fast run `40141551209` (20 checks) and extended run `40141551202` (10 checks) completed, with original command/shutdown and independent native grading exits 0 at CI merge `482b0d6`, tree-equal to harness `ff0eb90`. The 32 focused security executions retain source `07c35b6` with all named Rust/resource/security inputs unchanged at `ff0eb90`. Current affected source and documentation controls exited 0. Final ledger edits retain their own documentation checks. |
| B10 Operator workflow | Complete | `alert-owner-debian-fedora-16` and `alert-owner-fedora-debian-04`: both full fresh guide directions, original commands and independent four-stage query/custody grades 0, distinct boot identities and both-host shutdown confirmed. |
| B11 Owner identity | Complete for the registered finite controls | 70-row packaged Chrome/Firefox matrix, both migration supplements, actual resident/UV owner/second key and recovery walkthroughs. Approved exact old-key denial is HTTP 403 with `access denied`; legacy admin denial is HTTP 401 with `use a console credential`. Other denial expectations remain unchanged. Physical/mobile passkeys are untested. |
| B12 Roles and access durability | Complete for the registered finite controls | Registered role/route, revocation, session/reader expiry, CSRF/origin/RP/UV/replay, publication/audit and restore controls; exact-package supplements and focused security regressions. One owner and readers with visibility of all telemetry; no general ASVS certification claim. |
| B13 Operator PWA | Complete for the registered matrix and supplements | `browser-compatible-alert-04/all-70-terminal-01`: original expected outcomes and independent joins completed. Real packaged assets, lifecycle/offline/cache/isolation, accessibility and bounded UI checks retain their per-cell identities. |

B6/B7 deferrals are the [owner-approved alpha boundary](release-readiness.md#owner-approved-alpha-release-boundary).
They are not waived failures, completed trials or deployment qualification.

## Tested environments

| Tested environment and role | Exact observation | Evidence identity |
| --- | --- | --- |
| Debian 13 genericcloud `20261001-2618`; installation guest on Blade | Fabric `0.1.0~alpha.1` amd64; systemd `257.13-1~deb13u1`; 6 GiB RAM, 2 vCPU | `alert-presentation-deb-life-03-fresh-02-alert-02` |
| Fedora 44 Cloud `1.7`; installation guest on Blade | Fabric `0.1.0-0.alpha.1` x86_64; systemd `259.5-1.fc44`; SELinux enforcing; 6 GiB RAM, 2 vCPU | `alert-presentation-rpm-life-05-retry-03` |
| Debian 13 / Fedora 44 owner server guests on Blade | Same package/systemd versions above; kernels `6.12.111+deb13-cloud-amd64` / `6.19.10-300.fc44.x86_64`; server 4 GiB, 2 vCPU, TCG | Both owner runs above, `server-environment.txt` |
| Fedora 44 / Debian 13 owner edge guests on G15 | Same guest versions above; edge 1,536 MiB, 2 vCPU, KVM; fresh state per direction | Both owner runs above, `edge-environment.txt` |
| Browser matrix on Blade | Chrome `155.0.8059.39`; Firefox `157.0.1-1.fc44`; campaign NSS `3.129`; Orca `51.1`; geckodriver `0.37.1` | `browser-compatible-alert-04/all-70-terminal-01` and original cell receipts |
| Owner browser / pressure fixture on Blade | Firefox actual `157.0.1` / Chrome actual `155.0.8059.39`; later host inspection NSS `3.130.0-1.fc44` | `alert-owner-fedora-debian-04/browser-trusted/receipt.json`; `alert-pressure-a11fa001-smoke-02/browser/receipt.json` |
| Blade execution host | Fedora 44 Workstation; kernel `7.2.9-201.fc44.x86_64`; QEMU `10.2.2-1.fc44`; systemd `259.9-1.fc44` at closeout inspection | Host and campaign receipts retain their own observation times |
| G15 execution host | Fedora 45; kernel `7.2.11-300.fc45.x86_64`; QEMU `11.1.2-1.fc45`; genisoimage `1.1.11-64.fc45.x86_64`; Python `3.15.0` | `owner-split-host-preflight-42`; execution host only, no Fedora 45 product acceptance |

The guide bytes used by both full owner directions are pinned at
[`78b939ff540f71c361f292ad1b12d4c1f0339cce`](https://github.com/kmosoti/FabricO11y/tree/78b939ff540f71c361f292ad1b12d4c1f0339cce/docs).
The Debian-server direction retains harness `7feab64`; the fresh reversed direction
uses `25d6606`. The later changes affect Debian-edge package filename staging and
add a bounded verified-listener observer. Their original source identities remain
explicit; no earlier run is relabeled as a later harness execution.

## Placement, clocks and cleanup

Owner cells reserve both hosts: Blade outer 6 GiB/high 5 GiB, four CPU equivalents,
1,536 tasks; G15 outer 2 GiB, two CPU equivalents, 384 tasks. Both enforce zero
swap and a 1,800-second deadline before payload. The combined outer memory maximum
remains 8 GiB. Installation and main/query/pressure work stays on Blade.

The owned data path is G15 guest gateway `192.0.2.2:7443`, a fixture-only DNAT to
an ephemeral G15 high port, SSH reverse forwarding over Tailscale
the private G15 endpoint to the private Blade endpoint, Blade loopback `7443`, then the server
guest's native Fabric TLS listener. Per-run `edge-placement.json` records the exact
ports, forwarding command, guest rule, boot identity and clock observation.
Native CA/SAN verification and per-node authentication remain in force.

The preflight-42 SSH clock sample bounded G15 minus Blade wall clock to
[-9.801, 273.824] ms under its recorded no-clock-step assumption; round trip was
283.625 ms. Remote guest clock observations had SSH round trips of 198.764 and
201.548 ms. These are uncertainty intervals, not synchronized-clock guarantees;
owner workflow observations do not establish cross-host latency qualification.

Successful guest scratch is removed only after original shutdown and independent
raw evidence grading. Failed runs keep their real exits; private byte-verified
archives preserve their OS state and credentials. Raw journals, Spools, browser
profiles, private keys and access backups are controlled evidence and are not
public release assets. G15 scratch is empty after intact private preservation of six standalone old failed-command receipts; their original exits 120 remain unchanged. Final Blade cleanup and bundle verification remain pending.

## Release limits

The release has one owner. Readers can see all telemetry until expiry or
revocation; it offers no multi-owner or tenant isolation claim. Restore may report
a truthful diagnostic-log rotation gap; the approved adapter requires the exact
native identity/preserved-byte witness and independently checks every returned
gap before the unchanged signal query oracle runs. Physical/mobile passkeys,
physical power loss and the deferred deployment campaigns are untested.
