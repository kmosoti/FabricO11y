#!/bin/sh
set -eu
repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$repo_root"
rust_version=$(sed -n 's/^channel = "\([0-9][0-9.]*\)"$/\1/p' rust-toolchain.toml)
if [ -z "$rust_version" ]; then
    echo 'rust-toolchain.toml must pin an exact product Rust version' >&2
    exit 1
fi
dnf -y install curl tar gzip xz git gcc openssl-devel python3
toolchain_tmp=$(mktemp -d "${TMPDIR:-/var/tmp}/fabric-toolchain.XXXXXXXX")
trap 'rm -rf "$toolchain_tmp"' EXIT HUP INT TERM
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -fsS https://sh.rustup.rs -o "$toolchain_tmp/rustup-init.sh"
  sh "$toolchain_tmp/rustup-init.sh" -y --profile minimal --default-toolchain none
fi
installed_components=$(rustup component list --toolchain "$rust_version" --installed 2>/dev/null || true)
if ! rustup run "$rust_version" rustc --version >/dev/null 2>&1 \
  || ! printf '%s\n' "$installed_components" | grep -q '^rustfmt-' \
  || ! printf '%s\n' "$installed_components" | grep -q '^clippy-'; then
  rustup toolchain install "$rust_version" --profile minimal --component rustfmt,clippy
fi
rustc_version=$(rustc --version)
case "$rustc_version" in
  "rustc $rust_version "*) printf '%s\n' "$rustc_version" ;;
  *) printf 'expected Rust %s, got %s\n' "$rust_version" "$rustc_version" >&2; exit 1 ;;
esac
if [ ! -x "$HOME/.local/bin/jj" ] \
  || ! "$HOME/.local/bin/jj" --version 2>/dev/null | grep -Fxq 'jj 0.46.0-7d382314f9420b4b8d932245b49c99492c812b9f'; then
  curl --proto '=https' --tlsv1.2 -fLsS https://github.com/jj-vcs/jj/releases/download/v0.46.0/jj-v0.46.0-x86_64-unknown-linux-musl.tar.gz -o "$toolchain_tmp/jj.tar.gz"
  printf '%s  %s\n' fce0271158e665ceb82dc66c5eb95b1728649094e9133074f3b5baece345fc08 "$toolchain_tmp/jj.tar.gz" | sha256sum -c -
  mkdir -p "$HOME/.local/bin"
  tar -xzf "$toolchain_tmp/jj.tar.gz" -C "$HOME/.local/bin" ./jj
fi
"$HOME/.local/bin/jj" --version | grep -Fx 'jj 0.46.0-7d382314f9420b4b8d932245b49c99492c812b9f'
if [ -n "${GITHUB_PATH:-}" ]; then
  printf '%s\n' "$HOME/.local/bin" >> "$GITHUB_PATH"
  printf '%s\n' "${CARGO_HOME:-$HOME/.cargo}/bin" >> "$GITHUB_PATH"
fi
