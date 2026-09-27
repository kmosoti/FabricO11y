# Stage 3 generator library ablation

## Status

Method registered on 2026-09-26 before running either comparison binary. Both release binaries completed with exit status `0`. A Python 3 wrapper ran them in the registered order, parsed their CSV output, checked the counters below, and added `variant,run` columns to the [raw trial file](generator-library-stage3.csv).

## Question and decision rule

How much does the local `fake` plus ChaCha8 generator change the end-to-end cost of the Stage 3 buffered workload relative to the archived hand-written recurrence? The hypothesis is that the general-purpose sampler may cost more per event. Report the median elapsed nanoseconds per event for each variant and batch size. If the library variant is more than 25% slower at batch size `256`, investigate the cost before drawing a performance conclusion. There is no production traffic model or throughput target, so this probe does not accept or reject the dependency on speed alone.

## Fixed contract and variable

Both variants use the same [event types](../../../src/lib.rs), [buffer](../../../src/buffer.rs), and [probe](../../../examples/batch_probe.rs). Each produces `500,000` owned Gauge events with seed `42`, sequential IDs, fixed synthetic times, one `service.name=checkout` attribute, source `42`, resource `9001`, tenants in `1..=4`, and quarter-millisecond durations in `0..250` ms. The variants generate different tenant and duration sequences; matching semantic ranges, not identical bytes, is the comparison contract.

Only the generator implementation and its dependency resolution change: the [archived recurrence](fixtures/stage3_lcg_generator.rs) and [old manifest](fixtures/stage3_Cargo.toml) versus the current [`fake` adapter](../../../src/generator.rs) and [manifest](../../../Cargo.toml). This is a package-level comparison. It cannot isolate the sampler from different RNG algorithms or distribution details.

For each variant, the existing probe runs batch sizes `1`, `8`, `64`, and `256`, with logical capacity `256`. It rotates size order over three repetitions. Run each binary three times in alternating variant order, yielding nine measurements per variant and size. Verify `500,000` consumed events, identical fingerprints across sizes and repetitions *within each variant*, and occupancy at most `256`. The rolling fingerprint is a regression signal, not a collision-free proof.

## Metric and timing boundary

The timer starts before creating the generator and stops after the final batch is consumed. It includes event construction, allocations, buffering, batch creation, and consumer inspection. It excludes compilation, process startup, and printing. Record every raw elapsed nanosecond count, batch count, full rejection count, maximum logical occupancy, and fingerprint. Derive `ns/event = elapsed_ns / 500000` and the median of nine. Record the toolchain, machine, source hashes, and raw CSV. Do not compare these medians as though they were a generator-only microbenchmark, tail latency, peak memory, or throughput under real load.

## Reproduction procedure

From the repository root, create a temporary copy for the archived implementation, compile both release examples with the lockfiles, and run the binaries in alternating order:

```sh
probe_dir=$(mktemp -d)
mkdir -p "$probe_dir/src" "$probe_dir/examples"
cp docs/experiments/benchmarks/fixtures/stage3_Cargo.toml "$probe_dir/Cargo.toml"
cp docs/experiments/benchmarks/fixtures/stage3_Cargo.lock "$probe_dir/Cargo.lock"
cp src/lib.rs src/buffer.rs "$probe_dir/src/"
cp docs/experiments/benchmarks/fixtures/stage3_lcg_generator.rs "$probe_dir/src/generator.rs"
cp examples/batch_probe.rs "$probe_dir/examples/"
CARGO_TARGET_DIR="$probe_dir/target" cargo build --release --offline --locked --manifest-path "$probe_dir/Cargo.toml" --example batch_probe
cargo build --release --offline --locked --example batch_probe
"$probe_dir/target/release/examples/batch_probe"
target/release/examples/batch_probe
```

Repeat the last two binary commands three times in the orders `old, fake`; `fake, old`; `old, fake`. Preserve all output with a `variant,run` prefix in [raw CSV](generator-library-stage3.csv). The historical [batch-size probe](batch-size-stage3.md) used an earlier single run; it is preserved independently and is not pooled with this ablation.

The measured build used the default Cargo release profile, Rust `1.98.0` (`88d9e12ae`, LLVM `22.1.8`), Cargo `1.98.0`, Linux `x86_64`, and an Intel Core i7-10750H @ 2.60 GHz. The working tree contained uncommitted project work, so these SHA-256 hashes identify the compared code:

```text
53985ad4df28518291187387d297e79a7b8be4f58aaebf7717ed4b8dd5e4581c  src/lib.rs (both)
bac64bc8f2ca50acb620a63c73a93c159e83be02a1248293a93addf56ea96f29  src/buffer.rs (both)
9c1a3b62f96fa109963b8d94c2a21496f253ba67c247b87cca98435631533758  examples/batch_probe.rs (both)
249f06f32d8e81af2776f2dea03cbd2b33c70f374450317268f6e9eba24edbb1  archived src/generator.rs
fde30063c33ebacd8dc49185be8616c6580f5e7936568f57f7f91bf5169f0d7b  archived Cargo.toml
36ff241c1e08c39519e3e42ce49a7d1792d4fa8da764a303bb70f07bb3c02c80  archived Cargo.lock
bf6fc61639574e3fe1ae75e904798a0c1a8cdbcf283e46091b46a1fa5f0053c7  current src/generator.rs
4aad64233e53f3430f8ee401e773e8c76193769f250ab61c9fa6a20965ba6319  current Cargo.toml
3479bf548db4ad2bdaf3488e088e8f540e480478a8db407778b8860d4a9b468f  current Cargo.lock
```

## Results

The [raw CSV](generator-library-stage3.csv) contains all 72 trials: nine per variant and size. All trials consumed `500,000` events, reached maximum logical occupancy `256`, and reported the expected batch and rejection counts for their size. Every old-generator trial had fingerprint `5669176c880f3a3e`; every `fake` trial had fingerprint `c31dcb3506fbab41`. Different fingerprints are expected because the generated values differ.

| Batch size | Old median ns/event | `fake` median ns/event | Observed median change |
| ---: | ---: | ---: | ---: |
| 1 | 102.01 | 110.13 | 7.96% higher |
| 8 | 97.97 | 105.69 | 7.88% higher |
| 64 | 104.31 | 109.13 | 4.62% higher |
| 256 | 100.09 | 111.94 | 11.84% higher |

At size `256`, the registered 25% investigation threshold was not crossed. Within the `fake` trials, size `8` had a median 4.04% lower than size `1`; the other sizes did not show a large improvement. These comparisons do not select a batch size.

## Interpretation and limits

The dependency-backed version was slower by the observed median on this machine and workload, but the full ranges overlap. At size `256`, old trials spanned about `96.8–111.3` ns/event and `fake` trials about `100.5–153.5` ns/event. The latter includes a high outlier. This short run cannot quantify a stable speed penalty or prove why it occurred. Different RNG algorithms, range sampling, and generated values changed together; no component was isolated. Peak memory, allocation counts, event-size distribution, tail latency, and energy were not measured. The fixed service and Gauge shape are not representative production traffic. The result supports continuing with the dependency for the learning workload while retaining a measurable baseline; a future performance budget and representative trace would be needed for a production load-generator choice.
