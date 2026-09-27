#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    printf 'usage: %s FRESH_OUTPUT_DIRECTORY\n' "$0" >&2
    exit 2
fi

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo_root"
output_dir=$1
if [[ -e "$output_dir" ]]; then
    printf 'output directory already exists: %s\n' "$output_dir" >&2
    exit 2
fi
mkdir -p -- "$(dirname -- "$output_dir")"

cargo test --offline --locked --manifest-path tools/transport-sim/Cargo.toml
cargo run --release --offline --locked --manifest-path tools/transport-sim/Cargo.toml -- h1 "$output_dir"
python3 -B tools/bench/summarize_transport_h1.py "$output_dir" > "$output_dir/analysis.json"

{
    printf 'UTC run completion: '
    date -u '+%Y-%m-%d %H:%M:%S UTC'
    printf 'Command: bash tools/bench/run_transport_h1.sh %s\n' "$output_dir"
    printf 'Git HEAD: '
    git rev-parse HEAD
    printf 'Rust: '
    rustc --version
    printf 'Cargo: '
    cargo --version
    printf 'Kernel: '
    uname -a
    printf 'RUSTFLAGS: %s\n' "${RUSTFLAGS-<unset>}"
    printf '\nCPU:\n'
    lscpu | sed -n '/^Model name:/p;/^CPU(s):/p'
    printf '\nSource hashes:\n'
    sha256sum tools/transport-sim/Cargo.toml tools/transport-sim/Cargo.lock \
        tools/transport-sim/src/lib.rs tools/transport-sim/src/main.rs \
        tools/bench/summarize_transport_h1.py \
        docs/experiments/ablation/receiver-credit-h1-run-01.md \
        formal/transport/TransportOwnership.tla
} > "$output_dir/environment.txt"

(
    cd "$output_dir"
    sha256sum summary.csv messages.csv bursts.csv analysis.json environment.txt > SHA256SUMS
    sha256sum -c SHA256SUMS
)
