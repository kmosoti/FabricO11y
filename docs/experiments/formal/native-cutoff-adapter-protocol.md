# Native cutoff adapter engineering contract

Alpha acceptance is owner-paused. This is a harness implementation experiment,
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
