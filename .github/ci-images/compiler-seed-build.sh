#!/usr/bin/env bash
set -euo pipefail

source .github/ci-images/compiler-seed-release.sh
: "${SOURCE_REVISION:?Build the seed from an exact trusted source revision}"
[[ "$SOURCE_REVISION" =~ ^[0-9a-f]{40}$ && "$(uname -m)" == x86_64 ]]
[[ "$(pwd -P)" == "$workspace" ]]

version=0.19.0
asset="sccache-v$version-x86_64-unknown-linux-musl"
archive=$(mktemp /var/tmp/fabric-sccache.XXXXXXXX)
trap 'stop_seed_cache; rm -f -- "$archive"' EXIT
curl --disable --fail --location --silent --show-error --retry 3 \
  "https://github.com/mozilla/sccache/releases/download/v$version/$asset.tar.gz" -o "$archive"
printf 'aa73c7270c997894038c0d8f0d297865f57569e1687883322bc431713d4bb9dd  %s\n' "$archive" |
  sha256sum --check --status
mkdir -p "$seed"/{cache,provenance}
tar -xOf "$archive" "$asset/sccache" > "$seed/sccache"
chmod 0755 "$seed/sccache"
[[ "$("$seed/sccache" --version)" == "sccache $version" ]]
install -m 0755 .github/ci-images/compiler-seed-release.sh "$seed/release-build"
install -m 0755 .github/ci-images/compiler-seed-registry-rustc.sh "$seed/registry-rustc"
printf '%s\n' "$SOURCE_REVISION" > "$seed/provenance/revision"
( cd "$seed" && sha256sum sccache registry-rustc release-build ) > "$seed/provenance/tools.sha256"

export HOME=/github/home CARGO_HOME="$cargo_home" CARGO_TARGET_DIR="$workspace/target"
export CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export CI=true GITHUB_ACTIONS=true LEAN_NUM_THREADS=4
seed_inputs > "$seed/provenance/inputs.sha256"
seed_identity > "$seed/provenance/identity"
cargo metadata --locked --filter-platform x86_64-unknown-linux-gnu --format-version 1 |
  python3 .github/ci-images/compiler-seed-packages.py > "$seed/provenance/packages"
test -s "$seed/provenance/packages"
start_seed_cache READ_WRITE
release_build
"$seed/sccache" --show-stats --stats-format=json > "$seed/provenance/seed-stats.json"
python3 - "$seed/provenance/seed-stats.json" <<'PY'
import json
import sys

stats = json.load(open(sys.argv[1]))
assert stats['stats']['cache_writes'] > 0
assert stats['stats']['cache_write_errors'] == 0
assert stats['stats']['cache_read_errors'] == 0
PY
