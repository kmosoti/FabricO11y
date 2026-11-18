#!/usr/bin/env bash
set -euo pipefail

seed=/opt/fabrico11y/compiler-seed
workspace=/__w/FabricO11y/FabricO11y
cargo_home=/github/home/.cargo

release_build() {
  cargo build --locked --release --workspace --bins --examples
}

seed_identity() {
  rustc -vV || return
  cargo -Vv || return
  rpm -q gcc binutils glibc glibc-devel openssl-libs openssl-devel pkgconf-pkg-config || return
  printf 'CARGO_BUILD_JOBS=%s\nCARGO_INCREMENTAL=%s\nCARGO_PROFILE_DEV_DEBUG=%s\nCARGO_PROFILE_TEST_DEBUG=%s\n' \
    "${CARGO_BUILD_JOBS:-}" "${CARGO_INCREMENTAL:-}" "${CARGO_PROFILE_DEV_DEBUG:-}" "${CARGO_PROFILE_TEST_DEBUG:-}"
}

seed_inputs() {
  local link
  link=$(find . \( -path './target' -o -path './.local' -o -name .git -o -name .jj \) -prune -o \
    -type l -print -quit) || return
  [[ -z "$link" ]] || return 1
  sha256sum -- Cargo.lock rust-toolchain.toml .cargo/config.toml || return
  # Keep the resolved default-feature graph fixed, while ignoring build output.
  find . \( -path './target' -o -path './.local' -o -name .git -o -name .jj \) -prune -o \
    -name Cargo.toml -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum --
}

seed_compatible() {
  local inputs identity
  [[ "$(pwd -P)" == "$workspace" && "${HOME:-}" == /github/home &&
    "${CARGO_HOME:-$HOME/.cargo}" == "$cargo_home" ]] || {
    echo 'compiler-seed disabled: workspace or Cargo home differs' >&2; return 1;
  }
  [[ "${CARGO_TARGET_DIR:-$workspace/target}" == "$workspace/target" &&
    "${CARGO_BUILD_TARGET_DIR:-$workspace/target}" == "$workspace/target" &&
    "${CARGO_BUILD_BUILD_DIR:-$workspace/target}" == "$workspace/target" ]] || {
    echo 'compiler-seed disabled: Cargo output path differs' >&2; return 1;
  }
  [[ ! -e .cargo/config && ! -e "$cargo_home/config" && ! -e "$cargo_home/config.toml" &&
    ! -e /__w/FabricO11y/.cargo && ! -e /__w/.cargo && ! -e /.cargo ]] || {
    echo 'compiler-seed disabled: additional Cargo configuration' >&2; return 1;
  }
  local override
  for override in ${!CARGO_BUILD_@} ${!CARGO_PROFILE_@} ${!CARGO_TARGET_@} ${!CARGO_SOURCE_@} \
    ${!CARGO_REGISTRIES_@} ${!CARGO_REGISTRY_@}; do
    case "$override" in
      CARGO_BUILD_JOBS|CARGO_BUILD_TARGET_DIR|CARGO_BUILD_BUILD_DIR|\
      CARGO_PROFILE_DEV_DEBUG|CARGO_PROFILE_TEST_DEBUG|CARGO_TARGET_DIR) ;;
      *) printf 'compiler-seed disabled: custom %s\n' "$override" >&2; return 1 ;;
    esac
  done
  for override in RUSTC RUSTFLAGS RUSTC_BOOTSTRAP CARGO_ENCODED_RUSTFLAGS RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER; do
    if [[ -n "${!override:-}" ]]; then
      printf 'compiler-seed disabled: custom %s\n' "$override" >&2; return 1
    fi
  done
  [[ -x "$seed/sccache" && -x "$seed/registry-rustc" && -d "$seed/cache" &&
    -s "$seed/provenance/packages" && -s "$seed/provenance/revision" ]] || {
    echo 'compiler-seed disabled: incomplete seed image' >&2; return 1;
  }
  (cd "$seed" && sha256sum --check --status provenance/tools.sha256) || {
    echo 'compiler-seed disabled: compiler seed tools differ' >&2; return 1;
  }
  inputs=$(seed_inputs) && [[ "$inputs" == "$(<"$seed/provenance/inputs.sha256")" ]] || {
    echo 'compiler-seed disabled: lockfile, toolchain or Cargo manifests differ' >&2; return 1;
  }
  identity=$(seed_identity) && [[ "$identity" == "$(<"$seed/provenance/identity")" ]] || {
    echo 'compiler-seed disabled: compiler, package or build environment differs' >&2; return 1;
  }
}

start_seed_cache() {
  local mode=$1 variable
  # The client and daemon use a private socket and a local-only configuration.
  for variable in ${!SCCACHE_@}; do unset "$variable"; done
  seed_run=$(mktemp -d /var/tmp/fabric-sccache.XXXXXXXX) || return
  : > "$seed_run/config" || return
  export SCCACHE_CONF="$seed_run/config" SCCACHE_SERVER_UDS="$seed_run/socket"
  export SCCACHE_CACHE_SIZE=512M SCCACHE_DIR="$seed/cache" SCCACHE_LOCAL_RW_MODE="$mode"
  export SCCACHE_IGNORE_SERVER_IO_ERROR=1 SCCACHE_IDLE_TIMEOUT=0
  "$seed/sccache" --start-server || return
  export RUSTC_WRAPPER="$seed/registry-rustc"
}

stop_seed_cache() {
  if [[ -n "${seed_run:-}" ]]; then
    if [[ -S "$seed_run/socket" ]]; then
      "$seed/sccache" --stop-server >/dev/null 2>&1 || true
    fi
    rm -rf -- "$seed_run"
  fi
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  if [[ "$#" == 1 && "$1" == --preflight ]]; then
    seed_compatible
    printf 'compiler-seed ready\n'
    exit 0
  fi
  if (( $# != 0 )); then
    echo 'Usage: release-build [--preflight]' >&2
    exit 2
  fi
  if ! seed_compatible; then
    release_build
    exit
  fi
  trap stop_seed_cache EXIT
  # The job container gives sccache a private writable overlay for LRU metadata.
  if ! start_seed_cache READ_ONLY; then
    echo 'compiler-seed disabled: local daemon unavailable' >&2
    release_build
    exit
  fi
  printf 'compiler-seed ready\n'
  export CARGO_HOME="$cargo_home" CARGO_TARGET_DIR="$workspace/target"
  result=0
  release_build || result=$?
  if [[ -n "${FABRIC_CI_SEED_STATS:-}" ]]; then
    "$seed/sccache" --show-stats --stats-format=json > "$FABRIC_CI_SEED_STATS" ||
      echo 'compiler-seed could not record local cache statistics' >&2
  fi
  exit "$result"
fi
