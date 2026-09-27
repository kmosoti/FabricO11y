# M2 unscheduled-prefix raw run 01

The [registered experiment and result](../../unscheduled-prefix-m2-run-01.md) explains the workload, checks, gates, interpretation, and limits. `bash tools/bench/run_transport_m2.sh target/transport-m2/run-01` exited 0 on 2026-09-27 UTC. [environment.txt](environment.txt) records the exact command, host, toolchain, and source hashes. Its experiment-document hash is the registered version before results were added.

- [summary.csv](summary.csv): 40 profile/seed/variant aggregate rows.
- [bursts.csv](bursts.csv): 40,000 burst-peak rows.
- [messages.csv.gz](messages.csv.gz): deterministic gzip of all 800,000 message rows; no rows were removed.
- [analysis.json](analysis.json): independent CSV validation, per-seed measurements, and registered gates.
- [SHA256SUMS](SHA256SUMS): hashes of the original uncompressed files in `target/transport-m2/run-01`.
- [PRESERVED_SHA256SUMS](PRESERVED_SHA256SUMS): hashes of the files stored in this directory.

`gzip -t messages.csv.gz` exited 0. The SHA256 of its decompressed bytes was `6f03373063204d1bcb0383130c97c7ee094f15880dfbe2b32290c01189837a07`, exactly matching the original `messages.csv`. The preserved file hashes can be checked from this directory with `sha256sum -c PRESERVED_SHA256SUMS`.

To rerun the analyzer from the repository root, use a fresh directory:

```sh
mkdir -p target/transport-m2/recheck
cp docs/experiments/ablation/data/unscheduled-prefix-m2-run-01/{summary.csv,bursts.csv} target/transport-m2/recheck/
gzip -dc docs/experiments/ablation/data/unscheduled-prefix-m2-run-01/messages.csv.gz > target/transport-m2/recheck/messages.csv
python3 -B tools/bench/summarize_transport_m2.py target/transport-m2/recheck > target/transport-m2/recheck/analysis.json
```

The analyzer reads the preserved [H1 M1 message rows](../receiver-credit-h1-run-01/messages.csv.gz) as its regression reference. It validates the registered trace, message timings, counts, caps, control-byte accounting, burst peaks, and decision gates. It cannot recover every per-tick queue state or individual grant from these CSV files.
