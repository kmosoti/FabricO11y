# Streaming Segment output: local R0 pilot protocol

Status: **registered before candidate implementation or timing trials**. This executes one route from the [pipeline experiment design](sealing-pipeline-experiment-design.md) on a constrained cloud host. It does not implement adaptive scheduling or the complete bounded builder. This protocol and its fixture/runner are committed separately before production edits.

## Question and boundary

Baseline source: `6835d0d1ce978a1cdb1b6c4a97dd301d955616a8`. Baseline plus the fixture in this registration is compiled before production modification. Candidate changes only the table writer sink: replace full-file encoded output in a `Vec` with bounded buffered file writes and incremental hashing. Keep Arrow conversion, row ordering/groups, codecs, Segment files, manifest, sync sequence, reclaim and runtime concurrency the same. No change to ingest or query execution is being timed.

**H1:** on the primary high-entropy 64 MiB fixture, streaming output reduces incremental live-heap peak by more than 10%, with byte-identical output and no more than 10% median instrumented build-time regression. **H0:** the heap ratio is >=0.90, or correctness/time guardrails reject it. Other cells reveal size/compressibility sensitivity; they do not replace a failing primary cell. This is an exploratory paired pilot of three seeds per cell, not a powered confirmatory or end-to-end ACK-latency experiment. Ratios and every outcome are reported; no statistical rejection of the population null is inferred from three trials.

## Fixed workloads and metrics

The native [`seal_output_probe`](../../../crates/fabric-server/examples/seal_output_probe.rs) generates deterministic valid Batch envelopes, Groups and projected logs, metrics, spans and gaps. Sixteen node identities, 64 logs per Batch, 1,024-byte bodies, two string attributes per log, one gauge and span plus a gap per Batch. Observation times are deliberately out of order. Shapes: identical repeated bodies and deterministic high-entropy printable bodies. Seeds: 2703163393, 2703163394, 2703163395. Target total Group protobuf bytes: 16 and 64 MiB, stopping at the first whole Group at or above the target. It is a builder fixture, not a claim of valid contiguous per-Strand delivery histories or journal framing overhead.

Input hash covers each Group's length followed by exact encoded bytes; record fixture hashes, actual bytes and row counts. All fixture Groups stay in memory before the measured build. Reset the allocator peak immediately before `segment::build`; record incremental peak above this resident-input baseline, total live-heap peak and post-build live heap. Record build wall time through final Segment publication/sync. Verification happens afterward. `/usr/bin/time` measures whole-process CPU and peak RSS, including fixture creation and verification; those boundaries differ from the build-only interval.

The counting allocator wraps System, counts successful alloc/zeroed/realloc/dealloc capacities, and is identical in both binaries. All timing is instrumented, exploratory timing; no uninstrumented speedup is claimed. Linux RSS includes allocator effects and is not interchangeable with the live-heap counter. No cache flushing is performed. Report the actual filesystem and CPU contention limits rather than labeling the runs cold disk measurements.

Every pair must have equal input hashes and byte-identical complete Segment directories, including manifest and filters. Independently recompute manifest file digests through `segment::verify`. The Python comparator must accept identical files and reject an injected one-byte corruption before trials. Existing independent query/delivery tests remain mandatory for the candidate. Dedicated writer tests cover partial writes, failing writes/flush and the hash/length accounting boundary where applicable.

## Execution and budgets

Host preflight: Linux x86_64, cgroup quota four CPU equivalents, 16 GiB memory ceiling, five affinity CPUs, overlayfs workspace and about 25 GiB free. These are observations, not qualification. Runner pins each process to the first two allowed CPUs and limits its address space to **2 GiB**, CPU time to **180 s**, and wall time to **180 s**. Address-space enforcement is not an RSS/cgroup guarantee. One child at a time; one Segment per child. Fixture generation is inside those process bounds.

At most 12 pairs/24 trials, alternating baseline/candidate order by pair. A fresh owned state directory per trial, refuse existing paths, delete only paired state directories after comparison, preserve compact measurements and hashes. A single pair is below 5 GiB live data under the declared fixtures; stop if the free-disk floor is below 4 GiB. Evidence <=50 MiB and campaign <=30 minutes (checked between pairs, plus the per-child watchdog). No service installation, Docker, whole-disk exhaustion or privileged faults. Build/cache storage is outside the disposable data ceiling and is recorded separately.

Rust 1.98.0, locked dependencies, release profile, two build jobs. Cache may be shared; immutable baseline and candidate binaries are copied to separate paths and hashed. Commands, with the existing toolchain environment configured:

```sh
cargo build --release --locked -p fabric-server --example seal_output_probe
# Preserve baseline binary before editing production source; rebuild/copy candidate afterward.
python3 tools/bench/run_seal_output.py BASELINE_BINARY CANDIDATE_BINARY NEW_RESULT_DIRECTORY
cargo test --locked -p fabric-server --lib --tests
cargo xtask checks --profile fast
```

Compile time is not included in the trial budget. Compilation or fixture failures are recorded and fixed without silently changing workload semantics; any required protocol change is recorded before its affected measurement. Freeze source/binary/harness hashes in the result. Performance confirmation on the native server under offered traffic and PC reproduction are subsequent work. A passing writer pilot does not establish an overall bounded sealer, ACK-latency improvement, or a production memory ceiling.

## Results

Not run at registration. Keep the experiment's null result if this removes too little peak memory or costs too much time. Do not expand scope to an entire pipeline until the result is interpretable.
