#!/usr/bin/env bash
# Run the registered Stage 6 local-log workload. See docs/experiments/benchmarks/local-log-stage6.md.
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: bash tools/bench/run_local_log_stage6.sh <NEW_OUTPUT_DIRECTORY>" >&2
    exit 2
fi

output_dir=$1
if [[ -e "$output_dir" ]]; then
    echo "output directory already exists: $output_dir" >&2
    exit 1
fi
mkdir -p "$output_dir"

cargo build --release --offline --locked --example local_log_probe
probe=target/release/examples/local_log_probe

for trial in warmup 1 2 3 4 5; do
    path="$output_dir/trial-$trial.fol2"
    "$probe" write "$path" 42 2000 > "$output_dir/write-$trial.csv"
    "$probe" verify "$path" 42 2000 > "$output_dir/verify-$trial.csv"
done

"$probe" write "$output_dir/recovery-0.fol2" 42 0 > "$output_dir/write-recovery-0.csv"
"$probe" verify "$output_dir/recovery-0.fol2" 42 0 > "$output_dir/verify-recovery-0.csv"
"$probe" write "$output_dir/recovery-500.fol2" 42 500 > "$output_dir/write-recovery-500.csv"
"$probe" verify "$output_dir/recovery-500.fol2" 42 500 > "$output_dir/verify-recovery-500.csv"

cargo build --release --offline --locked --features stage6-alloc-probe --example local_log_probe
"$probe" write "$output_dir/alloc-2000.fol2" 42 2000 > "$output_dir/write-alloc-2000.csv"
"$probe" verify "$output_dir/alloc-2000.fol2" 42 2000 > "$output_dir/verify-alloc-2000.csv"

(
    cd "$output_dir"
    sha256sum -- *.csv *.fol2 > SHA256SUMS
)
{
    date -u
    uname -a
    rustc --version
    cargo --version
    lscpu | rg 'Architecture:|Model name:|CPU\(s\):|Hypervisor vendor:'
    findmnt -T . -o SOURCE,FSTYPE,OPTIONS -n
    printf 'RUSTFLAGS=%s\n' "${RUSTFLAGS-<unset>}"
    git rev-parse HEAD
    git status --short
    sha256sum Cargo.toml Cargo.lock src/generator.rs src/buffer.rs src/log.rs examples/local_log_probe.rs
} > "$output_dir/environment.txt"

echo "Stage 6 raw results: $output_dir"
