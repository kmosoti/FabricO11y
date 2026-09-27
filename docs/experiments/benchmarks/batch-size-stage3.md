# Stage 3 batch-size probe

This is a historical measurement of the archived hand-written generator. The current generator uses `fake`; running the probe against the current checkout produces different values and timings. The [generator library ablation](generator-library-stage3.md) measures both variants with a recorded procedure.

## Status

Method registered before execution on 2026-09-26. The runnable [probe](../../../examples/batch_probe.rs) completed with exit status `0`. A pilot run preceded a source-only Clippy annotation; the recorded run below used the final source hash and is the basis for interpretation. The pilot is not pooled with these three repetitions per size.

## Hypothesis and decision rule

Larger batches may reduce per-event batching and consumer overhead in this local pipeline. The baseline is batch size `1`. Flag a size for later investigation only if its median elapsed nanoseconds per event is at least 10% lower than the baseline and all correctness counters match. This exploratory probe does not choose a production batch size or storage strategy.

## Setup and fixed contract

- The [archived generator](fixtures/stage3_lcg_generator.rs) creates `500,000` events with seed `42` for every trial.
- The [buffer](../../../src/buffer.rs) has logical capacity `256`. A full push returns the event; the caller drains a batch and retries it. The final partial batch is consumed.
- Every event must be consumed exactly once in source order. The probe checks count, a rolling order-sensitive fingerprint, and occupancy at most `256`. The fingerprint is a quick regression signal, not a collision-free proof.
- The consumer does not print events. It inspects ID, tenant, and gauge value, then drops the owned event. `black_box` keeps the batch materialized for the measurement.

## Variable and measurement

Only batch size varies: `1`, `8`, `64`, `256`. Run three repetitions per size, rotating order each repetition. The timer starts before generator creation and stops after the final batch is consumed. It includes event construction, heap allocations, buffer operations, batch creation, and consumer work. It excludes compilation, process startup, and result printing.

Record raw elapsed nanoseconds, batches, full rejections, maximum logical occupancy, and fingerprint. Derive nanoseconds per event as `elapsed_ns / 500000` and report the median of three repetitions. This measures end-to-end work in one process; it does not isolate queue cost, tail latency, peak bytes, or persistence.

## Reproduce

Use the archived [generator](fixtures/stage3_lcg_generator.rs), [manifest](fixtures/stage3_Cargo.toml), and [lockfile](fixtures/stage3_Cargo.lock) in the temporary crate described in the [ablation reproduction procedure](generator-library-stage3.md#reproduction-procedure), then run its `batch_probe` binary. Running `cargo run --release --offline --example batch_probe` in the current checkout measures the `fake` generator instead.

Build settings: Cargo release profile defaults in this repository; no external Rust dependencies. Execution environment: Rust `1.98.0` (`88d9e12ae`, LLVM `22.1.8`), Cargo `1.98.0`, Linux `x86_64`, Intel Core i7-10750H @ 2.60 GHz. `git` HEAD was `39664057b9bbaec02c36a845b611e005f6cfeaad`, but the working tree contained uncommitted project work. The relevant SHA-256 source hashes below identify the measured code more precisely:

```text
53985ad4df28518291187387d297e79a7b8be4f58aaebf7717ed4b8dd5e4581c  src/lib.rs
bac64bc8f2ca50acb620a63c73a93c159e83be02a1248293a93addf56ea96f29  src/buffer.rs
249f06f32d8e81af2776f2dea03cbd2b33c70f374450317268f6e9eba24edbb1  src/generator.rs
9c1a3b62f96fa109963b8d94c2a21496f253ba67c247b87cca98435631533758  examples/batch_probe.rs
fde30063c33ebacd8dc49185be8616c6580f5e7936568f57f7f91bf5169f0d7b  Cargo.toml
```

## Results

Raw probe output, in execution order:

```csv
repeat,batch_size,events,elapsed_ns,batches,full_rejections,max_occupancy,fingerprint
0,1,500000,55219300,500000,499744,256,5669176c880f3a3e
0,8,500000,51643600,62500,62468,256,5669176c880f3a3e
0,64,500000,68604900,7813,7809,256,5669176c880f3a3e
0,256,500000,64237101,1954,1953,256,5669176c880f3a3e
1,8,500000,49997800,62500,62468,256,5669176c880f3a3e
1,64,500000,52563100,7813,7809,256,5669176c880f3a3e
1,256,500000,53023700,1954,1953,256,5669176c880f3a3e
1,1,500000,53056900,500000,499744,256,5669176c880f3a3e
2,64,500000,55748801,7813,7809,256,5669176c880f3a3e
2,256,500000,52211400,1954,1953,256,5669176c880f3a3e
2,1,500000,53402900,500000,499744,256,5669176c880f3a3e
2,8,500000,51188900,62500,62468,256,5669176c880f3a3e
```

| Batch size | Median elapsed ns | Median ns/event | Change versus size 1 |
| ---: | ---: | ---: | ---: |
| 1 | 53,402,900 | 106.81 | baseline |
| 8 | 51,188,900 | 102.38 | 4.15% lower |
| 64 | 55,748,801 | 111.50 | 4.39% higher |
| 256 | 53,023,700 | 106.05 | 0.71% lower |

## Interpretation and limits

All twelve trials reported `500,000` consumed events, the same fingerprint, and maximum logical occupancy `256`. No size met the preregistered 10% improvement threshold, so this round selects no batch-size winner. The elapsed time includes generator allocations and consumer work, which can mask queue effects. The size-64 trials span about 52.6–68.6 ms, a spread larger than the decision threshold. These are short runs on one machine with possible background activity; they do not establish latency, peak memory, production throughput, or a universal ordering of batch sizes.
