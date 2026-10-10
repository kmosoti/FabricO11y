# Archive documentation snapshots before the final check

`catalog-range-closeout-01` preserved the interrupted run's twelve files exactly
and established unchanged product sources. Its manual profile failed because
the new driver copied Markdown snapshots into the live documentation tree:
their relative links resolved from the evidence directory. The checker correctly
reported 166 issues. Its own probes and hook tests exited 0. Preserve this failed
run and its receipts; change no checker or expected outcome.

Correct the evidence packaging by archiving all files in that run's
`final-documents` directory, comparing complete coverage and exact decoded bytes,
then removing only those loose snapshot copies. Originals remain in their normal
repository paths. Future snapshots use `.txt` suffixes. Record the mapping and
archive, with a 256 KiB decoded cap. Preserve/remove any remaining launcher-owned
temporary files for unit `fabric-work-805846b579ac4601b7f704c85b0a741b` after
confirming inactivity, with a 128 KiB cap. Keep failure receipts unchanged.

Register `catalog-range-closeout-02`, verification stage, 35 seconds, within the
remaining approximately 39 seconds of the existing 3660-second allocation.
No additional budget transfer or cap increase. Use the existing standalone
evidence-maintenance pattern: resource launcher, the coordinator lock, actual
campaign/stage/frontier admission, and `coupled_admit.observe` before/after. The
job writes its consumption into the same coordinator ledger. Reserve 512 KiB for
its driver/receipt/artifacts and reuse the immutable completed-fast source
archive instead of creating another full source copy near the evidence cap.
Capture the new driver/protocol, source hashes, final documentation diff, exact
command exits and cgroup observations. Read back every saved byte before cleanup.

Run all three unchanged manual documentation checks again. Recheck product Rust,
Cargo and oracle bytes against the completed fast archive, including a changed
byte negative control. Existing altered/missing/duplicate archive controls remain.
The original speed rejection, timeout and packaging failure remain recorded.
This repairs the evidence layout; it does not alter any product acceptance rule.
