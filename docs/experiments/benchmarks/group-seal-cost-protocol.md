# E2R cost follow-up: equal batching before batching gains

Status: registered before cost implementation or timing. Run only after the
[sector/error model](../../../tools/seal-probe/CONTRACT.md) and its selected
candidate pass independent review. This remains a research writer on disposable
files. It does not change the application's FOL2 contract.

## Hypothesis and baseline

Hypothesis: one file sync per group reduces elapsed ingest cost versus two on this
host. Compare the **same sector-aligned encoding and group size** first. The control
writes descriptor/frames, syncs, writes the seal, then syncs. The variant writes
descriptor/frames and seal, then syncs. Both release the caller's group only after
all required calls return successfully. Errors abort the trial, retain the source,
and forbid automatic continuation on the affected file.

This isolates sync placement/count within the proposed encoding. It is not a direct
FOL2-versus-group-seal performance ranking: format padding, interpreter costs and
input shape differ from [S0](append-attribution-s0.md). Report that distinction with
any numbers. The finite sector model's assumptions are not established by a process
restart on this workstation.

## Workload and boundaries

For each modeled sector size 512 and 4096, begin with group size 1. Then repeat with
2, 4 and 6. Each trial contains 1,200 opaque bodies, cycling the six frozen lengths
57, 127, 509, 513, 1023 and 4096 with bytes from the frozen seed-11 fixture. These are
framing payloads, not complete Fabric Events or validated UTF-8 log messages.
Use one warmup per configuration, then five paired trials with alternating
one-sync/two-sync order. Each pair uses fresh files on the repository filesystem;
run one writer at a time and record other active experiments/background load.

Generate source bodies and create/open/sync the fresh parent directory before
timing. Include group encoding, all writes and all file syncs in the ingest interval.
No per-event output belongs inside timing. Record encoding elapsed time separately,
total process CPU and wall time, successful sync call count, file bytes and process
peak RSS. CPU includes the interpreter and libraries. The process-level resource
measurement also includes setup/verification if those phases are not separated;
label its boundary explicitly rather than attributing it solely to ingest.

Close each writer and verify in a separate process. Compare every byte against the
independent frozen fixture encoder and all recovered bodies/group boundaries against
the expected input. Preserve raw trial JSON, commands, exits, source hashes,
toolchain/platform/mount information and output-file hashes. Any failed gate makes
that trial a failed experiment, never a fast successful sample.

## Metrics and decision rule

Report events/s, group commit P50/P99 (nearest rank), CPU seconds/event with its
boundary, syncs/event, bytes/event and peak RSS. For group size 1, a material elapsed
benefit requires median paired throughput improvement >=10% and median pipeline CPU
cost <=110% of the two-sync control. All pairs must pass exact replay and file-hash
equality; sync counts must equal groups or twice groups respectively. Report every
trial and variation. Then report batching results separately; do not pool them into
the group-size-1 claim.

A positive result supports a later Rust/durability investigation only. A negative
result rejects the measured benefit for this workload. Neither selects a production
storage format, demonstrates arbitrary corruption recovery, measures device IOPS,
or removes the need for an independently retained I/O-error witness.
