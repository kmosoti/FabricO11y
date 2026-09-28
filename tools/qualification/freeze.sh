#!/bin/bash
# Freeze the release binaries, the examples the harnesses launch, and the
# harness itself into target/<NAME>, with SHA-256 values and the source commit.
# Usage: tools/qualification/freeze.sh <NAME>   (NAME must start with alpha-)
# Check it again after the runs with: (cd target/<NAME> && sha256sum -c hashes.txt)
set -eu
name=$1
case "$name" in alpha-*) ;; *) echo "NAME must start with alpha- (runner convention)" >&2; exit 2 ;; esac
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
[ -z "$(git status --porcelain -- src crates examples tools/qualification Cargo.toml Cargo.lock)" ] \
  || { echo "uncommitted changes under src, crates, examples or tools/qualification; commit first" >&2; exit 1; }
cargo build --release --locked --workspace --bins --examples
out=target/$name
rm -rf "$out"
mkdir -p "$out/examples" "$out/tools"
cp target/release/fabric-server target/release/fabric-node target/release/fabricctl "$out/"
for example in server_dump spindle_sim spool_dump server_mode local_log_probe; do
  cp "target/release/examples/$example" "$out/examples/"
done
cp tools/qualification/*.py "$out/tools/"
git rev-parse HEAD > "$out/source-commit.txt"
( cd "$out" && find . -type f ! -name hashes.txt ! -name source-commit.txt | sed 's|^\./||' | sort \
  | xargs sha256sum > hashes.txt )
echo "frozen $(wc -l < "$out/hashes.txt") files from $(cat "$out/source-commit.txt") into $out"
