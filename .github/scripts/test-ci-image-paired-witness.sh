#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")"
witness="$PWD/ci-image-paired-witness.sh"
scratch=${TMPDIR:-/data/tmp}
case "$(realpath -- "$scratch")/" in
  /tmp/*|/dev/shm/*) printf 'Use disk-backed TMPDIR for the paired witness test\n' >&2; exit 1 ;;
esac
work=$(mktemp -d "$scratch/paired-witness.XXXXXXXX")
trap 'rm -rf -- "$work"' EXIT
export TMPDIR="$work" CARGO_TARGET_DIR="$work/target" CARGO_BUILD_BUILD_DIR="$work/target"
mkdir -p "$work/project/src" "$work/restored"
cd "$work/project"
cat > Cargo.toml <<'MANIFEST'
[package]
name = "xtask"
version = "0.1.0"
edition = "2024"
MANIFEST
printf 'fn main() { println!("paired witness"); }\n' > src/main.rs
cargo +1.100.0 generate-lockfile --offline
cargo +1.100.0 build --offline --locked
cargo +1.100.0 build --offline --locked --release
tar -C "$work" -cf "$work/target.tar" target
tar -C "$work/restored" -xf "$work/target.tar"

for profile in debug release; do
  restored="$work/restored/target/$profile"
  bash "$witness" cargo "$restored"
  [[ "$("$restored/xtask")" == 'paired witness' ]]
  for marker in "$restored"/build/xtask/*/fingerprint/bin-xtask; do
    rm -- "$marker"
  done
  if bash "$witness" cargo "$restored" 2> "$work/missing"; then
    printf 'An executable without a fingerprint passed the %s witness\n' "$profile" >&2
    exit 1
  fi
  [[ "$(<"$work/missing")" == *'matching Cargo 1.100 xtask binary fingerprint'* ]]
done
