# Transfer Batch ownership at durable append

Registered after the [delivery read screen](catalog-delivery-read-protocol.md),
before owned-append checks. The ACK-position candidate missed its timing rule and
changes post-read corruption detection; remove it from the implementation while
preserving its exact [source snapshot](data/catalog-delivery-read-01/source/src/spindle/spool.rs)
and [raw comparisons](data/catalog-delivery-read-01/probe-comparison.json).
The observer ablation supported its hypothesis; retain the fixture-only final
observer and its complete-prefix requirement.

The new mechanism is smaller: `Spool::append_owned(Batch)` takes ownership and
calls the existing `append_inner`, which already accepts a Batch by value.
Collection and trace commit move their local candidate into this path. Keep
public `append(&Batch)` unchanged for callers needing their input afterward.
The two runtime sites only use the returned committed Batch after success;
on failure they already discard their candidate. Source cursors/counter history
are still advanced only on successful durable append; trace exporters retain
their original request until the existing success/error reply. Validation,
CRC, rotation, sync order, ACK behavior and wire bytes are unchanged.

H1: the owned path preserves the signal Vec and cursor/gap String allocations
across append, eliminating the deep Batch clone. H0: any of those allocations
are replaced. Test nonempty logs/metrics/traces, cursor and gap buffers at log
body sizes 16/256/896 KiB. Independently normalize expected identity/sequence,
compare exact stored bytes and reopen/replay. A borrowed append is the negative
control: with its input still live, returned nonempty buffers must be distinct.
This establishes avoided copies/allocations, not RSS or throughput percentages.

Check malformed, oversized and full-Spool refusal without sequence/cursor
advancement, then a valid retry. Run existing sync/quarantine, runtime collection,
trace delivery and overlap controls through the full fast profile. No new
allocator wrapper, dependency, unsafe code or public port is needed. Existing
validation and serialization allocations remain.

Use one contained closeout job, at most 900 seconds, 3 MiB prospective evidence
reserve after its source snapshot, and the unchanged data-drive/20 GiB/no-swap
limits. Format touched Rust, run targeted Spool controls, the full fast profile,
and manual documentation profile; retain exact commands, exits and receipts.
This consumes the existing frontier (about 3237 seconds after delivery-read01).
The check driver must remove only its exact unchanged Bun executable copy even
on failure, after verifying its retained canonical archive/member reference;
all other failure evidence remains for the normal cleanup workflow.

The preceding service launch omitted `--delegate` from its wrapper invocation.
The host's ancestor delegation still permitted all nested groups, with actual
limits verified before each workload and captured in results. It did not run
uncapped. Future service invocations must use explicit `--delegate`; this
closeout launches no nested service groups. Preserve the original command.
