# Fresh owner guide walkthrough

Revision 2, registered prospectively for [#46](https://github.com/kmosoti/FabricO11y/issues/46).
Native construction, walkthrough and browser/record acceptance remain prospective;
unit-regression evidence belongs to its actual revision and wiki record.
This protocol supplements [console acceptance](console-access-protocol.md) and
[installation acceptance](installation-release-protocol.md), without regrading
their historical results. Results and the redacted transcript belong in the
[research wiki](https://github.com/kmosoti/FabricO11y/wiki).
The [constructor plan](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-owner-fixture-constructor-plan)
holds prospective commands and revision-bound preparation evidence; its recorded
unit controls do not establish native construction or walkthrough acceptance.

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
