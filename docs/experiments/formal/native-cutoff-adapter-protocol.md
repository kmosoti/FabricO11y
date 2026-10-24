# Native cutoff adapter engineering contract

## Retry diagnostics carry no ACK credit

Registered before correction, 2026-10-23. The
[literal Fedora counterexample](fixtures/native-retry-projection.json) retains
guest lifecycle exit0, then cutoff exit2 with `ACK status grammar mismatch`.
The unchanged sender emitted a valid `status=no_response` attempt before a later
successful ACK. The independent Python custody check completed before this
structural projection refusal. The failed native cell remains unaccepted.

The projector shall recognize the sender's existing retry diagnostic grammar:
`delivery sequence=DECIMAL sha256=LOWERCASE_HEX64 status=no_response elapsed_us=DECIMAL`
and the equivalent `status=unavailable` form. These rows bind to the same actual
unit/PID/boot as other recognized sender events, remain in the original captured
journal and grant zero ACK credit. They do not enter the normalized ACK list.
Keep the exact successful-ACK grammar, cursor/sequence/digest checks, stable
prefix, pause/stop windows, stopped capture, inspection health and native
attestation requirements. Unknown statuses, malformed retry/ACK grammar and
missing real ACKs still refuse. No diagnostic producer, product wire format,
normalized plan format or independent Python oracle changes.

Register source controls before implementation: valid retries plus a later
real ACK reach the unchanged actual consumer; replacing the sole ACK with a
retry cannot satisfy it. Reject unknown statuses, wrong digest syntax, extra
committed-through/trailing fields, malformed elapsed values and wrong sender
identity. Preserve every existing negative control and expected outcome. Replay
the untouched actual captured manifest with old and repaired helpers, preserving
their exact identities/statuses; this is diagnostic evidence only. Require fresh
affected native execution with the repaired pinned helper after source/hosted
verification and real merged ancestry. Unaffected package-mutation and prepare
refusal cells retain their original commands and expectations.

## Original engineering registration context

Alpha acceptance was owner-paused at the original registration below. It described a harness implementation experiment,
not a replacement installation protocol or an admitted qualification run.
Use the [primitive protocol](primitive-composition-protocol.md) and its measured
source/ACK join, cutoff reducer and inspection decoder before shared adoption.

## Boundary

The Rust helper consumes a caller-pinned, version1 normalized observation plan.
It validates a structural cutoff and exact ordered byte coverage. It cannot
authenticate a producer, prove omitted records absent, or certify process stop.
Raw journal sender/invocation/executable/cgroup/UID/GID checks, original filesystem
observations, package/source identities and the independent Python custody/query/
rate oracles remain mandatory and separate. Local stdout cannot become native
journal evidence by normalization. Never synthesize missing times or identities.

The plan names candidate, node identity/generation, expected pause revision2 or4,
process incarnation, event window, ordered source/ACK records and a lifecycle
trace. Inspection entries reference pinned raw stdout files rather than copied
counters. Commands retain start/finish bounds; journal entries retain their own
emission times. Inspect intervals must not overlap the preceding selected command.
Pause application can occur during its request interval. Stop begins after the
drain reads finish; the stopped inspection begins after stop finishes.

`cargo xtask custody-cutoff --root DATA --plan RELATIVE --plan-sha256 HEX
--candidate HEX40` performs no effects beyond read-only inspection. Require the
actual resource limits and exact launcher DATA root as existing preflight does.
Use descriptor-owned reads, strict JSON, canonical relative paths, pinned hashes
and final identity rechecks. Plan JSON is at most4MiB; individual inspection text
is at most4096 bytes; combined retained raw/JSON bodies keep the existing64MiB
ceiling. Bound trace/source/event lists to4096/65536/65536 entries respectively.
No new dependency, product format or pure-core dependency is introduced.

## Controls and adoption

First compile and run the extracted kernel's original22 finite controls against
the extracted module. Preserve their literal outcomes and independent reference
shape; API visibility/accessor changes are not algorithm changes. Then exercise
the helper with valid normalized traces, wrong candidate/pin, missing raw body,
body substitution, node/generation mismatch, overlapping intervals, wrong sender,
missing exact ACK, recovery state and the retained commit9/cutoff3 regression.
Keep the actual local capture's non-attesting status. Controls generated beside
the helper are supplemental, not an independent custody oracle.

Shared adoption requires the native caller to replace its duplicated cutoff
policy with this helper, retain raw observations, pin the helper explicitly and
run its affected adapter controls plus the required fast profile. Preserve old
drafts/failures as evidence; do not keep unused alternate dispatch paths in the
successor. Register successor runtime/source bindings before acceptance resumes.

## Native producer effect experiment

Before changing the current guest lifecycle, exercise its actual `guest_main`
against a bounded effect fixture. The fixture must reject stopping the node
without an applied pause and fixed acknowledged prefix. An old implementation
failure must reach that semantic guard, not fail on a missing mock method.
After correction, require pause2/drain/stop/resume3 and, on the distinct reboot,
active3/new-prefix/pause4/drain/stop/resume5 in that order. Keep command intervals,
inspection bodies and complete process observations from the actual methods.
Require a successful inspection after the independently observed stop, before
resume; a successful stop command alone cannot establish stopped state.

Polling is an operational wait, not a custody verdict: bound it by the existing
phase deadline; retry only the existing inspection-busy refusals; preserve all
attempts. Recovery, interrupted append, changed incarnation, wrong desired
configuration and nonmatching fixed prefix must refuse. Do not issue an action
again merely because its application is delayed. Run the preimplementation
counterexample and corrected controls through the resource launcher and retain
their literal outputs. No effect fixture or local stdout run is native journal
attestation or alpha acceptance.

Profile changes in the independent witness and raw-journal binding changes are
separate trust-boundary work. Do not copy the preserved draft's duplicated host
cutoff reducer. Its old caller-card source ceilings do not admit a new runtime;
measure and register the complete successor separately before acceptance.

## Rust projection experiment

Before adding a host projection, exercise a DATA-only Rust function over the
actual Guest record shapes: ordered commands, ordered process observations,
ordered `observed_events`, stopped snapshot node/generation/source metadata and
final candidate process, plus explicit phase, prior ACK, candidate and relative
phase root. Use only existing locked `serde_json`. Bounds are4096 commands and
process samples,65536 journal events and sources,4096 normalized trace entries;
existing4MiB plan,4096-byte inspection and64MiB aggregate input limits remain.
This function emits the existing version1 plan; it does not add a custody oracle.

Validate stream order without sorting or reconstructing clocks. Require a
process sample before pause and a distinct sample strictly after the actual
applied marker and before stop. Bind each used marker, Batch and ACK to its
recorded sender. Require exactly one successful pause, stop and resume action,
two post-marker drain inspections and a stopped inspection before resume.
Incarnations have exactly four fields; unit belongs in window/event records.
Malformed recognized Batch/delivery messages and cross-stream time ties refuse.

Use a hand-labeled synthetic fixture and the literal inspection body to invoke
the real `custody::verify_plan` after projection. Exercise valid drain/reboot,
wrong sender/incarnation, missing or substituted inspection, recovery, absent
exact ACK, stream disorder and an added incarnation `unit`. In particular move
the applied marker after the only second process sample while leaving both
drain reads later: this must refuse. The preserved non-attesting local report
does not meet the input shape and must not be adapted by inventing fields.

After controls, compare a direct indexed-vector prototype with a refinement
that borrows ordered records and retains only fixed selection state. Keep the
same normalized output/refusals and explicit input bounds. Use Rust1.99.0,
edition2024, opt3, codegen1, assertions/overflow checks enabled and debug0.
Use the same synthetic valid fixture with0/10/1000/4000 inert ordered records
added to each command/sample/event stream, construction excluded from timing.
Compile both first, then three alternating pairs with three samples each of
100 projections; compare medians at4000. A regression above50% rejects the
refinement pending investigation. Report source-derived auxiliary bounds
separately from measured timing; neither is allocator bytes or native latency.

The native raw-journal adapter must apply its existing executable, service,
InvocationID, cgroup, UID and GID joins to every recognized Batch and configuration
record used by the projection, as it already does for delivery. Before that
strengthening, mutate each trusted sender field independently for Batch,
configuration and delivery records. Preserve the preimplementation refusals and
misses; after correction run these controls and the existing native modules.
Keep the event output schema and independent Python byte/query joins unchanged.

## Captured-metadata plan composition

After the projection experiment's controls and screen pass, compose it inside
the existing `custody-cutoff` reader. Version2 has exactly `version`, `candidate`,
`phase`, `prior_ack`, `relative_phase_root`, `commands`, `observed_events`,
`process_observations` and `stopped_snapshot`. The snapshot contains exactly
node identity, generation, ordered source sequence/digest metadata and the final
candidate process. Other observation objects retain the actual Guest record
shape, with the required fields and bounds above. Candidate must match the CLI
pin before projection. The existing owned reader admits the complete input;
projection emits the same version1 structure into the existing cutoff checker
and inspection reader. Both versions keep final file identity rechecks.

Version1 remains the direct normalized primitive interface. Version2 adds no
native authenticity claim: independent host raw archive/journal/process/package
joins and the Python custody oracle remain required. Use actual CLI controls for
the composed input, including extra/missing top-level keys, wrong candidate,
post-marker sample refusal, source/ACK mismatch and body substitution. Do not
integrate the DATA-only Python projection sketches or a fallback dispatcher.

## Independent native profile transition

The native lifecycle uses exact stopped profiles1/false,1/false,2/true,4/true
for before/install/drain/reboot, with the two registered allowed/denied log paths
and metric interval15. Before/install byte equality remains mandatory. Add an
explicit boolean native-transition scope to the independent Python conservation
oracle; its default generic/static rule remains unchanged. Do not permit a
caller-supplied expected profile. Require the registered fixture path and four
stopped checkpoints, retain private regular single-link ownership and stable
mode/UID/GID, and allow atomic replacement to change inode and raw digest.

Compare each exact profile with the strict parse of its actual raw configuration
bytes, not a separately supplied cached view. Bind those bytes to recorded size,
digest and original metadata. Retain every source, exact SHA ACK, query, gap,
diagnostic and reboot rule. This is a separate trust-boundary change from the
producer and host caller implementation.

Before implementation, run an exact-transition fixture against the current
oracle using its existing signature: it must reach the static configuration
refusal, not a missing-keyword or fixture setup failure. Then exercise exact
transition success, legacy static success, wrong revision/state/logs/interval,
noninteger/duplicate JSON, byte/digest/metadata substitution and a forged cached
view accompanying coherently altered raw bytes. Preserve both the failed
baseline and the retained Fedora missing116-ACK refusal. These are engineering
controls, not a new acceptance run or reinterpretation of a historical receipt.

## Pinned native caller integration

This continuation implements the existing host bridge; alpha acceptance remains
owner-paused. The producer at `31cf809` already implements pause2/4, fixed-prefix
drain, stopped inspection and resume3/5. The earlier DATA draft's missing-lifecycle
statement is historical and must not drive another producer rewrite.

The owner admission adds the exact `cutoff_helper` record `{path, sha256}`. Its
path is a canonical, owned regular executable under the admitted DATA `results`
root; its lowercase SHA-256 pins the bytes. Freeze that pair and the bridge source
in the existing admission inventory and recheck them before transfer. Preserve
the clean-source and merged-ancestor requirements. No fallback helper search,
runtime build, or acceptance admission bypass is provided.

After independently admitting the original archive, transport its captured
metadata to the existing version2 interface in drain/reboot order, using the
installed/drain prior ACK respectively. Join each inspection member to both the
archive manifest and captured stdout digest. Do not synthesize observations or
reimplement the Rust projection in Python. Preserve the raw journal, process,
package, source, query and Python conservation checks independently.

Copy the helper from a no-follow checked descriptor into exclusively created
private invocation scratch; verify its digest and source identity again. Keep
plan and body files private and exclusive. Existing input limits remain4MiB per
plan,4096 bytes per inspection and64MiB aggregate input. Invoke the snapshot
directly with a timeout of at most60 seconds, additionally capped by the caller's
remaining deadline, and at most65536 bytes on each stdout/stderr stream. Require
exit0, unique-key JSON, structural-cutoff status and the exact candidate. Retain
the helper identity, plan digest, raw output, exit and parsed report; preserve
failure scratch. Remove only owned scratch after both phases and independent
conservation succeed. A successful helper is not native authenticity evidence.

The workspace adapter gate must invoke the affected installation controls with
the actual compiled helper path and digest, retaining the existing parent-module
checks. Exercise valid drain/reboot, missing or changed helper, archive/body
substitution, missing exact ACK, invalid output and failure retention. Reproduce
the original Fedora missing116-ACK refusal from the immutable stopped bundle
under a fresh receipt. Never rewrite its historical result.

Final source checks include the affected Rust/Python controls, full fast profile
and documentation checks. An actual native capture requires separately pinned
prospective admission. The ordinary native caller still requires a merged
harness; an unmerged implementation cannot obtain a qualification receipt.
No package rebuild, release acceptance, merge, tag or publication is admitted by
this engineering continuation.

## Rust-first capture transport amendment

Owner steering on2026-10-20 selects Rust for the capture bridge. The earlier
Python policy-module draft is rejected and retained only as development evidence.
Keep the existing Python raw archive reader and independent custody oracle until
an independently validated replacement exists; they must not acquire another
projection or cutoff decision. Existing guest lifecycle code is unchanged.

Add an explicit transport mode to `custody-cutoff`:
`--root DATA --capture-manifest RELATIVE --manifest-sha256 HEX --candidate HEX40`.
It is mutually exclusive with the existing `--plan/--plan-sha256` interface.
The strict version1 manifest has exactly `version`, `candidate`, `phase`,
`prior_ack`, `relative_phase_root`, `commands`, `observed_events`,
`process_observations`, and `stopped_snapshot`. The last four entries are exact
`{path, sha256}` references to the original admitted captured JSON files beneath
the phase root. The Rust owned artifact reader checks pins and file identity;
each JSON input is at most4MiB and total retained input at most64MiB. Existing
array limits and4096-byte inspection-body limits remain. Unknown keys, mixed
interfaces, noncanonical paths and a mismatched candidate refuse.

Rust selects the source sequence/digest and candidate-process fields from the
original snapshot, constructs the existing version2 value and uses the existing
projection and cutoff verifier. The existing version1/2 primitive interface and
its checks remain supported. No tar parser, new dependency or product format is
introduced. Original commands and inspection bytes are neither reconstructed nor
retimestamped. Independent archive and raw journal checks still establish the
native bindings; structural verification does not replace them.

The existing host reader performs only transport of already admitted members
and explicit invocation of the pinned helper. Test the actual compiled Rust CLI
with both phases, original fixtures, wrong pins/candidate/body, missing exact ACK,
invalid paths, malformed metadata, ordering and bounds. No mocked successful
helper can satisfy the positive integration check. Report the necessary remaining
Python consumer explicitly; repository-wide removal must preserve independently
specified semantics and every registered verifier obligation.


## Active reboot prefix before pause

The retained `spindle-cadence-deb-life-03-fresh-03-restart-03` reboot applied
active revision3 and delivered new Batches, but its operational wait required
identical adjacent high-water marks while collection was active. All121 successful
inspections advanced;102 already ACKed the current high water. The40-second wait
therefore failed despite progress. Preserve the original controller2, incomplete
transfer and [literal inspections](fixtures/installation-active-prefix.json).

Prospectively distinguish this existing active3/new-prefix observation from the
subsequent paused fixed-prefix drain. At that single reboot call, latch the first
successfully inspected high-water strictly above the prior stopped ACK, then
require a successful inspection whose ACK covers that fixed observed target.
Newer appends do not move the latched target. Keep the existing40-second/phase
bounds, busy-error allowlist, raw commands, recovery/interrupted-append refusals,
configuration/process identity joins and all source/query conservation checks.
Reject nonboolean scope and combining active-prefix scope with an exact target.

The pause2/4 drain stays unchanged: two identical observed high waters and ACK
through that complete high water before stopping. Its independent Rust cutoff
projection and negative controls stay unchanged. A source control must show an
advancing active prefix succeeds on the new caller and fails on the old caller;
stale ACK, no new commit, unrecovered state and moving paused prefix must fail.
Repeat affected source checks and fresh native cells. This amendment changes no
product/package, custody oracle, workload, deadline or resource limit.
