# Blade and G15 release placement

Registered before split-host admission. The owner explicitly lifted the alpha
acceptance pause on 2026-10-22 for the bounded release plan. Publication still
requires final owner authorization against the completed bundle. This placement
amendment changes no product contract, workload, oracle or numerical gate.

## Frozen inputs and placement

Keep product source `b1e5874fbf9c6feb611d98ed40386ac44a703fe1`, packaged snapshot
`43150dbd5cff9e0bdb0082997e796978c659d8af`, exact candidate build receipts and
package hashes. Pin the clean successor harness and helpers separately before
acceptance. Retain previous failed/interrupted runs under their original verdicts.

Blade owns candidate builds, all 6 GiB installation guests, server guests,
browsers, main/query/pressure/outage campaigns and full R3 soak. G15 runs one
disposable Debian 13 or Fedora 44 edge using KVM, two vCPUs and 1536 MiB RAM.
Fedora 45 is the execution host; it receives no product-environment acceptance
credit. Record exact package versions, actual guest environments and evidence
identities as tested environments. No concurrent project guest and source job
on G15. Heavy measured campaigns are serial; split cells reserve both hosts.

| Job | Memory high / maximum | CPU quota | Tasks maximum | Deadline |
| --- | --- | --- | --- | --- |
| G15 preparation/edge | 1706 MiB / 2048 MiB | 200% | 384 | 1800 s |
| G15 bounded source checks | 3413 MiB / 4096 MiB | 400% | 512 | 1800 s |
| Blade split-cell controller/server/browser | 5120 MiB / 6144 MiB | 400% | 1536 | 1800 s |

All three profiles require zero cgroup swap and verified actual kernel memory,
CPU and task limits before the payload. Every project job uses
`tools/resource_group.py`; a user-systemd service is its own boundary, never
assumed to inherit Olo's cgroup. Fail closed if host-scaled capacity cannot fit
the requested profile. The split maxima preserve the combined 8 GiB envelope.
Existing longer outage/R3 registrations and Blade-only limits remain unchanged.
First run a disposable budget smoke; if it cannot fit, retain failure evidence
and return affected cells to the original placement without shrinking work.

## Ownership, transport and measurement

G15 storage root is `/home/olo/.local/share/fabrico11y-release`. Blade uses
`/run/media/kmosoti/data/FabricO11y`. Stage immutable packages, helper binaries,
clean harness snapshot and pinned official images once; verify hashes at both
ends. Keep private fixture keys separate from live credentials. Complete package
installation, image preparation and compilation before measured cells. Preserve
Olo, the live Spindle, SSH/Tailscale and host firewall; no host reboot or reset.

Use owned OpenSSH forwards over Tailscale: edge guest -> G15 loopback forward
-> SSH/Tailscale -> Blade loopback forward -> server guest. Native Fabric TLS
trust/SAN verification and per-node authentication remain required. Document
ports, endpoints, boot IDs, forward ownership, command intervals, clock sync and
round-trip clock observations. Monotonic clocks are host-local; retain timing
uncertainty rather than silently comparing different hosts' monotonic values.
Outage controls interrupt only the fixture data forward; management stays up.

Count both hosts' images, overlays, evidence and caches against the existing
100 GB project budget and 95 GB admission boundary, including reservations.
Record per-host actual allocations and free disk before each admission. Never
exclude an unreadable tree or silently replace the data mount.

## Adapter and evidence acceptance

Extend the existing Guest lifecycle with explicit edge host/storage placement;
reuse provisioning, producers, custody/query/rate oracles and lifecycle checks.
Exercise exact transfer plus wrong hash, wrong guest identity, startup failure,
SSH disconnect, deadline and partial evidence. Transport exit zero alone never
means workload completion. Require matching command receipts, original exits,
guest IDs and confirmed owned-process shutdown on both hosts. Preserve failures
and archive receipts before deleting owned scratch. Keep overlays/backing inputs
immutable when retained for a counterexample. Verify live-service identities
after cleanup. Run a cross-host smoke with unchanged oracle expectations before
admitting full split cells. Harness fixes rerun affected controls/cells.

The full [cross-family protocol](cross-family-forwarding-protocol.md),
[owner walkthrough](owner-walkthrough-protocol.md),
[installation protocol](installation-release-protocol.md) and release gates
remain applicable. No pass is recorded by this registration.

## Source-verification namespace retries

The actual `release-final-fast-04` run was interrupted when a workspace test
removed `scratch/storage-2448052-0` while the observer traversed it. Preserve that
interruption and its original stopped receipt; it is not a failed product test
or a successful full-profile run.

For source verification, the existing namespace census may restart complete
scans after `ENOENT` for up to 18 seconds inside the existing 25-second observer
timeout. Every restart resets all totals. Only an error-free complete traversal
returns an observation; permission and other I/O errors still fail immediately.
No path is omitted to manufacture a result. The 95 GB stop, 100 GB budget,
per-host counts and per-job reservations remain unchanged. Namespace sampling
is not an atomic filesystem snapshot; reservations remain necessary. This
operational retry repair changes no product oracle or measured workload.
