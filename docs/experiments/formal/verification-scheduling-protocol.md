# Critical-job-first verification experiment

Registered after the FIFO experiment and before implementing or measuring this
composition. The original experiment and evidence remain unchanged.

## Discriminating observation

Source `beb00f653bf39542a8ec89d98ec383f9f8e1c58d` completed seven alternating
one/two-worker pairs. The raw summary at
`results/verification-concurrency-01/native-paired-01/summary.json` has SHA-256
`3aa8cb54a5ac0828cf326e1c65fef3142880e3b523c37f1cdfc59134c8f594bb`.
Serial/two-worker median wall times were 4185/2908 ms. These observations apply
only to its seven independent lightweight checks, not the full verification suite.

The median serial per-check durations were: telemetry-cli 2241 ms,
workload-rate-oracle 887, query-oracle 322, delivery-oracle 310,
telemetry-contract 142, soak-decision-rule 87 and ui-palette 42. FIFO starts the
longest check late. For the frozen additive-duration model, the two-worker lower
bound is max(2241, ceil(4031/2)) = 2241 ms. Assigning telemetry-cli alone to one
worker and the other six to the second attains this model bound. Actual startup,
contention and receipt processing remain outside that arithmetic.

## Candidate and invariants

Reuse the bounded worker pool, immutable command vectors, isolated receipts and
failure controls. Add one scheduling choice: order independent jobs by frozen
descending measured weight, with canonical ID tie breaking. Check the weight
table against the exact selected ID set; reject missing, duplicate or extra IDs
and nonpositive weights. Sorting costs O(N log N), dispatch O(N), with O(N)
metadata and the same bounded worker count. A tiny table does not justify a new
general scheduler, dependency framework or resource policy.

The weighted rule is not globally optimal for arbitrary unknown durations. Its
optimality claim is limited to the frozen seven-job additive model above, and
must be checked by an independent enumeration of all two-worker assignments.
Test tie ordering, invalid tables and every job admitted exactly once; retain
injected exit-7 and copied-evidence controls in every measured mode.

## Matched experiment and decision

On one clean source, compare serial, original two-worker FIFO and two-worker
weighted dispatch. Keep all seven commands unchanged and include full dispatch,
child execution and receipt validation in wall time. Run one warmup per mode,
then seven triples with mode order rotated deterministically. Preserve raw
receipts and compare the complete normalized semantic inventories across every
mode and repetition before interpreting time. Recheck actual one/two worker
peaks. Report model bounds separately from actual measurements and do not infer
per-mode CPU or memory costs unless measured independently.

Use the original launcher, DATA-only output/scratch, fresh 2 GiB reserve and
exclusive engineering lease. Alpha acceptance remains paused. Adopt no
production scheduling change until source checks and independent negative
controls pass. Prefer the simpler FIFO rule if the weighted rule improves median
paired two-worker wall time by less than 5%, or changes any verification result.
