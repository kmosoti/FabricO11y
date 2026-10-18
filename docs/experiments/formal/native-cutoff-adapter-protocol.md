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
