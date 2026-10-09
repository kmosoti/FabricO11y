# Failure archive location repair

`failure-preservation-01` passed full membership/content verification for eleven
trees. Its helper wrote large archives beside repository reports. The subsequent
data-drive relocation copied and read back all hashes, but the enclosing ledger
correctly rejected the repository symlinks (`linked or special evidence/scratch
member`). That attempt remains failed, with child exit 0 and coordinator exit 1.

Restore the existing no-symlink policy using ordinary JSON location records;
do not relax the census. A bounded metadata repair must run directly inside the
resource launcher because the normal ledger cannot admit a job while these
links remain. Its first attempt rejected relative/absolute path disagreement
before mutation; the corrected attempt replaced eleven exact owned links and
deleted no archive. Preserve both launcher receipts and charge their measured
maintenance duration alongside the ledger, with no allocation increase.

The upcoming diagnostic reconciliation must keep the original preservation
summary unchanged. For a relocated archive, require a regular location record
binding the original path, original size and original SHA-256 to the exact
owned data-drive destination. Recompute the archive hash at that destination;
do not accept the relocation helper's prose as a substitute. Report external
archive bytes plus local evidence against the same memory-lab evidence cap.
The historical missing Scan launcher remains unresolved, and strict closeout
remains unrun. No original failure is relabelled.

Archive/remove the relocation and metadata-repair failure trees with the same
unchanged complete-membership verifier. Run normal manual documentation checks
and the diagnostic reconciliation after the census is restored. The two direct
maintenance receipts are `fabric-work-5e426fbc62cb4da9aa1fd9fa051b2bea` (failed)
and `fabric-work-055f6b2357be41c09fe13822d5ef20bd` (passed). The relocation receipt
is `fabric-work-0ab04ffbb6084b1ea2ff4bd6b3a4debc` (failed).

All work retains the mounted data drive, 20 GiB laboratory/no-swap limits,
100 GB storage ceiling and existing remaining time. This is evidence maintenance,
not a new service measurement or changed acceptance gate.
