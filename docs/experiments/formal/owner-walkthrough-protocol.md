# Fresh owner guide walkthrough

Revision 3, registered prospectively for [#46](https://github.com/kmosoti/FabricO11y/issues/46).
Native construction, walkthrough and browser/record acceptance remain prospective;
unit-regression evidence belongs to its actual revision and wiki record.
This protocol supplements [console acceptance](console-access-protocol.md) and
[installation acceptance](installation-release-protocol.md), without regrading
their historical results. Results and the redacted transcript belong in the
[research wiki](https://github.com/kmosoti/FabricO11y/wiki).
The [constructor plan](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-owner-fixture-constructor-plan)
holds prospective commands and revision-bound preparation evidence; its recorded
unit controls do not establish native construction or walkthrough acceptance.

Historical constructor setup failures, minimized archive/process counterexamples
and revision-bound source controls are preserved in the
[constructor source record](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-owner-fixture-source-results-01).
Those controls do not establish native construction or walkthrough acceptance.

## Input and guide boundary

Require fresh completed #40/#41 evidence for original frozen package head
`8fb6f413179b094288316e25845c137440ba23fa`, epoch1791841113 and canonical DEB A/RPM A
receipts. Verify actual linked package hashes, minimized source commit,
manifest/bundle, binaries, complete console and helper identities. Original head
and minimized source commit are different fields. No old pass, guest, credential,
browser profile, state tree or historical package is reused.

Pin a clean full harness commit separately from the artifacts and the actual
**merged** guide commit. Archive exact `docs/operations.md` and
`docs/access-operations.md` bytes/hashes and extract their fenced blocks with
source line spans before execution. PR #63's prospective head is not a merged
pin. Record the documented source-build route to fresh #40 artifacts/checksums;
final release publication is not a circular prerequisite.

Declare OS and role before selecting the guide's OS-specific install block.
Execute fenced `sh` blocks in document order. A link is a callpoint only when its
sentence says “now … then return here”; execute that linked section's fences in
order through the next heading of the same or higher level, apply the conditional
rule below and record the return point. Nested callpoints are allowed with their
source spans and return stack recorded; cycles or an ambiguous extent are guide
defects. Every other link is a reference. A fence introduced by “If …” runs only
when that condition is observed; “Otherwise …” runs only when the corresponding
condition was false. Record the condition and selected branch. An unpaired or
ambiguous “Otherwise” is a guide defect. Inline
code in prose is not executed. Perform and record prose-only browser enrollment,
console revoke, `access.conf` creation and other configuration actions explicitly.
The runner may substitute only HOST/IP,
the declared lab network `192.0.2.0/24`, OWNER_PRINCIPAL_ID, the pasted node
credential and package filenames. It cannot silently add, omit, reorder or repair
commands. An action that cannot be placed consistently is a guide defect.

These rules replace the broader linked-section rule, following #59 C-034.
PR #63 head `c5c5096` prospectively makes restore's offline-recovery link a
reference and keeps its inline recover/start fence as the single executable
recipe. Verify that ordering against the actual merged guide before admission;
the prospective fix is not a final guide pin. Any guide correction needs merged
new pins and a fresh walkthrough. Product-code fixes require new frozen
artifacts and affected qualification, rather than relabeling old results.

## Same-code native upgrade fixture

Construct lower metadata versions from the exact canonical inputs: Debian
`0.1.0~alpha.0`, RPM `0.1.0-0.alpha.0`, then upgrade to canonical alpha1 with native
package managers. Preserve every regular payload byte and maintainer
script/scriptlet, including canonical RPM `CANDIDATE-PROVENANCE.json`.
Compare every data/control tar member header, including directories: exact name,
type, mode, uid/gid, uname/gname, mtime, size and linkname. Keep complete control
contents identical except the one declared Version field. Require RPM provide
and recommend/suggest/supplement/enhance names, flags and versions unchanged.
Normalize only the exact self-provides `fabrico11y` and `fabrico11y(x86-64)`, with
equality flag `8` and the declared `0.1.0-0.alpha.1` to `0.1.0-0.alpha.0` transition;
an unrelated dependency with the same version text must remain unchanged.
Keep the fixture declaration outside the package payload. Changing a builder
hash inside that provenance file is a payload change and must be rejected.
Do not relax the historical predecessor's fixed digest admission.

Write new constructor outputs only beneath the standalone
`FABRIC_STORAGE_ROOT/results/owner-fixture-constructor-01`, never inside frozen
#40 evidence. Refuse an existing output. Archive bounded command/stderr diagnostics
in the driver log, including pre-output refusals. Every exception after staging
begins must leave an explicit failed receipt, diagnosis and preserved staging;
archive/parser errors must not escape with an unrecorded failure directory.
Require preceding controls' confirmed outer stop and actual input/harness/helper
hashes before native construction. Synthetic corruption controls do not establish
native transformation; its own completed record must also confirm outer stop.

### Constructor process-control amendment

Register these real-process controls before correcting command ownership. A
command's normal return, cap failure, timeout or signal interruption must leave
no live owned descendants before its caller hashes, archives or removes staging.
Parent SIGTERM must stop owned command descendants and produce exit2 with an
interrupted receipt, preserved staging and raw stdout/stderr. A typed error while
finishing cleanup must retain already captured diagnostics. Existing 30s payload,
300s build and 10s termination/drain limits remain unchanged.

The literal fixture forks one child that ignores TERM, closes stdout/stderr and
holds a tiny owned staging file; the leader exits normally or waits. Both stay
inside the launcher's cgroup, with a private process group and finite 45s fixture
lifetime. Tests require actual containment and disk-backed `FABRIC_SCRATCH_ROOT`.
The test temporarily enables Linux child-subreaper ownership, restores its exact
old setting and unconditionally stops/reaps the recorded fixture processes.
Cleanup uses pidfds plus PID/start-tick identity, never a process-name match or
an unbound reused PID. The leader atomically records and validates its forked
child's UID, parent, group and start ticks before a bounded pipe handshake
permits child setup. Readiness markers are atomic too. Fixture scratch is removed
only after both recorded identities are confirmed stopped/reaped; missing,
malformed or uncertain cleanup evidence retains the directory and fails the
control. No native installation, project service or guest runs.

| Literal control | Required observation |
| --- | --- |
| Payload leader exits0; closed-pipe child survives | Exact stdout and no live descendant at return |
| Payload real30s timeout; closed-pipe child survives | Timeout remains typed, literal stderr retained, no live descendant |
| Build leader exits0; closed-pipe child ignores TERM | Exit0 and literal stdout/stderr, no live descendant at return |
| Build real timeout with TEST-ONLY1s timer | Typed interruption, literal stdout/stderr, no live descendant |
| Typed final-drain error after actual bounded drain/reap | Both raw diagnostic files retained despite interruption |
| SIGTERM to the actual constructor parent after staging/diagnostics readiness | Interrupted exit2 receipt, archived exact staging and raw diagnostics, no live descendants |

The build controls explicitly assert the requested production300s timeout and
shorten only their real `communicate` wait to1s. The final-drain control similarly
records original300/10/10s requests, uses1s real waits, then injects a typed
TimeoutExpired only after the last real drain/reap completes. It tests retained
evidence, not an elapsed native300s/10s limit or an unbounded hostile pipe.
The parent-TERM control synthetically admits inputs, storage placement, harness
and native metadata queries; it invokes the actual constructor main, staging,
command-owner code and real signal/process behavior. These synthetic admission
fixtures cannot establish canonical-package validation or native construction.

Root runs these exact selectors through the admitted resource driver with its
normal outer1800s containment and full1GiB reservation:

```sh
env PYTHONPATH=tools/packaging python3 -B -m unittest \
  test_payload.PayloadTests.test_extraction_stops_real_descendant_on_success_and_timeout \
  test_packaging.PackageTests.test_build_stops_real_descendant_on_success_and_timeout \
  test_packaging.PackageTests.test_build_final_drain_interruption_preserves_raw_diagnostics \
  test_packaging.PackageTests.test_constructor_parent_term_preserves_interrupted_receipt_and_stage
```

These newly registered controls are not run at registration. Capture actual
head-bound red and stopped descendants before the separate ownership correction;
rerun the unchanged controls and broad packaging/payload suite afterward.

#### Closeout interruption amendment, 2026-10-13

Preregister `PackageTests.test_constructor_closeout_term_preserves_receipt_and_stage`
for the completed-build boundary. It executes the actual `fixture_main` final
closeout statements under the real CLI SIGTERM handler in a finite subprocess,
with tiny completed staging and captured diagnostics; native admission is
synthetic. A readable positive releases the removal gate and must exit0 with a
built receipt and removed staging. A real TERM at that gate must exit2 with an
interrupted receipt, exact retained staging, unchanged raw stdout/stderr and no
claim of completed cleanup. The five-second readiness gate and pidfd-confirmed
parent reap preserve uncertain scratch. Root runs the named selector inside the
same admitted default1800s/full1GiB resource command. This amendment changes no
native package inputs, command limits, earlier control expectations or gate.
Record the actual named red before its separate closeout correction.

Require native strictly-older comparison, exact phase package/source identities,
unchanged account/configuration/retained telemetry, new login and installed
binary/console hashes after upgrade. This establishes only native metadata
upgrade behavior for the **same implementation**, never migration from a released
older implementation. Record that limit in the transcript and release known issues.
Fixture observations are not described as canonical-package observations.
After documented reset, install the actual canonical candidate into empty state
and repeat first owner, independent second key, edge enrollment and all three
received signals. Never downgrade populated state to manufacture an upgrade.

## Placement, trust and containment

Run two serial cells: fresh Debian13 server/Fedora44 edge, then a fresh reversed
pair. Record four fresh guest instances and distinct boot identities, with only
two VM slots active. Reuse pinned `cross_family/guest.py` and `install/run-qemu.py`
placement/SSH/stop primitives; its legacy installation/provisioning commands do
not replace the guide's apt/dnf or normal production `serve` steps. Fedora remains
Enforcing and its audit facility must be available.

Each guest has2vCPU and6GiB virtual disk, server4096MiB and edge1536MiB; outer
memory high7GiB/max8GiB, no swap,1800s default deadline. A browser is a bounded
descendant, at most1GiB/no swap/2CPU/512tasks. Verify actual host CPU/task limits
and effective guest service/slice limits. Full storage reservation is
13,958,643,712 bytes, strict precensus≤81,041,356,288 plus actual free disk.
Recheck census every30s and fail closed at95GB/unreadable entries. Sparse disks
do not reduce reservation; local20GiB never transfers to smaller hosts.

Use guide hostname/origin/RP/SAN and a real trusted CA. Prefer existing Firefox157
NSS/geckodriver setup in an isolated owned profile with acceptInsecureCerts=false;
extend ConsoleBridge, not a parallel browser implementation. Private DNS mapping
may route that hostname to loopback QEMU forwarding while retaining the original
URL and TLS hostname. Import only public CA material; its private key remains
server/offline. No owner-host DNS/trust/profile mutation or certificate bypass.
Actual trusted-positive, wrong-SAN and untrusted-CA browser controls are required
before enrollment; an arbitrary timeout/429 is not certificate rejection evidence.
Virtual resident/UV devices establish only their named emulation, not physical or
mobile compatibility. Archive resident flags/RP/counts, never private keys.
Preserve existing550ms UI/500ms bridge/250ms integer-nanosecond query pacing,
session checks, refusal propagation and independent query/delivery oracles.

## Required observations and bounded phases

The guide's own call graph controls execution; this table defines required
observations, not a replacement command sequence. Archive each actual command,
source span, declared substitution, OS/role/package/boot identity, start/end,
exit and semantic witness. Preserve exact logs/metric points/spans, authorized
query bounds/pages, completeness/freshness/gaps and edge backlog drain.

| Phase | Required observation | Allocated ceiling |
| --- | --- | --- |
| Boot pair | Pinned images, distinct OS/boot IDs, cgroup2, no swap, Fedora Enforcing/audit |600s shared pair budget |
| Install | Exact native guide checksum/install, account/units and initially inactive services |120s |
| TLS/trust | Guide certificate/hostname/SAN/firewall recipe and real browser trust controls |120s |
| First owner | Normal serve, consumed bootstrap, resident/UV owner plus independent second key, companion diagnostics |90s |
| Edge | Guide config/token/ACL, packaged node, real OTLP exporter and received logs/metrics/traces |180s including60s readiness |
| Queries/revoke/restart | Received values, specific unauthorized revoked edge, protected token removal, stale session denial/fresh login |120s |
| Backup/restore/recovery | Stopped complete protected snapshot, move-aside, single recovery before reopening, old authority denied/new owner works |240s |
| Upgrade | Both native version advances, unchanged state/config/account/payload and fresh login |120s |
| Remove/reset/re-enroll | Services/processes absent, state preserved then explicit reset, actual candidate fresh owner/second key/edge signals |165s |
| Archive/stop | Protected failures retained, redacted transcript, all descendants/units stopped |45s |

Ceilings sum1800s; they are unmeasured allocations, not predicted timings.
Apply the shared boot ceiling to the pair, rather than two independent600s waits.
Guide dependency acquisition and all shell blocks consume their phase budgets;
the OTLP loop's socket timeout plus sleep can approach60s. An exceeded phase/outer
deadline remains interrupted, with preserved evidence. No phase omission,
detached descendant or silent deadline extension is permitted.

Require the complete observation set in both directions. Keep protected bootstrap,
tokens/cookies, SSH credentials, passkey/CA private keys and backup contents out
of argv/public transcripts. Retain controlled raw evidence and public hashes.
Only a complete candidate/guide/harness-bound record with real phase exits and
confirmed cleanup can complete #46.

## Controls and admission

Registered [literal controls](fixtures/owner-walkthrough.json) include wrong
original frozen head/epoch, payload or script mutation, equal version, omitted
restore/fresh-candidate enrollment phase, wrong candidate record and unrelated
HTTP refusal. The expanded constructor controls cover corrupt tar/xz, missing or
truncated members, every declared DEB header field and RPM provides/weak metadata.
Only the two expected self-provide EVRs may normalize. A trusted-positive browser
control must succeed; wrong SAN and untrusted CA must reject specifically. For
revocation, HTTP403/429/503, timeout or connection reset are not the required
specific HTTP401 for a revoked credential. Each defect must reject at its named
boundary before affected guest
mutation or a completed verdict; unavailable startup is not defect detection.
Run constructor/record/browser controls prospectively under containment, retaining
raw exits/failures. These remain NOT RUN until actual executable consumers run.
Wren reviews the registered protocol and actual preflight; neither prose nor
model consensus replaces executable evidence. Read #59 before each step and
after each run, stop on unresolved requests/resource/input/guide drift, and
archive/stop/clean the first pair before admitting the reversed pair.

### Production snapshot and observer binding amendment

The production console deliberately uses opaque snapshot identifiers; the legacy
`gN-M` parser cannot grade this walkthrough. Before the first page of each edge
query, retain the actual read-only `fabricctl inspect` observation of its durable
ACK cursor. Immediately after that first page, retain a second observation of its
next sequence. Both observations must have the same guest boot, node identity and
generation, no interrupted/recovery-required inspection, and ordered native host
timestamps surrounding the first request. The pre-query ACK is a lower bound on
the committed edge prefix; post-query `next_sequence - 1` is its upper bound.
Reject an empty acknowledged population, inconsistent bounds or more than 32
candidate prefixes. These observations supply only bounds, never expected rows.

After both writers stop, independently decode complete original server journal
groups and require exact agreement with the source-matched recovery helper. For
each bounded edge prefix, use the existing zero-gap scoped signal projection and
unchanged query oracle. Reject any authorized edge gap, missing sequence or
identity mismatch; preserve all attempted verdicts and the admitted prefix bounds.
The answer's opaque identifier must remain consistent across pages, but cannot
choose its own prefix bounds. A failed query is not retried into a passing result.
Controls reject a legacy identifier assumption, a prefix outside the independent
bounds, a changed node/generation, missing source sequence and other-label or
other-signal metadata leakage. This replaces only the inapplicable legacy
snapshot adapter, not any oracle or acceptance threshold.

Retain stopped native Spool source/cursor dumps, original selected and diagnostic
file bytes, and an independent `/proc`/unit inventory proving writers stopped.
Compare every recovered Batch to its original Spool source with the unchanged
delivery oracle and separately require every durable Spool ACK in recovery; no
wire-attempt observation is inferred. Observe real guest UID/GID, privileges and
effective service/slice kernel limits while active. Require four different boot
identities across the two pairs. A cleanup failure must not prevent stopping the
other owned guest or writing the failed receipt; retain uncertain scratch.

## Trusted Firefox bridge controls before enrollment

The narrow optional ConsoleBridge Firefox backend preserves the supervised owned
wait domain and actual1GiB browser budget. Default Chrome behavior is unchanged.
Firefox uses157.0, retained geckodriver0.37.1 and owned NSS tools; record actual
binary/version identities in each native receipt. Import only the single public
fixture CA into a fresh isolated NSS profile, disable enterprise roots and keep
acceptInsecureCerts false (including returned capabilities). Private DNS maps
only the exact guide hostname to loopback inside that profile; URL/Origin/RP/SAN
are unchanged, with no host DNS/trust mutation or certificate bypass.

Before any enrollment, run three finite native controls using actual supervised
Firefox: trusted CA+matching SAN succeeds with exact secure origin; trusted CA+
wrong SAN refuses with actual SSL_ERROR_BAD_CERT_DOMAIN; untrusted CA+matching
SAN refuses with actual SEC_ERROR_UNKNOWN_ISSUER. Generic driver errors, timeouts
and absent error codes are inconclusive, not certificate refusals. These may use
an explicit miniature TLS page solely to expose the existing bridge passkey-helper
readiness hook; no fake page authenticates an owner or claims package acceptance.
Keep actual TLS error-page code and origin witnesses; archive no secret bodies.
Pure source controls pin capabilities, DNS confinement and narrow authenticator
operations; actual browser controls are independent evidence. Register exact
source/commands/tool pins and bounded fresh fixtures before native execution.

The bridge exposes add/remove/resident_credentials/replace_authenticator. Replacing
removes all existing virtual devices and creates a fresh independent token; it
cannot switch back. Resident inventory returns public resident/RP/count/UV
configuration only, never private keys. Recovery must first demonstrate denial of
the still-active old token before replacing it. This is virtual-device evidence,
not physical/mobile certification. Existing request/UI pacing and source oracles
are unchanged.


## Replacement TLS input and constructor identity, prospective revision 7

Reason: the historical8fb candidate contains rustls0.23.45 and is held after
RUSTSEC-2026-0349. The completed #40 replacement is frozen at
`e078ba23b543126011a2136dd1f7e98ca65370eb`, epoch1792100410, canonical minimized
source `28bcdf7ac833ab634d3b130b85cb2a0aa47eb7a8`. For the next actual constructor
and walkthrough only, substitute this exact identity and canonical receipts:
`results/installation-package-build/replacement-tls-01-deb-a/receipt.json`
(SHA256 `f2a06e16fd8f82e27517eee12a1d8176d052765ac2e75959a85f70cdaeb6fd85`)
and `results/packages/replacement-tls-01-rpm-a/receipt.json`
(SHA256 `4a2a800e8351ec8480359d0912f05d36682d27834c11c60bae87752961a5326a`).
Their canonical DEB/RPM hashes are respectively
`2a555bec6334fde6774d21c414d7d42e041c39ca170e3a30fd3676e134b9f593`
and `12c03f92e96c7d1a848a5198f017c21a1e17d485cdfca7feb8464c3a425971c1`.

The new exclusive constructor output is
`FABRIC_STORAGE_ROOT/results/owner-fixture-constructor-02`. Preserve constructor01
and all historical packages/results unchanged. This supersedes only the frozen
input/epoch/canonical receipt tuple and exclusive output path above; every native
payload/header/control/metadata/version comparator, preceding control/stop gate,
actual clean harness and merged-guide binding, fresh #41 prerequisite before
walkthrough, resource/deadline limit and observation remains required. No actual
construction or walkthrough is established by this registration. The matching
native-installation binding is [revision6](installation-native-custody-caller-card.md#replacement-tls-candidate-binding-prospective-revision-6).

## Effective-memory observation correction

The owner approved this correction and both fresh reruns on2026-10-24.
The stopped owner trial's1536MiB edge had the exact packaged node and slice
kernel settings, but systemd reported the slice's effective memory ceiling at
physical guest memory. The earlier exact comparison against the larger configured
slice values was inapplicable to that registered guest size. See the upstream
[systemd resource-control definition](https://freedesktop.org/software/systemd/man/latest/systemd.resource-control.html).

Fresh observations must record physical memory from the guest kernel and both
root and `system.slice` ancestor memory controls. At the real cgroup v2 root,
record and require the documented absence of `memory.high` and `memory.max`;
[these files exist on non-root cgroups](https://www.kernel.org/doc/html/v6.6/admin-guide/cgroup-v2.html).
Require both `system.slice` controls to be `max`; an unexpected ancestor
cap fails admission rather than changing the registered envelope. Physical memory
must be positive, exceed the role's configured service maximum and not exceed its
registered4096MiB server or1536MiB edge allocation. Compare each effective memory
property to the minimum of its unchanged configured value and observed physical
memory. Require exact kernel node128/256MiB, server2560/3072MiB and shared-slice
2816/3328MiB high/max values, unchanged tasks, service swap0, privileges, writer
inventory, boot identity and all original shutdown checks.

Controls must reject missing/invalid physical observations, unexpected ancestor
caps, a wrong effective value, raised/unbounded configured limits, swap, elevated
privileges and extra writers. Old observations without this witness remain
incomplete. Rerun both full fresh-owner directions after approval; this changes no
product/package, guest RAM/vCPU allocation, host envelope or acceptance threshold.

## Owner-approved retained-resident refusal producer

The owner approved this fixture correction and both fresh full reruns on
2026-10-24, then authorized execution of the remaining release plan. Publication
remains a separate final owner decision.
After offline recovery, the existing account-privacy cover returns decoy allowed
credential IDs. A normal browser can refuse credential selection before sending
login-finish. That local failure cannot satisfy an actual server-refusal check.
The paired exact-package probe observed a genuine old-key assertion rejected by
login-finish with403; the existing backend maps passkey-verification denials to
`403 {"error":"access denied"}`. The earlier401 expectation was inapplicable.

For this negative control only, arm one native `navigator.credentials.get` call
to use resident selection by clearing only its browser-side allowlist. Preserve
the actual server-issued challenge, RP, origin and verification options; restore
the original get function before awaiting its result. Retain the same owned old
resident device until real login-finish returns403 with exactly `access denied`.
Record actual UP/UV/RP, origin, challenge equality and bounded route/status/error
summaries even on failure. Observe only denial error text, never successful
credential-issuing response bodies. No private key is exported/imported, no
signature is synthesized, and no server response, product authorization or
ordinary UI behavior changes. Local failures, timeouts,401,429,503, a wrong body
or successful old-key login cannot pass. Stale sessions, old readers and revoked
edge credentials still require their original401 outcomes.

Before full reruns, a bounded exact-package mechanism must show this producer
completing a valid native resident login with200, then exact403/access-denied
with the retained device after offline recovery. Completeness checks require the
native witness and exact denial as well as the old-owner-denied flag. Both full
fresh-owner directions remain required; prior failures receive
no retroactive credit.

## Proposed restored-diagnostic gap observation

Owner approval is required before admission under this proposal. Run14's
documented move-aside/extract restore changed the diagnostic log's inode while
preserving its complete bytes. The Spindle truthfully committed one
`log rotated; old tail unknown: /var/lib/fabrico11y/node/spool/diagnostics/spindle.log`
gap. The registered zero-gap projection cannot grade that restored fixture.

Keep zero gaps mandatory for fixture and fresh-candidate checkpoints. Only at
restored/upgraded checkpoints, admit exactly one copy of that exact diagnostic
gap in the complete recovered edge prefix, with unchanged node/generation and
contiguous sequences. Require stopped before/after native source observations:
the diagnostic device/inode pair changed and its restored bytes preserve the
entire original prefix; the selected application log retains device/inode and
its exact bytes. Missing proof, another gap text/path, duplicate gaps or changed
application input fails. All original Spool/ACK custody checks remain unchanged.

For each independently bounded query prefix, compare every returned gap entry
on every page to the original committed gap's exact node, sequence, receive time
and the console's fixed `collection coverage gap; signal-specific details unavailable`
redaction. Do not derive expectations from answers. Then remove only the already
validated gap metadata from oracle copies, and apply the existing zero-gap
signal projection and unchanged query oracle to all rows, pagination, retained
window, freshness, completeness and unavailable metadata. Original bytes and
answers remain preserved; gap-only records must not widen signal retention.

Controls reject missing/altered/extra gap answers, wrong source-identity proof,
unrelated application gaps, pre-restore/fresh-candidate gaps, row mutations and
scope/sequence defects. Preserve query pages before grading even on failure.
Require fresh complete walkthroughs in both directions after approval; neither
run14 nor a journal-replay mechanism receives full owner acceptance credit.
