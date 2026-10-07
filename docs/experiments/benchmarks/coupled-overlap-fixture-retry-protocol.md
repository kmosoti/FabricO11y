# O8 full-Spool fixture correction

Controls01 exited101 because one6000-byte log line exceeded the existing4096-byte
source line limit. The source correctly skipped it and emitted a small gap; the
Spool never filled. Preserve that failed result. Replace it with three valid
2000-byte lines, asserting no source gaps, three lines and6000bytes exceeding the
configured journal allowance before the unchanged error/ACK/cursor assertions.
No runtime change or expected control outcome changes.

Run `catalog-coupled-o8-controls-02` with the original controls command,180s,
recovery stage,4MiB admission reserve. The frozen pilot supplement is registered
separately before execution. Freeze and pair commands require coupled_admit with
32MiB and8MiB reserves respectively; aggregate/query allocations remain unchanged.
Only the development pilot is initially admitted after O6/O7 and controls pass.
Small/profile continuation depends on actual evidence and mechanism results.
