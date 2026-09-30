#!/bin/bash
# Run the Kani proof harnesses of fabric-core (crates/fabric-core/src/proofs.rs).
# Prints one line per harness, "test <harness> ... ok|FAILED", in libtest's
# format so `cargo xtask mutants` can name a harness as the expected failure.
# Exits 0 only if every harness verified; 3 if Kani is not installed.
# Usage: bash formal/kani/check.sh [--harness NAME]...
set -u
cd "$(dirname "$0")/../.."
if ! command -v cargo-kani >/dev/null 2>&1; then
  echo "kani: NOT RUN: cargo-kani is not installed (cargo install --locked kani-verifier && cargo kani setup)" >&2
  exit 3
fi
out=$(cargo kani -p fabric-core "$@" 2>&1)
rc=$?
harnesses=$(printf '%s\n' "$out" | sed -n 's/^Checking harness \(.*\)\.\.\.$/\1/p' | sort -u)
failed=$(printf '%s\n' "$out" | sed -n 's/^Verification failed for - \(.*\)$/\1/p')
if [ -z "$harnesses" ]; then
  printf '%s\n' "$out" | tail -20
  echo "kani: no harness ran (exit $rc)" >&2
  exit 1
fi
for h in $harnesses; do
  if printf '%s\n' "$failed" | grep -qx "$h"; then echo "test $h ... FAILED"; else echo "test $h ... ok"; fi
done
printf '%s\n' "$out" | grep -E "^Complete - " || true
[ "$rc" -eq 0 ] && [ -z "$failed" ]
