# Native installation custody input registration

Status: prospective qualification-only amendment, 2026-10-13; NOT RUN.
Coordination [C-092](https://github.com/kmosoti/FabricO11y/issues/59#issuecomment-6139100360)
admits this separate registration from `77d32ece01646f39d97e98b942fa7c776463a19c`.
It extends the [stopped-custody witness](installation-release-protocol.md#prospective-stopped-custody-upgrade-witness)
for native shapes without changing its historical controls or results. No
production mechanism, guest result or package acceptance is recorded here.

## Source authority and the shape conflict

The [installation contract](../../PRODUCT-CONTRACT.md#linux-installation-contract),
[deployment view](../../architecture/deployment.md), [ADR-0013](../../decisions/ADR-0013-deliver-batches-in-order-with-bounded-dedup.md)
and [Spool invariants](../../formal/verification-matrix.md#spool) remain authoritative.
The frozen [node](../../../src/bin/fabric-node.rs) opens
`spool/diagnostics/spindle.log` and collects it as a local source before opening
the Spindle. The [installed config](../../../packaging/etc/node.conf.example)
fixes `/var/lib/fabrico11y/node/spool` and the 268435456-byte Spool allowance.
[OperationalLog](../../../crates/fabric-adapter-linux/src/operational_log.rs)
retains active `spindle.log` and optional `spindle.log.1`, each at most 262144
bytes, with single-link regular private files and a private owned directory.
Lines are at most 4096 bytes including their newline. Rotation renames the
active file over the previous old file when the next whole line would overflow
the active bound. A final stop line can remain uncollected before the next launch.

The existing [reader](../../../tools/qualification/install/custody_witness.py)
rejects the diagnostic directory. Its later all-archive equality, pending-row
restriction and all-cursor tiling also cannot admit ordinary diagnostic growth.
This is a qualification input-shape gap, not evidence of a product custody defect.
The retained three old rows plus pending/unread fixture witness remains required.
Diagnostics are best effort before Spool commit; this amendment does not infer
complete diagnostic delivery from a retained file or a frame CRC.

## Proposed follow-up cards and ceilings

These are prospective maximum net production deltas, excluding tests/docs,
for review before correction. Actual changes must be measured and minimized;
an estimate is not an observed implementation diff. No implementation is admitted
until the exact trust-source preflight and its actual registration red are published.

| Card / owned production paths | Maximum net addition | Reason for the ceiling |
| --- | ---: | --- |
| Reader shape: `tools/qualification/install/custody_witness.py` | +120, at most 537 total lines from current 417 | One bounded directory inventory (about 60), one explicit path selection shared by the existing conservation check (about 25), and append/rotation preservation (about 35). This supersedes the earlier 420-line ceiling only for this separately registered card. |
| Guest producer: new `tools/qualification/install/custody_native.py` | +180 | Fixed-path stopped capture and manifest, source archives/complete query pages, candidate process/journal joins; reuse the reader and existing guest commands. |
| Bridge/admission/caller: `tools/qualification/install/run-qemu.py`, `lifecycle.sh`, and the same `custody_native.py` | +220 combined, separately counted from the producer | Expected #40 tuple and transfer binding (about 60), bounded transfer/evidence custody (about 80), lifecycle entry/exit and unchanged-reader invocation (about 80). One reviewed PR may contain separate commits for these caller cards. |

The total follow-up ceiling is +520, not a target. The smaller approach is one
explicit reader argument; a separate filtered-root adapter would still need the
directory inventory and duplicate the path-selection/conservation logic.
No generic new caller API is registered by the source controls. Optional raw
server-storage rereading is dropped. It is neither a pass prerequisite nor a
claimed independent raw-server commitment witness.

## Full raw root and explicit finite projection

Extend `stopped_snapshot(root, stopped_receipt, archived_allowed_logs, *,
fixture_paths=None)` without changing the default legacy behavior. The native
mode is a one-element tuple containing an absolute archived source path; reject
empty, list, duplicate, relative or unarchived registrations. The native caller
must independently require exactly `('/var/log/fabric-accept/allowed.log',)`.
All three snapshots must carry the same `fixture_paths` value. An arbitrary
caller-supplied path is not authority to select a different native fixture.

In native mode the input is the actual configured, stopped, unfiltered guest
Spool root. Inventory the whole root, including the one diagnostic directory;
allow only its active/optional-old names, rejecting links, aliases, unexpected
entries, nonprivate modes, wrong guest ownership and oversized files. Retain
each diagnostic leaf in `files` and `file_observations` using its root-relative
name. Return `diagnostics[name]` with original `bytes`, SHA-256, size, device,
inode, mode, UID, GID and link count, plus the directory observation. Read with
the existing descriptor-relative no-follow and before/after drift guards.
Host synthetic controls check current UID/private modes; the guest caller must
separately establish the actual `fabricolly` UID/GID and private guest paths.
Native receipts carry strict integer `service_identity.uid` and `.gid`, and the
reader compares diagnostic ownership with those expected values. The producer
captures their actual getent/package/account authority; fabricated receipt values
in a unit fixture establish only that the reader checks the fields.
Host-copy metadata is never substituted for original guest ownership.

Strictly frame-check, decode and hash **every** committed Batch before selecting
any row: fixture rows, diagnostic rows, metric/trace containers and other retained
signals. Preserve original payload bytes/hash, all projected raw log rows with
their original `(node_id, sequence, index)`, all latest cursors and their source
archives. Earlier raw cursor bytes and before/install diagnostic inventories
remain available; this does not reconstruct already missing historical files.
Existing limited metric/trace leaf scope remains unchanged;
product ingest is its semantic authority. Reject incomplete/torn active tails,
failed-writer markers, CRC errors, malformed diagnostic log fields and drift.
Never reencode, replace or expose a filtered/copied Spool as the actual raw root.

Only after full admission, select fixture rows by the registered path and
their exact archived source intervals, never by body text or a substring.
Keep the existing five-row comparison and exact `[0, final_cursor)` tiling on
that fixture path, with newline bytes/device/inode, no missing leading line,
gap, overlap or duplicate. Apply the same path selection to the complete raw
query pages, while keeping the unfiltered pages available. A diagnostic row
whose body equals a fixture sentinel must not enter or duplicate that fixture.
Extra fixture-path query rows still fail exact projected row conservation.
Projection compares node label/ID, sequence/index, observed time, string body
and string attributes; it is not equality of every raw OTLP field. Query,
delivery and rate oracles remain byte-identical.

Stopped before/install inventories, raw sources, ACK, cursor and archives must
remain unchanged. Drain may append/rotate diagnostics, but every pending Batch,
including diagnostic-only Batches outside the five-row projection, still needs
the observed candidate sender's exact original-payload SHA-256 ACK. Retain the
before/install/drain diagnostic inventories and archives even after rotation
replaces an older `.1` entry. Do not impose diagnostic EOF tiling: an uncollected
stop tail is a registered legitimate shape.

For an unchanged active device/inode, after bytes must start with all before
bytes; an existing old file must remain identical. For one observed rotation,
after `.1` must have the previous active device/inode and preserve its entire
prefix; the new active identity must differ. The retained old file plus the
first new whole line must exceed 262144, the new line must fit 4096, and each
file must remain bounded/private/single-link. A short unjustified rename or a
different generation in `.1` cannot stand in for the prior prefix. Multiple
unobserved rotations are refused as insufficient preservation evidence unless
a separately registered observation history supplies every required prefix;
there is no general diagnostic-completeness claim.

## Registered source controls and expected first red

New [hand-encoded controls](../../../tools/qualification/install/test_custody_native.py)
reuse only the old fixture's wire primitives and adapter loader. Their temporary
trees and fabricated process/stop receipts are explicitly synthetic source
controls, not native evidence. Both the old reader and all 17 old control methods
stay byte-identical in this trust-only revision.

The named `NativeShapeAvailability.test_unfiltered_native_root_and_explicit_fixture_projection_are_supported`
must admit a full raw-root shape and expose its explicit projection and diagnostic
inventory. On the original reader, its observed root refusal is converted to a
named assertion failure, not an import/TypeError/setup error or product defect.
Two independent literal methods require original mixed-row indices and rejection
of invalid diagnostic wire/UTF-8 before projection. Shape-dependent methods
explicitly skip until the keyword exists; skips are missing evidence, not passes.

The dependent controls require CRC-valid malformed diagnostic frame rejection,
full append/stop-tail authority, body-collision isolation by path, one observed
rotation, prefix rewrite/unjustified or unobserved rotation refusal, diagnostic-only
pending exact-byte ACKs, fixture-prefix-gap/extra-query refusal, invalid or
substituted fixture registrations, and unexpected/link/alias/mode/size/UID/GID
or noninteger service-identity refusals.
The final unchanged green must run every method with zero skips. Adding the
keyword alone is not an implementation-availability success for these controls.

From this worktree, the exact inside-launcher source command is:

```sh
env PYTHONPATH=tools/qualification/install:tools/qualification:tools \
  python3 -B -m unittest discover -v -s tools/qualification/install -p 'test_*.py'
```

Root invokes it through `python3 -B tools/resource_group.py --`, with a fresh
serial full-1-GiB reservation and default 1800 seconds/16 GiB high/20 GiB max/no
swap. Expected initial registration result: 43 methods, 32 positive methods,
one named unsupported-native-shape assertion failure and 10 explicit skips,
exit 1. This prediction is NOT RUN and must be checked against the actual log.
The existing 30 installation methods are unchanged; the new file adds 13. A
focused reader-only command would instead name `test_custody_witness
test_custody_native` and predict 30/19/1/10; it is not the complete source cell.
Preserve that actual red before any separately committed production correction.
Root/Wren/Pip exact source-pin preflight comes first; review is not run evidence.

## Native producer, admission and caller obligations (later cards)

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
