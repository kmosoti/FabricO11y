# Registered release package and lifecycle checks

Status: revision 5 amended 2026-10-13 for the existing unreadable-audit gate.
Revision 4 registered 2026-10-12 the frozen candidate's exact candidate and L1
predecessor versions and the dated Debian image. Revision 3,
registered 2026-10-10 before stopped-overlay continuation, named the earlier
candidate; a result belongs to the revision it ran under. This extends the
[baseline installation protocol](installation-acceptance-protocol.md) for the
owner-selected Debian/Fedora release matrix. No result is recorded here.
These checks are finite local guest measurements, not deployment qualification.

Revision 5 preregisters the literal baseline Fedora F3 control
`InstallationShellControls.test_acceptance_fedora_rejects_unreadable_audit`:
execute the actual audit decision with readable-empty audit evidence (PASS) and
an audit-file `grep` exit 2 (FAIL, with no F3 PASS). An unavailable or unreadable
audit already cannot support a pass; this adds its baseline control without
changing guest limits, gate outcomes or the frozen package inputs.

## Stopped-overlay continuation

A separately recorded continuation may finish L2/removal from an immutable
stopped trial whose historical baseline exited 0 and whose exact candidate
L1 upgrade passed, but whose reboot observer failed. Preserve the original
overall failure. Bind its receipt/acceptance/harness hashes, exact candidate and
predecessor, source identity, stopped overlay SHA-256 and official backing image.
Use a new owned overlay with that stopped disk as read-only backing; never
mutate the retained input. Freeze the saved pre-reboot boot ID, config hashes,
account identity and exact row witnesses from that disk before L2. A genuinely
different live boot identity remains required; mere SSH reconnection is insufficient.
Use stdin for the privileged boot comparison so the unprivileged SSH shell
cannot expand protected paths. Record each actual phase exit, caps and cleanup
in a new continuation receipt. This does not qualify unrun baseline criteria on
the candidate or relabel the original failed run. The final new candidate still
requires fresh full installation/lifecycle trials.

## Fixed inputs and containment

Freeze source (including Rust crates, UI inputs, vendored dependencies, package
scripts, helpers, license/notice), Rust 1.99.0, Cargo.lock, a successful exact
console build, package SHA-256, and the candidate/predecessor versions before
execution. Candidate package version is `0.1.0~alpha.1` on Debian and
`0.1.0-0.alpha.1` on RPM. L1 predecessors are the registered historical Debian
package re-versioned to `0.1.0~alpha.0` (payload and maintainer scripts unchanged,
`packaging/reversion-predecessor-deb.sh`) and the constructed RPM fixture
`0.1.0-0.alpha.0`. This metadata does not authorize a release tag.
`build-isolated.py --console-build BUILD --family debian|fedora` builds in the
bounded rootless Debian 12 sysroot; RPM construction uses `rpmbuild` there too,
keeping the same registered glibc floor. Copy verified UI assets into owned
storage; recheck their receipt/hashes in the frozen-source guest before packaging.
A source or asset mismatch blocks construction.

The candidate RPM may instead use `build-rpm.sh OUT CANDIDATE_DEB
--candidate-deb BUILD_RECEIPT [--mutate=NAME]`. Require a completed successful
Debian build/cleanup receipt, its exact package SHA-256, exact candidate Debian metadata,
and its matching frozen-source manifest/bundle hashes. Preserve every regular
Debian data member unchanged, including native binaries, UI assets and notices;
record their hashes plus source and input package identity in RPM documentation.
Use the frozen candidate account-check script for RPM installation. RPM metadata,
scriptlets, dependencies and SELinux context restoration remain Fedora-specific.
Reject historical/wrong-version/substituted payloads before staging. This is
repackaging the same Debian12-sysroot binaries, not an independent Fedora compiler
result. Actual Fedora enforcing-mode lifecycle cells still remain necessary.

Run every host command through `python3 -B tools/resource_group.py -- COMMAND`.
Use only the mounted data drive, a maximum 20 GiB/no-swap descendant cgroup and
the default 30-minute deadline. The package-build container inherits that cgroup;
QEMU is its descendant. One guest at a time, 2 virtual CPUs, 6144 MiB guest RAM,
8 GiB virtual disk, disk-backed owned overlay/scratch, no KVM requirement. Preserve
failures/receipts and stop/reap guest processes before cleanup. Enforce the
existing 100 GB data-tree budget, stop admission at 95 GB including the reserved
8 GiB build or full guest disk plus image/log overhead, and record free disk
before admission. Do not
interpret these guest limits or a tiny fixture as the 100 GB retention-capacity
release result.

Debian: Debian 13 genericcloud amd64 build 20261001-2618/systemd 257, from that
build's [dated directory](https://cloud.debian.org/images/cloud/trixie/20261001-2618/),
SHA-512 `f46f0671…b468c3d` as pinned in the harness. Fedora: Cloud Base Generic 44-1.7 x86_64, from the
[official Fedora image directory](https://dl.fedoraproject.org/pub/fedora/linux/releases/44/Cloud/x86_64/images/),
583729152 bytes, SHA-256
`28680fe5b371a5a82ebf43a31926e086a168e59949d03969c5093e7071f90b7f`.
Match the official HTTPS checksum manifest and the downloaded image before boot.
The Fedora checksum manifest is signed; this harness pins its reviewed HTTPS
checksum and does not claim independent OpenPGP signature validation. Record
actual guest OS, systemd, kernel, PID1, cgroup controllers and SELinux mode.
Fedora must remain enforcing; no permissive switch or broad allow policy is a
workaround. An unavailable tool, image mismatch or unavailable gate is not a pass.

## CI host containment

GitHub-hosted runner bootstrap binds disk-backed `/mnt` to the required data
mount and starts the user systemd manager. Before project children run, configure
`user-UID.slice` as a stricter ancestor: memory maximum is the lesser of 12 GiB
and 75% of actual MemTotal, high is five-sixths of that maximum, swap maximum
zero, CPU quota no greater than the available CPU count. Verify the actual
kernel memory/high/swap/CPU files and write `ci-parent.json`; a missing or
mismatching bound fails closed. `RUST_TEST_THREADS=2` and `CARGO_BUILD_JOBS=2`
pass through the launcher. This hosted-runner ancestor does not alter the local
20 GiB contract or establish release workload acceptance.

## Gates

The existing A1–A12 criteria remain unchanged. Backend package operations are
`dpkg` on Debian and `rpm` on Fedora; guest extraction uses `rpm2cpio/cpio`.
The fixture's explicit 20 GiB retention policy is a narrow delivery/lifecycle
fixture. The release's 100 GB default is independently checked in configuration
and examples, and needs the release storage workload before capacity acceptance.

| Gate | Pass rule |
| --- | --- |
| A2c/A2d | Fresh candidate install refuses duplicate UID0 and GID0 named fabricolly before unpack. Preserve root account/group; delete only disposable duplicate fixture entries. Historical predecessor migration cells do not claim this corrected admission behavior. |
| P1 | Both exact packages contain a complete hash-verified console (HTML, compiled WASM, manifest, normal/maskable icons, worker), CSP metadata, binaries, units, examples and Apache-2.0 license/notice. Reject changed/missing/unlisted/symlink assets, a failed build receipt and a mismatching source identity. |
| F1/F2 | Fedora reports SELinux Enforcing before delivery and after restart/removal; every A1–A12 delivery/sandbox/custody criterion still succeeds. |
| F3 | Boot audit contains no AVC/USER_AVC with a Fabric binary `comm`; retain audit and service journal. An unavailable audit facility is a failed prerequisite. |
| L1 | Install an exact strictly older predecessor, complete A1–A12, upgrade to the candidate with the native package manager, restart services, and preserve exact three fixture rows including source/sequence/index/timestamps/body/attributes, config checksums and service-account identity. Validate installed UI hashes after upgrade. An equal-version reinstall is refused. |
| L2 | Record the guest boot ID, reboot the VM, reconnect only after a distinct boot ID, and observe both enabled services active. Exact retained rows/config/account still match. Fedora remains enforcing. |
| A13a | Native removal stops both services and removes binaries while retaining telemetry/configuration/account. |
| A13b | Debian native purge removes state/configuration. RPM has no purge operation: verify removal preserved both, then explicitly delete the disposable fixture state/configuration. This cleanup is not presented as a package purge guarantee. |
| F4 | Fedora remains enforcing through reboot/removal; post-reboot audit contains no Fabric-process AVC. |

L1's expected bodies are `accept-line-before-start`, `accept-line-during-outage`
and `accept-line-after-pressure`, each exactly once from `spindle-1`. Identity
comparison includes every projected field and uses Python JSON integers. The
query fixture intentionally has a denied-log gap; these lifecycle checks do not
claim full-source completeness or change the independent query oracle.

These are **explicit legacy-admin migration fixtures**. Fresh candidate tests use
`--legacy-server`; upgrade installs a guest-only systemd drop-in selecting
`serve-legacy`. The shipped normal service uses passkey configuration, and the
separate authenticated-console protocol must exercise its production entrypoint.
Legacy-admin success cannot satisfy access-control or authenticated UI gates.

Use `run-qemu.py --family FAMILY --package CANDIDATE --source-commit COMMIT
--run-id ID --memory-stressor parallel --upgrade-from PREDECESSOR` for L1/L2.
Without `--upgrade-from`, the harness runs the ordinary baseline/removal path and
has no upgrade/reboot result. The previous package's hash/version belongs in the
receipt. A synthetic predecessor must be labeled as such and cannot establish
migration from a historical implementation it did not contain.

## Negative controls and evidence

### Prospective removal-checker counterexample controls

Coordination C-035 / C-aud-7 minimized an unenforced-check defect at harness
`de77de7399710c5afd50ae0de981fda6e0b75f30`: negated commands and the first
member of `&&` lists bypass Bash errexit; baseline A13a accepted one live unit.
`InstallationShellControls` in `tools/qualification/install/test_run_qemu.py`
invokes the actual lifecycle remove phase with isolated paths and package/service
stubs. Either active, activating, deactivating or reloading unit, literal Fabric
AVC, missing retained state and incomplete purge must exit1 without the affected
PASS; clean fixtures must exit0. Baseline A13a rejects the same live states.
Service-query controls additionally require both observed states: empty or one-line
output and exit1 despite two inactive lines fail A13a. A literal grep exit2 at the
post-reboot AVC check fails F4; clean inactive queries and readable empty audits
remain positive controls. These are verifier prerequisites, not guest outcomes.
The actual QEMU cleanup AST additionally requires an independently detected RPM mutation
and confirmed process stop; unrelated/provenance failures retain scratch.
These prospective controls precede repair and claim no guest outcome. Preserve
the contained red first; after repair, register its actual fix commit in
`docs/formal/counterexamples.json` in a separate trust-boundary commit. RPM
construction plans explicitly create each fresh mutant output directory before
`build-rpm.sh`; an existing directory is refused. Limits and gate rules remain.

Revision-bound source-only checker execution evidence is recorded in the
[installation checker results wiki](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-installation-checker-prerequisite-results-01).
These controls establish no guest or package acceptance.

### Prospective stopped-custody upgrade witness

Issue #41's Spool/backlog upgrade obligation needs a predecessor-produced
unacknowledged Batch and a distinct unread newline-terminated source suffix
across stopped native install, then exact candidate ACK/query and real reboot.
Historical L1/L2 checks remain required. A retained file hash alone cannot
establish recoverable delivery; shared-Rust `spool_dump` is a diagnostic.

`test_custody_witness.py` independently hand-encodes small FAB1/FAC1/protobuf
fixtures for prospective `custody_witness.stopped_snapshot` and `conserve`.
The initial named `MechanismAvailability` failure records absence of the
adapter; it is not a product defect or corrupt-frame counterexample. Actual
specific refusals remain NOT RUN until that adapter exists. Its unchanged
`CustodyWitnessControls` require complete CRC-checked frames, envelope/wire/u64
bounds, nonterminal sequence/ACK, full Cursor/Btrfs identity and consumed-prefix
CRC, last cursor in sequence order rather than maximum offset, complete stopped
service observations, no-follow owned paths, and exact two-line custody joins.
Reset source identity, missing/changed pending bytes, premature ACK, duplicated
or changed query rows and unchanged reboot identity must be refused. ACK events
must match the independently observed candidate node PID/unit/boot/run window
and exact raw Batch hash; their message alone omits Strand identity.

Run only through the admitted serial resource launcher: default1800 outer and
1GiB reservation for synthetic controls. Native cells keep1800 outer,180-second
lifecycle phases and9GiB whole guest reservation. Preserve failures and raw
source custody, exporting only hashes of credentials. Frozen packages, wire
semantics, query/delivery/rate oracles and all existing gate limits stay fixed.
This prospective registration claims no checker or guest acceptance.

#### Nested OTLP structural amendment, 2026-10-13

Preregister `CustodyWitnessControls.test_nested_otlp_structural_refusals` with
hand-encoded inner OTLP logs inside CRC-valid FAB1/FAC1 frames. A sparse LogRecord
containing valid unknown field99 is a positive, even though its projected body is
empty. ExportLogsServiceRequest field0 (`00 00`), a nested LogRecord field number
greater than `(1<<29)-1`, and an eleven-byte nested varint must raise `ValueError`
for the malformed protobuf field/tag or varint; accepting an empty projection or
rejecting only missing sentinels is insufficient. The locked prost0.14.4
`encoding::decode_key` bounds the key to u32 and tags to1 through `(1<<29)-1`, and
its varint decoder rejects values requiring more than ten bytes. The existing
Rust `Batch::validate` decodes each OTLP signal before admitting its bytes.
These source constraints add no new wire semantics or oracle changes. Run the
named control only through the registered default1800s/full1GiB serial driver;
retain the actual red before a separately reviewed structural-validator repair.

#### Bounded reader and original-byte projection amendment, 2026-10-13

Coordination C-061/X-106 admits a 420-line production ceiling, at most +120 after
the 297-line adapter, with no oracle or limit change. Register seven additional
`CustodyWitnessControls` methods before repair; the three malformed cases and
sparse positive in `test_nested_otlp_structural_refusals` stay unchanged.

`test_generic_reader_positive_forms` admits opaque unknown bytes, nonminimal
valid varints, the largest legal tag/u64 and matched unknown groups, including
100 empty nested groups. `test_generic_reader_structural_refusals` rejects
invalid keys/wires, truncation, overlong/overflowing value and length varints,
lone/mismatched/unterminated groups, 101 nested groups and an unknown field at
exhausted depth. Controls call the existing `fields(raw, types)` reader directly.
`test_packed_numeric_reader_forms_and_refusals` registers its explicit
`('packed', element_wire_kind)` spec for kinds 0/1/5: both unpacked and packed
forms are admitted; malformed packed varints and nondivisible fixed-8/fixed-4
payloads are refused. Arbitrary opaque bytes are never guessed to be messages.

`test_logs_common_schema_and_utf8_refusals` requires declared wires and UTF-8
through ExportLogsServiceRequest/ResourceLogs/ScopeLogs/LogRecord and all
consumed common schemas: Resource, InstrumentationScope, EntityRef, KeyValue,
AnyValue, ArrayValue and KeyValueList. `test_logs_common_and_message_depth_boundaries`
admits literal common fields, opaque bytes and nonzero bools. From the export
root, 48 AnyValue.array/ArrayValue.values pairs place a known string at depth 100
and remain valid; 49 pairs or an unknown scalar at exhausted depth fail.
Each signal export has its own recursion budget, matching pinned prost 0.14.4.

`test_metrics_traces_container_scope` requires strict Export→Resource→Scope→
Metric/Span wire containers; malformed keys/varints/lengths and container wires
fail. Hand-encoded `system.memory.total` Gauge and `system.cpu.time` cumulative
Sum use the actual host shapes in `src/spindle/runtime.rs:266-299` and
`crates/fabric-adapter-linux/src/host.rs:190-217`; a literal Span is also positive.
Metric/Span nested leaves remain opaque, explicitly witnessed by a malformed
nested Gauge/Event fixture that this reader admits without asserting semantic
validity. Product ingest decoding is the leaf-semantics authority; frame CRC
establishes byte integrity, not semantic validity. No general prost parity or
native product acceptance is claimed by these finite controls.

`test_original_payload_log_projection_binding` registers exact literal log rows
for valid unknown groups, last oneof arm and merged optional body/value messages.
The stored original Batch payload and SHA-256 must remain exact. A separately
filtered log projection may feed the byte-identical query oracle; it cannot
replace raw custody or exact-byte candidate ACK evidence. The caller projects
log rows only and must not reinterpret opaque metric/trace leaves through the
oracle merely to obtain those rows. Query/delivery/rate oracles remain unchanged;
native caller authentication against immutable candidate receipts stays separate.

Run these source-only fixtures through the same serial default 1800s/full 1GiB
driver; preserve actual red before production repair and rerun unchanged controls.
Refusals name the violated wire/string/group/depth property. Legitimate-form
rejections or wrong projections on the old reader are harness defects, not
product qualification failures. No execution result is recorded by this amendment.

#### Nested envelope recursion amendment, 2026-10-13

Static review of the bounded repair found that Cursor and BtrfsIdentity readers
restart their recursion budget. Register two literal controls before correction:
`test_cursor_inherits_batch_recursion_budget` and
`test_btrfs_inherits_cursor_recursion_budget`. Locked prost 0.14.4
`Message::merge` starts at budget 100; `encoding::message::merge` decrements it
for each nested message. Batch field 7 is Cursor at depth 1, and Cursor field 8 is
BtrfsIdentity at depth 2 (`fabric-frame::envelope`). Unknown group and scalar
skipping use that inherited context, including its exhausted-budget refusal.

Inside CRC-valid Batch frames, Cursor admits 99 matched empty field 99 groups and
refuses 100; BtrfsIdentity admits 98 and refuses 99. An unknown field 98 scalar inside
the accepted number of groups must also be refused at exhausted depth. Positives
retain the literal cursor identity/offset and exact original payload. Existing
root-reader 100-group positives and all earlier expectations remain unchanged.
This is an envelope/strict-reader correction within the existing 420/+120 scope;
it adds no metric/trace leaf semantics or general prost-parity claim. Root must
retain the contained named red before a separate production repair. These new
controls and native acceptance are NOT RUN in this registration.

Keep the existing named Debian package defects: root-user must fail A6,
no-collision-check must fail A2a/A2b, and no-memory-max must fail A8. The existing
mutation grader must reject unrelated failures and bad provenance/environment.
Fedora package mutations are separate exact RPM artifacts built with
`build-rpm.sh OUT BUILD --mutate=NAME`; grade them with
`run-qemu.py --family fedora --package MUTANT --expected-mutation NAME` and the
same named A6/A2a+A2b/A8 failure requirements. The harness refuses the Debian
repacker on Fedora. An unrelated failure, missing fixture/provenance or NOT-RUN
cannot count as detection. Required RPM negative controls remain unrun until
these artifacts are constructed and independently graded; a baseline success
cannot erase that missing gate.

A Fedora migration predecessor can be constructed by
`build-rpm.sh OUT HISTORICAL_DEB --historical-deb`. Its source is the exact
`readiness-debian12-03` alpha1 Debian artifact, SHA-256
`ac71995b3c8b88cbb6f7dd05d9854e347b7a783caadc1b602f6590d6460f4a2f`.
Preserve its historical binaries, static units, examples, sysusers and account
collision preinstall script. The fixture's `MIGRATION-FIXTURE.json` records
those payload hashes; native RPM lifecycle scripts adapt package operations.
It contains no historical console and is a **constructed predecessor fixture**,
not evidence that an alpha1 Fedora package shipped or was accepted. L1 upgrades
its historical implementation/state into the exact current candidate RPM.

Every receipt records the exact package/helper/image/source identities, command,
exit, chosen A12 fixture, measured resources, real lifecycle stages and cleanup.
A failed, interrupted or missing required phase blocks the corresponding cell.
First-run results and limitations belong in a separate result record.
