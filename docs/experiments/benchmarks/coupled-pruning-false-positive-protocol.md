# O7 empty-window discriminator and C5 output control

Native02 passed12 full chains and four injected defects. Its two empty windows
were outside all bounds, so it did not exercise the planned false-positive empty
window. Preserve that scoped pass and limitation. The follow-up adds[T+2,T+3):
no producer row has that time, but the first actual group's[T+1,T+5] bound admits
it. Require a nonzero footer-admitted cardinality with zero matches. This is still
footer selection, not measured query IO. All earlier1100-row/35-Group semantics
and independent controls remain fixed; expected distinct windows7/chains14.

Run the same source fixture at compile-time run caps16,8,32MiB under fresh
`catalog-coupled-o7-r<VALUE>-build-01` and `catalog-coupled-o7-r<VALUE>-01` jobs,
using the original contained build and native commands. Build reservation4MiB,
timeout180s; native reservation12MiB, timeout180s, driver argument
`--run-mib <VALUE>` and output `catalog-coupled-o7-r<VALUE>-01`. The actual native
reported selector must match the explicit argument. Each local20MiB cap retains
8MiB failure headroom; require existing aggregate reserve before every job.

Compare all three manifest file entries (sizes, row counts, hashes) with16MiB
reference after independent oracle/control checks pass. Equal bytes across these
bounded-sort variants qualify only this log fixture's output-preservation control;
they do not recertify old reference-vs-bounded C2 or all metrics/spans workloads.
A disagreement stops C5 admission. Diagnostic/protocol amendments are separately
committed before these new runs; prior outcomes are not overwritten.
