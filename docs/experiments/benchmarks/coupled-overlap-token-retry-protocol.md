# O8 fixture credential correction

Pair1-02 again stopped before measurement: its fixture admin token was shorter
than the existing32-byte minimum. Preserve that result. Use a fixed46-byte
printable fixture credential, retaining actual credential validation and all
workload/binary/oracle settings. Register pair1-03 with the same180s deadline,
8MiB reserve, freeze01 and fresh catalog-overlap-pair1-03 output. Preserve pair1-02
first using catalog-coupled-failure-cleanup-04,120s verification stage and the
catalog cleanup helper. These are harness failures, not server performance data.
