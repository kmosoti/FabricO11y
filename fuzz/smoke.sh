#!/bin/bash
# Fuzz every target for a fixed time from the committed corpus (cargo-fuzz,
# nightly). New inputs go to target/fuzz-work, not fuzz/corpus; a crash
# leaves its input under fuzz/artifacts/<target>/ and fails the run. Dependencies
# must resolve from the committed lockfile; reject any lock rewrite. Keep a
# minimized crash as fuzz/regressions/<target>/<name> with its fix.
# Usage: bash fuzz/smoke.sh [SECONDS_PER_TARGET]   (default 60)
# Exits 3 when nightly or cargo-fuzz is missing.
set -u
cd "$(dirname "$0")/.."
seconds=${1:-60}
if ! cargo +nightly --version >/dev/null 2>&1 || ! command -v cargo-fuzz >/dev/null 2>&1; then
  echo "fuzz: NOT RUN: needs a nightly toolchain and cargo-fuzz" >&2
  exit 3
fi
# cargo-fuzz 0.13.2 does not expose Cargo's --locked option. Resolve the same
# manifest with --locked before it can build, then verify its bytes afterward.
if ! cargo +nightly metadata --manifest-path fuzz/Cargo.toml --locked --format-version 1 >/dev/null; then
  echo "fuzz: FAILED: dependency lock does not resolve unchanged" >&2
  exit 1
fi
lock_before=$(sha256sum fuzz/Cargo.lock) || exit 1
status=0
for target in batch_identify query_request frame_recovery observation_block; do
  mkdir -p "target/fuzz-work/$target"
  if cargo +nightly fuzz run "$target" "target/fuzz-work/$target" "fuzz/corpus/$target" \
      -- -max_total_time="$seconds" -rss_limit_mb=2048 >"target/fuzz-work/$target.log" 2>&1; then
    echo "fuzz $target ... ok ($(grep -o 'Done [0-9]* runs' "target/fuzz-work/$target.log"))"
  else
    echo "fuzz $target ... FAILED (see target/fuzz-work/$target.log and fuzz/artifacts/$target)"
    status=1
  fi
done
if [ "$(sha256sum fuzz/Cargo.lock)" != "$lock_before" ]; then
  echo "fuzz: FAILED: dependency lock changed during trial" >&2
  status=1
fi
exit $status
