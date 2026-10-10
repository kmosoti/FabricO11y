# Service evidence finalization repair

Prospective harness correction, 2026-10-09, during the
[service/recovery diagnostic](service-recovery-research-protocol.md).
Scan completed its semantic/resource gates. Read-only inspection found its
`compact-sha256.json` names a pre-cleanup `cgroup-final.json` digest; the runner
then rewrote that file in `finally`. No other listed digest differed. Walk is
already running the same frozen runner and its result will be inspected too.
Original maps and outputs must remain intact. This is a bookkeeping defect,
not evidence that source/custody/query checks failed or a reason to repeat
unchanged timed service work.

The repair moves manifest generation after final resource/cleanup receipts.
Separately write a reconciliation record for completed datasets, preserving the
original stale map, recording mismatches, and authenticating all current files.
That record does not recreate the overwritten earlier resource snapshot or
retroactively turn the original stale map into a valid final manifest.

The review also found that failure-archive member metadata was outside its cap
check. Reserve final bookkeeping bytes before disposal, account archive plus
member metadata, and reject duplicate/nonregular/unlisted archive members as
well as missing or changed content. Preserve empty directories and re-inventory
originals before deleting failure state. Exercise the real archive/readback
path with small valid, duplicate, changed, missing, added and budget-overflow
controls. All defective controls must be rejected; budget failure leaves the
original tree. No original service gate, oracle or threshold changes.

Run new verification through the same resource launcher and native ledger;
no additional budget or production change. Store the exact original mismatch,
new source, command/exit and controls with the round's evidence. The captured
pre-repair harnesses remain the sources of the two timed measurements.
