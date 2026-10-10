# Release package installation checkpoint

Date: 2026-10-10. Scope: finite local guest checks under the
[release installation protocol](installation-release-protocol.md) and
[baseline acceptance protocol](installation-acceptance-protocol.md).
This records exact historical artifacts; subsequent native changes require
new package construction and fresh acceptance. It does not establish deployment
qualification or complete the longer release campaign.

## Artifact identity

The original clean source was `c06688c2e093a57ea23f4706c75a2671fa95e456`;
the isolated source snapshot was `fca46236a93a6dfb915bfe9e847f05c50d234ffb`.
The UI was `console-live-build-08`, ten assets totaling 4,742,466 bytes.
The bounded Debian 12 / Rust 1.99 build exited 0, as did the exact-payload RPM
adapter. The latter preserves all 637 regular Debian data members; it is the
same native build, with Fedora metadata/scriptlets, rather than an independent
Fedora compilation.

| Artifact | SHA-256 | Receipt beneath `/run/media/kmosoti/data/FabricO11y/results/` |
| --- | --- | --- |
| Debian alpha2 | `4d0499268ca0d51ac052e854d357cf22458f135aa664e7088baee930b8931a5b` | `installation-package-build/release-debian12-final-01/receipt.json` |
| RPM alpha2 | `a08e5094ef49d5b7fbc0b9275ed5a65766c1e6bee2e59fe088cf90709e182a4f` | `release-rpm-final-01/receipt.json` |
| Constructed RPM alpha1 predecessor | `49c6fc8e1b35b4d5aea8bc9ba8e44a66513d0db64b28b8e3369462d92213f0af` | `release-rpm-predecessor-03/receipt.json` |

The predecessor is a disclosed migration fixture using the historical Debian
alpha1 binaries; it was never a shipped Fedora release. Matching uninstalled
`spool_dump`, `server_dump`, and `spindle_sim` helpers were built from the same
source snapshot. Their manifest binds source/toolchain and executable hashes.

## Actual Fedora trials

Both trials used the checksum-pinned Fedora Cloud Base 44-1.7 image, actual
systemd 259, SELinux enforcing, QEMU TCG, two vCPUs, 6 GiB guest RAM, and an
8 GiB virtual disk. The outer cgroup was verified at 8 GiB maximum / 7 GiB high,
with swap disabled. Conservative storage admission reserved 9,663,676,416 bytes
below the 95 GB stop threshold. Each receipt confirms QEMU termination and
owned scratch removal.

| Trial | Actual scope | Exit / elapsed | Resource unit |
| --- | --- | --- | --- |
| `release-fedora44-final-01` | Predecessor A1–A12/F1–F3; exact candidate L1 upgrade, L2 changed-boot reboot, A13 removal/state preservation, F4 enforcing/no Fabric AVC | 0 / 697.880 s | `fabric-work-fb821f21428e44118335e5174c33924a` |
| `release-fedora44-candidate-baseline-01` | Fresh exact candidate A1–A12/F1–F3 | 0 / 585.462 s | `fabric-work-d6ab358ab58748eca2e4aca5b69ae985` |

The respective launcher peaks were 6.1 GiB and 5.5 GiB, with no swap.
Receipts, frozen guest scripts, acceptance output and witnesses live under
`results/installation-qemu/TRIAL/`. Commands were
`python3 -B tools/resource_group.py -- python3 -B tools/qualification/install/run-qemu.py`
with `--run-id TRIAL --package-family fedora --package` the exact RPM above;
the lifecycle trial additionally supplied `--upgrade-from` the disclosed predecessor.
The archived receipts identify the exact scripts and package hashes used.

Upgrade and real reboot retained exactly the original three log rows and
configuration/account witnesses. The console inventory matched the packaged
assets. Fresh candidate checks exercised identity collision rejection,
service permissions, native TLS, outage/restart, memory/task boundaries and
shared-slice pressure. These results concern the small registered fixture;
they do not measure 100 GB retained telemetry.

## Preserved counterexamples

An earlier RPM placed a native sysusers header ahead of the account collision
guard. Its actual guest failure led to suppressing automatic RPM sysusers
headers while retaining the explicit guarded lifecycle. The failure remains
recorded as failed.

The old stopped-overlay continuation exposed Btrfs device-number changes across
reboot: the source was reread and three retained rows became six. Its diagnostic
and reduced counterexample remain under
`results/installation-qemu/release-fedora44-lifecycle-continue-02/`.
Native correction `889dfac` uses stable Btrfs identity; the two fresh trials above
include that correction. No historical failed run was relabeled.

## Remaining queue

The [persistent package queue](data/release-package-run-01/queue.json) records
successor source/package freeze, fresh Debian installation and lifecycle,
actual guest detection of three RPM mutations, and four distinct-OS
[cross-family forwarding cells](cross-family-forwarding-protocol.md).
The cross-family execution/grading adapters and fifteen contained controls ran
with exit 0; no actual cross-family cell has run. Three exact linked RPM mutation
artifacts were constructed with exit 0; construction is not guest detection.
VM admission is held during the separately admitted query/browser/soak work.

Hosted CI run `38067490757` completed all twenty fast receipts with exit 0 and
the separate dependency/WASM/contrast/Firefox steps. Chrome failed at session
creation before authentication checks; its original logs lack browser stderr.
The prospective CI-only diagnostic patch `0eb10c3` captures initial stderr and
actual sandbox status, with a narrow Ubuntu user-namespace retry only for an
observed restricted-sandbox failure. Local sandbox-page probing and three
negative controls exited 0; the hosted cause and retry remain unobserved.
