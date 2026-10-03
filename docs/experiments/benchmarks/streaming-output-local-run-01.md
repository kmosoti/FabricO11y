# Streaming Segment output: local run 01

Status: **pilot complete; registered heap-reduction target not met**. All 12 paired trials produced byte-identical Segment directories. The candidate remains experimental on `milestone/streaming-segment-output`; it is not merged or a bounded-sealer completion. This is route R0 of the [experiment design](sealing-pipeline-experiment-design.md), under the [registered protocol](streaming-output-local-protocol.md).

## Result and decision

The streaming writer removes its whole-file encoded-output `Vec`, uses a 256 KiB file buffer, hashes accepted bytes incrementally, and flushes before the existing file sync. Arrow arrays, decoded Groups, copied Entries and all projected row vectors still exist for the whole file. Publication, compression, row groups, manifests and reclaim semantics are unchanged.

Values below are medians of three independent processes per variant/cell. Time ratios are medians of paired ratios, not ratios of the displayed medians. Heap is incremental live allocation above the already-created input fixture; RSS covers the whole process, including input generation and later verification.

| Shape / encoded Group target | Incremental peak heap, baseline → candidate MiB | Whole-process peak RSS, baseline → candidate MiB | Candidate/baseline instrumented build time |
| --- | --- | --- | --- |
| repeat, 16 MiB | 66.56 → 66.81 | 97.14 → 96.85 | 0.951 |
| repeat, 64 MiB | 242.72 → 242.97 | 343.95 → 343.96 | 0.970 |
| entropy, 16 MiB | 80.77 → 77.65 | 109.71 → 100.79 | 0.969 |
| entropy, 64 MiB | 313.20 → 291.34 | 420.28 → 368.87 | 0.966 |

Primary cell: high entropy, 64 MiB. The median paired heap ratio was **0.9302** (about 7.0% lower), which fails the predeclared >10% reduction target. Its RSS ratio was **0.8775** (about 12.3% lower). RSS does not replace the primary heap metric after seeing results. The time ratios met the pilot's median <=1.10 guard in every cell, but three instrumented trials establish neither statistical significance nor uninstrumented/end-to-end speed. The primary paired time ratios ranged from about 0.873 to 1.033.

Repeated bodies gain essentially no peak memory: the encoded file was already small, while the candidate adds a 256 KiB buffer (about 0.25 MiB extra live heap). That contrast supports the expected mechanism. The remaining high-entropy incremental peak is about 291 MiB for 64 MiB of Groups; the writer change does not approach the complete bounded-builder's 80 MiB finite gate.

Decision: retain R0 as measured experimental evidence, not a demonstrated solution to the RAM problem. The next high-value route is bounding decoded/projected data and Arrow/writer working sets with exact-output checks, before adding adaptive concurrency. These trials do not establish which remaining allocation is individually dominant; stage-specific attribution is still needed. Do not claim the full factory is faster, or that more concurrent builders now fit safely.

## Reproduction and evidence

Original source base: `6835d0d1ce978a1cdb1b6c4a97dd301d955616a8`. Registration/local commits: `d82f09b`, corrected before any measurement by `c68a0ff`. Baseline binary was compiled from `c68a0ff`; candidate from `9406469`. Their equal-tree published equivalents are registration [`e83f39e`](https://github.com/kmosoti/FabricO11y/commit/e83f39ecf80c9828acec5e3304fd117f0790113e), baseline/harness [`e08cc0c`](https://github.com/kmosoti/FabricO11y/commit/e08cc0c2757beee8193f3e523a28ab310a88d1a7), and candidate [`5b41254`](https://github.com/kmosoti/FabricO11y/commit/5b412542f726d5a17f70eb8d9b456f512697e8c9). Local/published trees were compared before aligning the branch; the original local SHAs remain in verification receipts. The exact runtime-source, fixture and runner SHA-256 values are in [environment.json](data/streaming-output-local-run-01/environment.json); immutable binary SHA-256 values are in [metadata.json](data/streaming-output-local-run-01/metadata.json).

Commands used the toolchain environment with two build jobs; binaries were built with `cargo build --release --locked -p fabric-server --example seal_output_probe`, copied to distinct scratch paths, then executed with:

```sh
python3 tools/bench/run_seal_output.py /workspace/scratch/stream-output-binaries/baseline /workspace/scratch/stream-output-binaries/candidate /workspace/scratch/stream-output-run-01
```

Exit 0; [completion receipt](data/streaming-output-local-run-01/complete.json). Every trial's command, exit, input hash, measurements, manifest and output-file hashes are in [pairs.json](data/streaming-output-local-run-01/pairs.json). All paired complete output directories matched byte for byte and passed production manifest-hash verification; this controls the sink change, while existing query tests provide the independent semantic oracle. The comparator accepted identical bytes and rejected its deliberate one-byte corruption ([control](data/streaming-output-local-run-01/negative-control.json)). Paired payload directories were removed only after comparison; measurements/hashes remain.

The initial fixture compile failed because vendored OTLP `KeyValue` requires `key_strindex`. Supplying its default corrected setup before any trial. GNU time was absent, so `wait4` supplied native per-child CPU/RSS accounting, recorded in the pre-trial protocol correction. No workload or target was relaxed after measurement.

## Constrained-host interpretation

Host: Linux x86_64, glibc 2.41, kernel 6.18.44, quota four CPU equivalents, five visible affinity CPUs, 16 GiB cgroup memory ceiling, overlayfs. Each measured child was pinned to two allowed CPUs and enforced a 2 GiB address-space limit plus 180 s CPU/wall limits. This limits virtual address space, not resident memory or a two-CPU allocation claim. Sequential single-builder processes all completed; no OOM, timeout or output mismatch occurred.

About 25 GiB workspace disk remained free. The shared build cache reached about 4.0 GiB. Retained pilot scratch evidence was about 224 KiB before compact copying. These are bounded native experiments suitable for this environment. They do not measure live ingress, Spindle spool behavior, ACK p99, concurrent queries, physical durability, steady-state server memory or real-disk throughput. Inputs were generated in memory, cache was not flushed, and allocation accounting was active during timing. The filesystem/host and possible co-tenancy remain part of the result.

## Checks

Focused command `cargo test --locked -p fabric-server --lib output_tests` exited 0: three tests covering partial writes/hash/length, flush errors, and byte-identical Parquet across a row-group boundary. [Output](data/streaming-output-local-run-01/tests.txt). `timeout 900 cargo test --locked -p fabric-server --lib --tests` exited 0: **30 tests** (16 library, one config, four delivery, nine history), including independent query-oracle coverage and existing crash-state checks. [Server output](data/streaming-output-local-run-01/server-tests.txt).

`timeout 900 cargo xtask checks --profile fast` exited **1**: **19 passed, one failed**. The failure is Clippy `chunks_exact_to_as_chunks` at unchanged `crates/fabric-observation/src/crc32.rs:81`, previously reproduced at the original baseline in [journal-reclaim run 01](journal-reclaim-local-run-01.md). This source is unchanged through both writer variants. No lint or gate was weakened. [Runner output](data/streaming-output-local-run-01/fast.txt); [workspace-test receipt](data/streaming-output-local-run-01/receipts/test.json); all 20 receipts are alongside it. Runtime source stayed fixed; documentation edits during the check were validated again afterward. Stored text logs trim trailing whitespace; receipt JSON and paired trial values are unchanged.

Final documentation check `bun tools/docs/check.mjs` and `git diff --check` exited 0. No solver, controller experiment, allocation bound proof or target-PC measurement is claimed.
