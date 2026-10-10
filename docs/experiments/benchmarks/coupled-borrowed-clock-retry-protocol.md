# C4 counted clock retry

Status: registered before retry. Run 02 passed the rejected-row validation test
and built both plain binaries, then failed before measurement because the counted
phase observer referenced a missing `process_clock::thread_now`. Copy the existing
benchmark Linux CLOCK_THREAD_CPUTIME_ID helper, record process clock resolution,
and preserve run 02 and its archived binaries. The all-features examples check
`catalog-coupled-probe-check-01` exited 0 after this correction. No data shape,
metric, observer threshold, oracle or product behavior changes.

Add live aggregate/prospective-capacity inventory checks to the existing local
evidence guard, including failed prior datasets/archives. The initial projected
reservation is 40 MiB: smaller binary archives and the 4,096-row fixture project
at most 24 MiB retained plus the existing 16 MiB failure reserve. The hard local
48 MiB cap and prospective 832 MiB cumulative capacity cap remain. Before every
child the live guard additionally reserves failure/new-object bytes against those
global caps. If projections fail, stop admission; no cap is increased.

Fresh coordinator ID `catalog-coupled-c4-diagnostic-03`, dataset
`catalog-borrowed-attribution-run-03`. Previous command otherwise unchanged,
except `--reserve-mib 40 --capacity-reserve-mib 40`. Source/native/protocol identity
is frozen before measurements; all earlier outcomes remain failed.
