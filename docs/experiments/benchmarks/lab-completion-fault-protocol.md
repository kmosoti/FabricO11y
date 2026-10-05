# Owned builder syscall fault collection

Registered before execution under [R1 campaign scope](lab-completion-protocol.md).
Use the faithful steady16MiB journal, frozen input digest, and a private helper
calling the public bounded builder and startup cleanup. A private LD_PRELOAD
interposer matches only absolute paths inside the owned fixture directory.
No shared filesystem is filled. GCC builds the interposer inside containment.

Inject ENOSPC at first spill, raw table, logs, metrics, optional filter and
manifest writes; EIO on first spill-run read; ENOSPC at log-table and manifest
sync, publication rename and final parent-directory sync. Kill the actual owned
child at first spill write, manifest sync and pre-publication rename. Every
cell must record an actual injection marker and nonzero child exit. A clean
control must succeed; a nonexistent-path negative control must produce no
marker, so that lack of injection cannot count as evidence. Preserve both.

Before retry require the input digest unchanged; after removing the injector,
run the same startup cleanup and retry against the same state. Require exact
manifest equality to a clean build, authenticated file verification and no
remaining building directory. Keep per-case commands, exit status, injection
trace and pre-restart file layout, retaining owned state on failure. This is
builder stage evidence with a differential manifest oracle; the separate
storage tests supply independent full-query grading and publication/reclaim
recovery. It is not physical power-loss evidence, exhaustive partial-IO evidence
or checkpoint-fault coverage. Run existing checkpoint/reclaim regressions too.
Frozen acceptance and independent oracles remain unchanged.

Before first execution, extend the cases with sealed-journal EIO and a successful
short (at most16-byte) spill write or merge-run read followed by EIO on the same
descriptor. Require nonzero failure and exact retry as above. This covers these
representative partial-IO paths, not every partial syscall boundary.
