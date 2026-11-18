#!/usr/bin/env bash
set -euo pipefail

# Cargo uses paths under this exact crates.io sparse registry for the locked job.
registry=/github/home/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f
manifest=${CARGO_MANIFEST_DIR:-}
package=${CARGO_PKG_NAME:-}-${CARGO_PKG_VERSION:-}
if [[ "$manifest" != "$registry/$package" ]]; then
  exec "$@"
fi
if [[ "$(realpath -e -- "$manifest")" != "$manifest" ]] ||
    ! grep -Fxq -- "$package" /opt/fabrico11y/compiler-seed/provenance/packages; then
  exec "$@"
fi

crate_type=
optimized=false
for ((i=2; i<=$#; i++)); do
  argument=${!i}
  codegen=
  case "$argument" in
    --crate-type) next=$((i+1)); crate_type=${!next:-} ;;
    --crate-type=*) crate_type=${argument#*=} ;;
    -C) next=$((i+1)); codegen=${!next:-} ;;
    -C*) codegen=${argument#-C} ;;
    --target)
      next=$((i+1))
      [[ "${!next:-}" == x86_64-unknown-linux-gnu ]] || exec "$@"
      ;;
    --target=*) [[ "${argument#*=}" == x86_64-unknown-linux-gnu ]] || exec "$@" ;;
    --codegen|--codegen=*|@*) exec "$@" ;;
  esac
  case "${codegen//_/-}" in
    opt-level=3) optimized=true ;;
    opt-level=*) optimized=false ;;
    target-cpu=generic|target-cpu=x86-64) ;;
    target-cpu=*|target-feature=*) exec "$@" ;;
  esac
done
if [[ "$optimized" == true && ( "$crate_type" == lib || "$crate_type" == rlib ) ]]; then
  exec /opt/fabrico11y/compiler-seed/sccache "$@"
fi
exec "$@"
