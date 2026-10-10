# Evidence allocation within the unchanged sweep envelope

Prospective supplement to the [matched sweep](cross-system-sweep-protocol.md).
The completed Vortex comparison retains its actual format outputs, including
four approximately 22 MB wide-body files. The first failed query install also
retains a complete 109 MiB compressed dependency/fixture tree. Their sizes are
observations, not reasons to delete failed evidence or truncate native outputs.

For subsequent jobs, move 256 MiB of the source category's unused allowance to
query: source 768 MiB, query 512 MiB, memory 256 MiB, operations 384 MiB and
coordinator 128 MiB. The aggregate remains **2048 MiB**. All three repaired source
archives already fit the reduced source allowance; no further download is
admitted here. Historical receipts retain the allocations they used. Preserve
the original protocol unchanged because frozen storage builds bind its hash.
The coordinator records this supplement's hash for subsequent jobs.

The [failed-tree preserver](../../../tools/bench/labs/cross_system/sweep_preserve.py)
may now reserve at most 192 MiB for one complete compressed archive, within the
same 512 MiB decoded-tree ceiling and the destination category's current cap.
It still verifies every original member twice, rechecks the original inventory
before removal, rejects unsafe links and stops retaining the original if complete
preservation fails. This admits the inactive 364 MiB Vortex failure tree without
claiming that its unknown compressed size will fit. Keep any failed attempt and
partial archive if it does not fit. The old 128 MiB preservation receipts remain
unchanged.

This is an internal evidence-storage reallocation, not an increase to the owner's
resource allowance: cgroup 16/20 GiB memory high/max, zero swap, per-service
deadline, 7200-second execution allocation, 8 GiB active scratch and 16 GiB free
disk reserve stay unchanged. Actual peaks, disk use and cleanup are reported in
the final run receipt. No workload, correctness oracle or performance threshold
changes.
