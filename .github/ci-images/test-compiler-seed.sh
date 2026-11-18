#!/usr/bin/env bash
# Run inside a fresh common tool image with the repo's .github/ci-images and the
# pinned sccache binary mounted under /test. Prepopulate /github/home/.cargo
# with the crates.io sparse index and itoa 1.0.18 for the offline fixture.
set -euo pipefail

[[ "$(cargo -V)" == 'cargo 1.100.0 '* ]]
source /test/compiler-seed-release.sh
export HOME=/github/home CARGO_HOME="$cargo_home" CARGO_TARGET_DIR="$workspace/target"
export CI=true GITHUB_ACTIONS=true CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_NET_OFFLINE=true
export TMPDIR=/var/tmp
scratch=$(mktemp -d "$TMPDIR/fabric-seed-test.XXXXXXXX")
trap 'stop_seed_cache; rm -rf -- "$scratch"' EXIT

mkdir -p "$seed"/{cache,provenance} "$workspace"/{.cargo,src}
cd "$workspace"
install -m 0755 /test/sccache "$seed/sccache"
install -m 0755 /test/compiler-seed-release.sh "$seed/release-build"
install -m 0755 /test/compiler-seed-registry-rustc.sh "$seed/registry-rustc"
cat > Cargo.toml <<'MANIFEST'
[package]
name = "seed-fixture"
version = "0.1.0"
edition = "2024"

[dependencies]
itoa = "=1.0.18"
MANIFEST
printf 'fn main() { println!("{}", itoa::Buffer::new().format(42)); }\n' > src/main.rs
printf '[toolchain]\nchannel = "1.100.0"\n' > rust-toolchain.toml
: > .cargo/config.toml
cargo generate-lockfile --offline
cargo metadata --locked --filter-platform x86_64-unknown-linux-gnu --format-version 1 |
  python3 /test/compiler-seed-packages.py > "$seed/provenance/packages"
[[ "$(<"$seed/provenance/packages")" == itoa-1.0.18 ]]
seed_inputs > "$seed/provenance/inputs.sha256"
seed_identity > "$seed/provenance/identity"
printf '%040d\n' 1 > "$seed/provenance/revision"
( cd "$seed" && sha256sum sccache registry-rustc release-build ) > "$seed/provenance/tools.sha256"

release_build
baseline=$(sha256sum target/release/seed-fixture)
[[ "$(target/release/seed-fixture)" == 42 ]]
rm -rf -- target
(
  trap stop_seed_cache EXIT
  start_seed_cache READ_WRITE
  release_build
  "$seed/sccache" --show-stats --stats-format=json > "$scratch/seed.json"
)
rm -rf -- target
"$seed/release-build" --preflight
FABRIC_CI_SEED_STATS="$scratch/read.json" "$seed/release-build"
[[ "$(sha256sum target/release/seed-fixture)" == "$baseline" ]]
python3 - "$scratch/seed.json" "$scratch/read.json" <<'PY'
import json
import sys

write = json.load(open(sys.argv[1]))
read = json.load(open(sys.argv[2]))
assert write['stats']['cache_writes'] == 1
assert read['effective_cache_mode'] == 'ReadOnly'
assert read['stats']['cache_hits']['counts']['Rust'] == 1
assert read['stats']['cache_writes'] == 0
assert read['stats']['cache_read_errors'] == 0
assert read['stats']['cache_write_errors'] == 0
print('Cargo 1.100: the locked registry lib hit the read-only seed; workspace binary bytes match')
PY

printf '\n# A different valid lockfile identity\n' >> Cargo.lock
if "$seed/release-build" --preflight; then
  echo 'Compiler seed unexpectedly accepted a different lockfile' >&2
  exit 1
fi
rm -rf -- target
FABRIC_CI_SEED_STATS="$scratch/disabled.json" "$seed/release-build"
[[ ! -e "$scratch/disabled.json" && "$(target/release/seed-fixture)" == 42 ]]
printf 'Cargo 1.100: lock mismatch fell back to the original release command\n'

# Route otherwise eligible rustc arguments through instrumented executables so
# the checks distinguish direct compilation from sccache's own fallback.
mv "$seed/sccache" "$scratch/real-sccache"
printf '#!/bin/sh\nprintf "cache\\n"\n' > "$seed/sccache"
printf '#!/bin/sh\nprintf "compiler\\n"\n' > "$scratch/rustc"
chmod +x "$seed/sccache" "$scratch/rustc"
export CARGO_MANIFEST_DIR="$cargo_home/registry/src/index.crates.io-1949cf8c6b5b557f/itoa-1.0.18"
export CARGO_PKG_NAME=itoa CARGO_PKG_VERSION=1.0.18
wrapper=("$seed/registry-rustc" "$scratch/rustc" --crate-type lib -C opt-level=3)
[[ "$("${wrapper[@]}" -Ctarget-cpu=generic --target=x86_64-unknown-linux-gnu)" == cache ]]
[[ "$("${wrapper[@]}" -C target-cpu=native)" == compiler ]]
[[ "$("${wrapper[@]}" -Ctarget-cpu=native)" == compiler ]]
[[ "$("${wrapper[@]}" -Ctarget_cpu=native)" == compiler ]]
[[ "$("${wrapper[@]}" --codegen target-cpu=native)" == compiler ]]
[[ "$("${wrapper[@]}" --codegen=target-cpu=native)" == compiler ]]
[[ "$("${wrapper[@]}" -Ctarget_feature=+avx2)" == compiler ]]
[[ "$("${wrapper[@]}" --target aarch64-unknown-linux-gnu)" == compiler ]]
[[ "$("${wrapper[@]}" --target=aarch64-unknown-linux-gnu)" == compiler ]]
printf 'Host-relative CPUs and foreign Rust targets bypass the compiler cache\n'
