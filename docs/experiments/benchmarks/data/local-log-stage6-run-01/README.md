# Stage 6 raw run 01

The CSV files are the raw output of the [registered local-log baseline](../../local-log-stage6.md), run on 2026-09-26 local time (2026-09-27 00:32 UTC). `write-1.csv` through `write-5.csv` and matching `verify-*.csv` are the five measured trials. `write-warmup.csv`, `write-recovery-{0,500}.csv`, and `write-alloc-2000.csv` are separate runs. [summary.json](summary.json) is derived by [the repository analyzer](../../../../../tools/bench/summarize_local_log_stage6.py).

[environment.txt](environment.txt) records host details, Git state, and hashes of the relevant source files. [CSV_SHA256SUMS](CSV_SHA256SUMS) checks the copied CSV files. [SHA256SUMS](SHA256SUMS) also records hashes of the generated `.fol2` logs, which remain under ignored `target/stage6/run-01/`; the logs are reproducible from the seed and current codec and are not included here.

From this directory, `sha256sum -c CSV_SHA256SUMS` passed with exit 0. The analyzer accepted the original run and exited 1 after one append row was removed from a temporary copy, so its row-count gate can reject a missing sample.
