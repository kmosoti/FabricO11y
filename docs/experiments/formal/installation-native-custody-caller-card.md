# Native installation custody producer and caller card

Status: prospective qualification-only follow-up, 2026-10-13; NOT RUN.
This card preserves the later obligations separated from the
[native reader registration](installation-native-custody-protocol.md) under C-101.
No producer/caller implementation, native command or result is admitted here.
Source-only controls do not authenticate a guest stop, service identity or transfer.
An exact source preflight and separate native producer/caller admission remain required.

## Prospective production ceilings

| Card / owned production paths | Maximum net addition | Reason |
| --- | ---: | --- |
| Guest producer: new `tools/qualification/install/custody_native.py` | +180 | Fixed-path stopped capture, manifest, complete raw roots, source archives/query pages and candidate process/journal joins. |
| Bridge/admission/caller: `tools/qualification/install/run-qemu.py`, `lifecycle.sh`, and the same `custody_native.py` | +220 combined, separately counted from producer | Expected #40 tuple/transfer binding, bounded evidence transfer, lifecycle entry/exit and unchanged-reader invocation. |

These are ceilings, not targets or observed deltas. Together with the separately
registered +80 reader ceiling, the combined ceiling is +480. No generic caller
API or optional raw-server reread is required by the source registration.
Minimize each actual diff and preserve all earlier controls and independent oracles.

## Native producer, admission and caller obligations


The guest producer reads `/var/lib/fabrico11y/node/spool` in place after stopping
both installed units and every server-owned companion. It retains raw systemctl
query exit/state/MainPID, actual ControlGroup membership and independent process
inventory, boot ID, service UID/GID and stable descriptor/stat inventories.
No literal `stop_confirmed`, `cgroup_empty` or successful exit bit is accepted
without its independently captured, expected-bound raw observation. Any live or
unreadable writer observation refuses capture before source/archive collection.

The producer's fixed guest read-only namespace is `/var/lib`: source archives
and receipts go under a newly exclusive, root-owned 0700
`/var/lib/fabric-custody-fixture/<run_id>/`, with 0600 files. Only that disposable
QEMU guest may set `FABRIC_SCRATCH_ROOT=/var/lib` for reading the two exact
registered roots; host commands always retain their DATA-owned scratch root.
The caller must establish the guest/run/boot/configured-root tuple and exact
allowlist before invoking the reader. This namespace variable authenticates
neither the guest nor any path. The actual Spool stays service-owned; the producer
does not chown, link or relocate it. Export complete raw trees as explicitly
labelled archives after reading the actual root, never as replacement roots.

Capture an unchanged stopped pre-install checkpoint and a separate stopped
post-install checkpoint; record all original guest paths, metadata, raw hashes,
source-prefix bytes/cursor CRCs and observer command/exits. During the bounded
server outage, actually collect the predecessor pending line, then stop the node
and append a distinct unread line. The former must occur in a committed raw
Batch after the durable ACK cursor; the latter must be an adjacent unread source
suffix. Candidate install must preserve that checkpoint. After candidate restart,
capture every query page without body filtering, full ACK journal JSON and an
independently observed node `/proc/PID/exe` hash, PID/start identity, service
UID/cgroup/boot and calibrated run window. ACKs must join that exact sender and
window, every required pending payload hash, Strand and durable ACK cursor.
Observe changed reboot identity and exact projected rows through the existing L2;
unchanged historical lifecycle/removal gates stay required.

On the host, use a new exclusive DATA scratch/output pair per cell. Before boot
and again before admission, freeze and validate the owner-admitted #40 tuple:
FROZEN `8fb6f413179b094288316e25845c137440ba23fa`, package/source/console,
canonical helper manifest and binaries, exact clean merged harness/source/helper
hashes, producer manifest, raw transfer hashes and actual stopped observations.
No guessed main revision or claimed boolean replaces the frozen owner record.
Canonical package source is `73ff77867d10ef521ca985b094e1d177f7786b77`.

| Immutable owner input under `/run/media/kmosoti/data/FabricO11y` | Expected SHA-256 |
| --- | --- |
| `results/installation-package-build/fresh-8fb6f413179b-01-deb-a/receipt.json` | `b10a4e4c2d83d8dbed7c1d6e74673fd377a83123fc1bce31fffc97e034beb9af` |
| Canonical candidate DEB named by that receipt | `8edfc7709f4b5dc3da01c00a1d8c442470298e96f8c8e502cfb14e8c9e713e92` |
| `results/packages/fresh-8fb6f413179b-01-rpm-a/receipt.json` | `eadbbd217804efdeae217fe04d4800c9482cc23650942860df4d756e45ac8434` |
| Canonical candidate RPM named by that receipt | `a54f55e7bf1b2230e026b49d6b86023778a9648ae49b11c6bcb0d558db34a840` |

Read the actual canonical #40 provenance and helper receipts to complete the
expected tuple before the native preflight; this table is not permission to
omit those joins. Output owns only
`DATA/results/installation-native-custody/<run_id>/` and exclusive scratch under
`DATA/scratch/installation-native-custody-<run_id>/`. Retain a manifest mapping
each guest source to its archive, original stat/ownership, digest and raw producer
receipt. Credential files retain only original path/stat/hash witnesses; never
export credential contents. Fixture/archive/source authority remains available.

Native cells: one positive Debian and one positive enforcing Fedora lifecycle;
two additional producer refusals on Debian only. Pending-not-produced deliberately
leaves the new line uncollected and must fail the named missing-committed-pending
check after successful raw decoding. Live-writer deliberately keeps the node
active and must fail the named stop observation before checkpointing. A boot,
credential, capture, timeout, import or package setup failure is not detection.
No candidate install starts after either intended producer refusal.

Host controls mutate explicitly labelled stopped-bundle **copies**: original
payload/CRC/ACK identity, source-prefix interval, transfer digest/path/ownership,
producer/package/helper/source mismatch and incomplete stopped observations.
Credential hash/metadata drift uses literal host records plus the positive native
credential join; no extra credential VM. Controls must fail their named admission
or conservation check with otherwise coherent inputs. Never mutate immutable
actual receipts, packages, raw archives or retained historical failures.

Keep the existing one-guest 2-vCPU/6144-MiB/8-GiB disk envelope, 7/8-GiB host
high/max and zero swap; default 1800-second outer deadline and 180-second lifecycle
phase ceiling. Charge every SSH/helper/copy/query against remaining phase/outer
time; the 20-second outage and bounded readiness/drain must fit those ceilings.
Setup/copy overhead is unmeasured and must be measured before native admission;
an overrun is a refusal, never an implicit deadline extension.
Full admission reserves 9663676416 bytes (9 GiB), stays at or below 95e9 including
reserve, and enforces the unchanged 100-GB DATA ceiling. Measure raw/checkpoint
overhead before native admission; do not silently exceed the reserved 1-GiB
overhead. Stop/reap all descendants before cleanup, retain failed raw observations
and archives, and require actual stopped/cleanup receipts. No native command,
image boot, Rust build or producer/caller completion is established here.

## Prospective fourth-checkpoint implementation ceiling amendment

The [fourth stopped checkpoint](installation-native-custody-protocol.md#fourth-stopped-reboot-checkpoint-prospective-revision-4)
adds phase-specific raw and sender evidence. Source inspection of the complete
uncommitted producer/caller design measured 212 producer lines and 296 caller
lines (including all new host admission and lifecycle call sites), beyond the
original 180/220 estimate. Before any native execution, separately register
maximum **230 producer** and **340 caller** net lines, **570 combined**. The
reader's separate 540-total ceiling remains independent. Actual final deltas,
including every new production helper and call site, must be reported.

The added responsibilities are concrete: independent Btrfs filesystem/subvolume
observations through Linux UAPI `BTRFS_IOC_FS_INFO` and the root-ID
`BTRFS_IOC_INO_LOOKUP` (objectid256), complete nested raw-archive membership and
original ownership checks, exact full-page reconstruction on host admission,
and a distinct reboot process/ACK window. The installed product never imports
these helpers. This expands the source budget only; the fixed paths, candidate,
packages, historical gates, finite five-row projection, independent oracles,
VM/cgroup/disk envelopes, 180-second lifecycle phases, outer deadline and
1-GiB evidence overhead remain unchanged. A native refusal never starts candidate
installation, and no source result substitutes for native acceptance.

For the stopped drain phase, the candidate node is stopped but stays enabled;
the server alone restarts for the unchanged L1 query/config/account/console
checks. The actual reboot starts the node, whose distinct sender joins every
new raw reboot Batch. This prevents an unobserved intervening sender from
committing additional diagnostics between checkpoints. L2 still observes both
healthy services and a changed boot before its stopped raw capture. Full source
and exact native-command preflight remain required; implementation and native
execution are NOT RUN at this registration.
