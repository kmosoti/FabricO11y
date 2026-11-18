#!/usr/bin/env bash
set -euo pipefail

missing() {
  printf 'Missing paired cache witness: %s\n' "$*" >&2
  exit 1
}

case "$1" in
  cargo)
    profile_root=$2
    binary="$profile_root/xtask"
    [[ -x "$binary" && -s "$binary" ]] || missing "executable $binary"
    # Cargo 1.100 stores fingerprints alongside each package's hashed build unit.
    for unit in "$profile_root"/build/xtask/*; do
      [[ -s "$unit/fingerprint/bin-xtask" && -s "$unit/fingerprint/invoked.timestamp" &&
        -x "$unit/out/xtask" ]] || continue
      if [[ "$binary" -ef "$unit/out/xtask" ]] || cmp -s -- "$binary" "$unit/out/xtask"; then
        printf 'Restored Cargo 1.100 xtask binary and fingerprint: %s\n' "$unit"
        exit 0
      fi
    done
    missing "a matching Cargo 1.100 xtask binary fingerprint under $profile_root/build/xtask"
    ;;
  proof)
    proof_root=$2
    opam_lock=$3
    stamp="$proof_root/opam-root/.aeneas-build-v1"
    aeneas="$proof_root/aeneas-ab34bd2/src/_build/default/main.exe"
    charon="$proof_root/aeneas-ab34bd2/charon/charon/target/release/charon"
    [[ -s "$stamp" ]] || missing "Aeneas build stamp $stamp"
    [[ -x "$aeneas" && -s "$aeneas" ]] || missing "Aeneas executable $aeneas"
    [[ -x "$charon" && -s "$charon" ]] || missing "Charon executable $charon"
    lock_digest=$(sha256sum -- "$opam_lock")
    executable_digest=$(sha256sum -- "$aeneas")
    [[ "$(<"$stamp")" == "${lock_digest%% *} ${executable_digest%% *}" ]] ||
      missing "Aeneas build stamp matching the OPAM lock and restored executable"
    printf 'Restored Charon, Aeneas and its lock-bound build stamp\n'
    ;;
  *) missing "unknown witness type $1" ;;
esac
