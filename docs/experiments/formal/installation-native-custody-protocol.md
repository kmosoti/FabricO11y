# Native installation custody input registration

Status: prospective qualification-only amendment, revision 3, 2026-10-13; NOT RUN.
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

## Reader correction ceiling

The prospective reader card owns only
`tools/qualification/install/custody_witness.py`: maximum +80 net production
lines, at most 497 from the current 417. C-101 supersedes the earlier +120 ceiling
before this registration is executed. Reuse the strict common schemas and existing
frame/ownership checks; one native directory inventory, one shared path/interval
selection and one append/rotation comparison are sufficient responsibilities.
The estimate is not an observed implementation diff. No production correction is
admitted until the exact registration source preflight and actual red are published.
The [separate prospective producer/caller card](installation-native-custody-caller-card.md)
retains native cells, source admission, ownership, transfer and containment duties.
Those later obligations are not completed or authenticated by source fixtures.

## Full raw root and explicit finite projection

Extend `stopped_snapshot(root, stopped_receipt, archived_allowed_logs, *,
fixture_paths=None)` without changing the default legacy behavior. The native
mode is a one-element tuple containing an absolute archived source path; reject
empty, list, duplicate, relative or unarchived registrations. The native caller
must independently require exactly `('/var/log/fabric-accept/allowed.log',)`.
All three snapshots must carry the same `fixture_paths` value. An arbitrary
caller-supplied path is not authority to select a different native fixture.

In native mode the input is the actual configured, stopped, unfiltered guest
Spool root. Require the diagnostic directory with mode 0700, service ownership
and active `spindle.log`; a filtered/rebuilt root without them cannot be native input.
Inventory the whole root, including that required directory; allow only its
active/optional-old names, rejecting links, aliases, unexpected
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
The caller-provided diagnostic archive must exactly match the in-root active
file's bytes and guest device/inode in each stopped snapshot. Neither an unrelated
archive nor fabricated host metadata is authority to alter that join.

Strictly frame-check, decode and hash **every** committed Batch before selecting
any row: fixture rows, diagnostic rows, metric/trace containers and other retained
signals. Preserve original payload bytes/hash, all projected raw log rows with
their original `(node_id, sequence, index)`, all latest cursors and their source
archives. Earlier raw cursor bytes and before/install diagnostic inventories
remain available; this does not reconstruct already missing historical files.
All signals' Resource, InstrumentationScope and schema URL containers use the
locked common message/string schemas, including UTF-8 Resource attribute keys
and Scope names. Only nested Metric/Span leaves stay opaque; product ingest owns
those leaf semantics. No general prost-parity claim follows.
Retain decoded `collection_gaps` strings in the returned Batch metadata and each
source record, in their original order; original payload bytes remain authority.
Reject incomplete/torn active tails, failed-writer markers, CRC errors,
malformed diagnostic log fields and drift.
Never reencode, replace or expose a filtered/copied Spool as the actual raw root.
Common Resource, InstrumentationScope and schema-URL fields in all three signals
are typed messages/UTF-8, not opaque Metric/Span leaves. The C-096/C-101
controls below register their separate existing strict-reader gap.

Only after full admission, select fixture rows by the registered path and
their exact archived source intervals, never by body text or a substring.
Keep the existing five-row comparison and exact `[0, final_cursor)` tiling on
that fixture path, with newline bytes/device/inode, no missing leading line,
gap, overlap or duplicate. Apply the same path selection to the complete raw
query pages, while keeping the unfiltered pages available. A diagnostic row
whose body equals a fixture sentinel must not enter or duplicate that fixture.
Extra fixture-path query rows still fail exact projected row conservation.
Before that finite projection, every log row of every retained Batch with sequence
above the pre-install ACK must appear exactly once, with exact projected fields,
in each complete drain and reboot query answer, regardless of source path.
Dropping the same diagnostic row from both answers does not establish custody.
The acknowledged query baseline includes the first diagnostic row and three old
fixture rows; each native-shaped Batch carries both source cursors. Preserve
that full baseline separately from the three fixture rows.
Unexpected fixture-path pending rows fail `unexpected pending log row`; an exact
five-row answer cannot erase an additional committed allowed-path row.
The actual [fixture enrollment](../../../tools/qualification/install/acceptance.sh)
adds the credential label `spindle-1`; that label is not present in Batch bytes.
[HTTP authentication](../../../crates/fabric-server/src/http.rs) supplies the
credential label to the committed Entry, and [server extraction](../../../crates/fabric-server/src/rows.rs)
preserves that label in query rows. The reader's fixed `spindle-1` is an explicit
fixture expectation, never evidence of the actual enrollment. All three old query
rows must retain that label and the node ID decoded from genuine FAI1/Batch input.
The native caller must independently join the observed enrollment/trimmed-token
hash, immutable credential-file stat/hash, actual Spool node ID and complete
unmodified query pages. Never synthesize labels, UUIDs or projected query rows
from source bytes to satisfy this join. A source-derived expected projection
and a genuine queried answer remain distinct observations.

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
Collection gaps on other sources stay visible without claiming diagnostic EOF
completeness. A gap naming the allowed fixture source in a pending or unread
Batch refuses fixture acceptance (`fixture source gap`); an exact row answer
cannot hide unknown fixture coverage. Gap strings use the actual source forms
from `log_source::issue` and the unavailable-source path in `Spindle::collect`.

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

The [hand-encoded controls](../../../tools/qualification/install/test_custody_native.py)
reuse only old fixture wire primitives and adapter loading. Temporary trees and
process/stop records are synthetic source controls, never native receipts.
All 30 original installation methods, production and the three independent
oracles stay byte-identical. `test_query_oracle_stays_byte_identical` fixes the
query oracle SHA-256 to `fc696ee1173093c258ff4451c4a796dd8e5832761f9e0ff49f89a922b411d23a`.
No wider query-oracle caller scope is introduced by this card.

C-101's Wren review supplies the new prospective controls; its independent probe
is review evidence, not Olo's contained registration run. Commits `bf4320ef` and
`a273fd21` and their earlier predictions remain history; this revision supersedes
them before the first owner source cell. No published failure revision is rewritten.

| Requirement / literal control | Expected behavior |
| --- | --- |
| Full-root availability | Acknowledged starting diagnostic, pending diagnostic and all original payloads remain available; the explicit fixture tuple is recorded. Legacy reader's actual first error is `unexpected or failed Spool input`, converted to the named `native reader shape unavailable (qualification harness, not product): unexpected or failed Spool input` assertion failure. |
| Common containers | Field-zero ResourceMetrics/ResourceSpans/ScopeMetrics messages refuse. Valid literal Resource/Scope metadata accepts. |
| Common UTF-8 | Across metrics/traces, invalid Resource attribute key, Scope name, Resource schema URL and Scope schema URL refuse: eight cases independent of the native keyword. Existing two logs URL cases also refuse. Metric/Span nested leaves remain opaque. |
| Query oracle freeze / legacy root | Immutable query-oracle digest and legacy refusal of diagnostic directories remain required. |
| Original decoder authority | Mixed literal indices roundtrip; malformed diagnostic wire/UTF-8 cannot hide behind path projection. Every original Batch is decoded and hashed without mutation. |
| Native root completeness | Require private owned diagnostics directory and active file; missing directory/file, mode 0755 and symlink refuse. |
| Failed/interrupted frame custody | All seven existing legacy frame/marker/ACK-sidecar mutations also refuse with the native keyword, including both failure markers. |
| Diagnostic archive join | Archive bytes/device/inode must agree with the actual in-root active file. |
| Pending raw query rows | Drop, duplicate or rewrite pending diagnostic seq4/index1 in either answer or both refuses exact raw-row conservation before projection. |
| Path selection | PENDING and UNREAD bodies each collide in seq4 and in a later diagnostic row; only the registered path contributes the five fixture rows. |
| Extra pending fixture row | Legacy and native controls require `unexpected pending log row`, even when complete query answers retain the five intended fixture rows. |
| Rotation/preservation | Genuine full-file rotation accepts with prior active identity in `.1`; short/exact-cap rotation, changed generation/inode, first line over4096 and old-file rewrite without rotation refuse. Unchanged old file without rotation accepts. |
| Collection gaps | Literal decoded gap stays visible. Other-source gap remains visible and does not imply complete diagnostics; allowed-path pending/unread gap refuses fixture acceptance. |
| Existing finite checks | Diagnostic-only exact-byte ACK, source-prefix tiling, extra query row, substituted/invalid path registration, leaf alias/mode/size/ownership/type and old-row label/UUID controls remain required. |

The first diagnostic row occurs in seq1 and `queries.before`, before the three
fixture rows; every Batch carries both cursors as the native runtime does.
Native-shape changes are literal registrations, not claims that synthetic bytes
or fabricated current UID authenticate a guest. The exact enrollment label
`spindle-1` is established by `acceptance.sh:223`; raw FAI1 supplies the UUID,
while HTTP authentication supplies the query label. Never relabel genuine pages
or manufacture a query answer from raw expected rows. Consistent predecessor
query label/UUID substitution still refuses an old-row identity/baseline check.

Primary authority is the locked `opentelemetry-proto=0.33.0` generated
ResourceMetrics/ScopeMetrics and ResourceSpans/ScopeSpans message and string
attributes, `prost=0.14.4` string decoding, `Batch::validate`,
`OperationalLog::append` (strict overflow `>262144`, line `<=4096`),
`fabric-node.rs:157` (starting event) and `spindle/runtime.rs:1197-1222`
(all configured cursors). Common Resource attribute keys and Scope names are
UTF-8; their bytes are not opaque Metric/Span leaves. The original raw payload
and source archives remain authority throughout projection.

From this worktree, the exact inside-launcher source command is:

```sh
env PYTHONPATH=tools/qualification/install:tools/qualification:tools \
  python3 -B -m unittest discover -v -s tools/qualification/install -p 'test_*.py'
```

Root uses `python3 -B tools/resource_group.py --`, a fresh serial 1-GiB
reservation and default 1800 seconds/16-GiB high/20-GiB max/no swap. Predicted initial result:
60 methods:35 positive, eight named failing methods with15 failed assertions/
subtests,17 explicit dependent skips, exit1. This is NOT RUN.
Failure entries: native-root availability1; common container3; schema URL4;
old query identity2; Resource-key/Scope-name UTF-8 four; visible gap1.
The original30 plus new30 definitions total60; native17 methods individually
skip until the keyword exists. Missing gap metadata is an assertion failure,
not a KeyError. The unchanged final green must execute all60 with zero skips.
Preserve the actual named red before separately committed production correction.
Exact source/tree/ancestry and every changed-path pin go into plan02 for
independent review before execution; review is not completion evidence.
QUAL-INSTALL/QUAL-H mapping stays queued until the first genuine native result.

## Later native integration

See the [prospective producer/caller card](installation-native-custody-caller-card.md)
for both-family positives, Debian producer refusals, owner-admitted package/helper/
source binding, credentials, raw observations, transfers, deadlines and cleanup.
Those requirements remain mandatory and NOT RUN; no source-only green completes
installation acceptance or authenticates a guest stop/service UID.


## Fourth stopped reboot checkpoint (prospective revision 4)

The three-phase strict rule rejects a genuine newly committed startup diagnostic
in the reboot answer. The deterministic `RebootRawControls` counterexample is
synthetic source evidence, not a native trace. Native admission now requires a
fourth stopped raw checkpoint after reboot; the optional three-checkpoint API
remains only for legacy and explicitly synthetic callers.

The drain answer must equal its full acknowledged baseline plus every raw log row
above the initial ACK from checkpoints through drain. The reboot answer must equal
that baseline plus every such raw log row through reboot. Later evidence cannot
justify an earlier answer. Check conflicting sequence/payload or row identities
before forming either union; reject missing, duplicate, forged and arbitrary extra
rows. Preserve the exact five fixture rows, stopped before/install equality,
original retained payloads, all source archives, monotone durable ACKs and the
append/single-rotation diagnostic preservation rule between each stopped phase.
Retain even reclaimed prior raw authority. The fourth snapshot binds the same
candidate/Strand/fixture registration and an actually changed boot. Every new
reboot Batch requires its exact payload SHA-256 ACK from the independently
observed reboot candidate PID/start identity, executable, UID/cgroup and calibrated
window. These requirements add coverage; no filtered answer or arbitrary extra
row is admitted. The independent query/delivery/rate oracles remain unchanged.

Register `test_fourth_raw_snapshot_admits_exact_new_reboot_diagnostic` as initial
red (missing fourth-snapshot API). The old strict refusal remains a required
counterexample. Separate controls remove/duplicate/forge the extra row, move it
into the earlier answer, conflict retained raw identity, reuse boot, substitute
ACK PID/hash, omit ACK or misattribute the extra source. Every control must refuse
at its conservation/sender boundary with otherwise coherent synthetic input.
The reader refinement has a separately justified maximum +55 net lines over
`30ef218` (540 total), replacing the earlier 497 total ceiling only for the
fourth checkpoint and shared phase conservation/ACK checks. The producer remains
+180 and bridge/caller +220; no cosmetic compression is required to satisfy them.
Native VM execution still requires separate exact source/command preflight.


The fourth checkpoint also compares the independently observed diagnostic Btrfs
namespace before choosing inode identity across reboot: equal filesystem UUID /
subvolume permits changed kernel `st_dev`, while changed namespace refuses.
The append/rotation and retained-old-file byte invariants remain exact. A literal
post-reader metadata control is explicitly synthetic and cannot authenticate a
guest mount. The native producer's UAPI observations, source archive joins and
changed-boot sender remain mandatory. The existing non-Btrfs device/inode rule
and all three-phase controls remain unchanged.
## Applied configuration counterexample and raw failure preservation

The first replacement native Debian cell `replacement-deb-life-01`, clean
`da250b7ab135374d1231ab66f2a454e367884fec`, exited2 after guest acceptance0:
the reader rejected the original root before its first snapshot. Original
qcow and failure bundle remain retained. Diagnostic01 was inconclusive despite
outer0 because debugfs returned0 after refusing filesystem open. Diagnostic02
read disposable copies with debugfs1.47.3 `-n`; metadata checksums were bypassed,
the ext4 image needed journal recovery, and the exported ACK inode was stale.
Those exports are diagnostic bytes only, never a valid stopped snapshot or
native acceptance. The original qcow SHA256 remained
`7d87a7288baa941cfe030eae9a00f87154c57e2f1aabc7bea084e0bcd014fc12`.
It exposed the134-byte private service-owned `applied-config.json` containing
revision1, paused=false, the allowed/denied fixture paths, interval15.
Independent source `RemoteView` denies unknown fields; the runtime persists it
by synced rename before activation. The earlier synthetic root omitted it.

Prospectively recognize only that named additional native metadata leaf.
Read it with existing no-follow/one-link/drift guards, limit65536 bytes, mode0600
and independently observed service UID/GID. Require UTF-8 JSON with no duplicate
or unknown fields: u64 revision, boolean paused, list of at most16 absolute
log paths of at most240 UTF-8 bytes, integer interval1..3600. Preserve exact
original bytes, digest and metadata in snapshot/raw authority. Native qualification
capture requires this file and the exact observed fixture configuration above;
absence remains supported only for legacy or explicitly synthetic reader callers.
Unknown leaves, staged `.tmp` files and both failed-writer markers still refuse.
The stopped before/install invariant remains exact. The registered #41 lifecycle
does not change configuration, so its sidecar must also remain unchanged through
drain/reboot. This rule does not prohibit legitimate revision changes in the
separate owner-guide execution, which follows its own configuration transcript.

Register `AppliedConfigurationControls`: original-byte preservation; duplicate,
unknown, scalar/path/range/UTF-8/size refusals; private-mode/link refusals; changed
configuration at drain refusal. Register the producer counterexample
`test_original_raw_tree_survives_semantic_reader_refusal`: export the entire
independently stopped original root before semantic parsing, and retain that
export if parsing refuses. A synthetic injected parser failure is not a native
failure simulation. All existing frame/source/query/sender/fourth-phase controls
remain required, with unchanged expected outcomes and independent oracles.

The smallest readable measured reader delta is+30 lines (515 to545); register
a550-total ceiling specifically for this typed metadata boundary. No VM, resource,
deadline, source authority or acceptance threshold changes. Actual source controls
and the restarted native cells must run on separately recorded pins; no result
from the first failed attempt is relabeled.
## Independent host configuration admission join

Native bundle admission must independently require the original sidecar and
validate its typed bytes/ownership against the stopped service identity and
the exact #41 fixture profile. Source-frozen producer checks alone do not supply
that host join. `admit_transfer` has only the native installation runner and
synthetic capture tests as callers; owner-guide execution does not call it, so
its legitimate configuration revisions are unaffected. Update literal complete
bundles with the real sidecar shape, retaining synthetic classification.
Register `test_coherently_rehashed_missing_or_forged_configuration_is_refused`:
remove it from every snapshot/raw tree or coherently replace paused=false with
true; recompute metadata, raw/member/manifest/transfer hashes. Both copies must
refuse at `native applied configuration` while the complete baseline passes.
No missing byte may be justified by a matching fabricated inventory.
