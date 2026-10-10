# CQ2 seed-publication fixture correction

Status: draft; root registers before the replacement freeze and trials.
The [original CQ2 protocol](catalog-maintenance-protocol.md), workloads, oracle,
negative-control outcomes and strict replay assertions remain unchanged.

Origin: `catalog-maintenance-0-preflight-01` failed native exit 101 at the final
recovery assertion: 30 actual Groups versus the producer ledger's 33, with seed
128 and 32 appends. Frozen build 01 and the failed receipt/stderr/scratch archive
remain evidence. This failed preflight is not a completed CQ2 trial.

The probe constructed Segment label 1 containing only seed Group 1 while that
Group's journal file remained active. Groups 2–4 later shared that file. Store
rotation names a file by its first Group, so it became sealed journal 1. Final
Store reopen reclaims sealed labels with a published Segment of the same label;
Segment 1 represented only Group 1, so this malformed publication fixture omitted
Groups 2–4. This follows [Store startup reclaim](../../../crates/fabric-server/src/store.rs)
and [FrameLog rotation](../../../crates/fabric-frame/src/frame.rs), rather than
evidence of a query or catalog selection failure. Independent source and retained
filesystem review should record the actual journal/Segment ranges in findings.

The smallest fixture correction in
[catalog_maintenance_probe.rs](../../../crates/fabric-server/examples/catalog_maintenance_probe.rs)
opens the seed FrameLog after the writer is closed and exact seed replay has been
checked, asserts every observed Group belongs to the seed, and explicitly rotates
its remaining active frames using their actual first Group as the sealed label.
It releases that writer lock before building any Segment. The normal Store's
4 MiB frame-payload ceiling is preserved. This occurs outside performance spans.
All seed journal files now end before any measured append can enter them.

The full synthetic catalog remains 64 nonoverlapping Segments of 1,024 rows;
small preflight remains one Segment of 128 rows. Segment labels 1..64 are synthetic
descriptor labels; they do not claim one Segment per physical 512 KiB journal.
All seed Segments are published before reopening the Store, their union covers
the entire closed seed range, and their label maximum 64 is below the full append
frontier 513. For small preflight, Segment label 1 covers the complete closed
seed journal 1, and the next active file begins at Group 2. A future append file
therefore cannot share a published seed label. Remaining covered seed journal
files may coexist with Segments and are masked by the unchanged query/replay code.
This is a fixed physical read-catalog fixture, not a live sealer publication test.

The unchanged final replay check (33 Groups for preflight, 544 for full) and
byte-for-byte Batch comparison are the deterministic regression of the original
failure. Every subsequent complete-chain query still uses actual recovered
prefix custody through the unchanged independent oracle. Missing rows/groups
cannot be accepted to accommodate fixture construction.

Root formats the example and captures fresh `catalog-maintenance-freeze-02`
plain/count binaries and source hashes. Both mechanisms/rates use only this
replacement freeze; build 01 cannot be mixed into a pair or summary. Rerun the
same tiny rate-zero preflight under a fresh `catalog-maintenance-0-preflight-02`
output before admitting the remaining fixed matrix. Freeze/trial commands are
identical to the original protocol except these explicit fresh IDs/paths; all
resource/time/evidence limits remain. Root records commands, exits, preserved
failure location and cleanup. No workload was run while preparing this correction.
