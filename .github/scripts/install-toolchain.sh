#!/bin/sh
set -eu
dnf -y install curl tar gzip xz git gcc openssl-devel python3
toolchain_tmp=$(mktemp -d "${TMPDIR:-/var/tmp}/fabric-toolchain.XXXXXXXX")
trap 'rm -rf "$toolchain_tmp"' EXIT HUP INT TERM
check_rust_version() {
  python3 -c '
import re, subprocess, sys
out = subprocess.check_output(["rustc", "--version"], text=True)
m = re.search(r"(\d+)\.(\d+)\.(\d+)", out)
if not m:
    sys.exit(f"unable to parse rustc version: {out}")
ver = tuple(int(g) for g in m.groups())
if ver < (1, 100, 0):
    sys.exit(f"expected Rust >= 1.100.0, got {ver}")
'
}
if [ -f "$HOME/.cargo/env" ]; then
  . "$HOME/.cargo/env"
fi
if ! command -v rustc >/dev/null 2>&1 \
  || ! command -v rustup >/dev/null 2>&1 \
  || ! check_rust_version >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -fsS https://sh.rustup.rs -o "$toolchain_tmp/rustup-init.sh"
  sh "$toolchain_tmp/rustup-init.sh" -y --profile minimal --default-toolchain stable
  . "$HOME/.cargo/env"
fi
installed_components=$(rustup component list --installed 2>/dev/null || true)
if ! printf '%s\n' "$installed_components" | grep -q '^rustfmt-' \
  || ! printf '%s\n' "$installed_components" | grep -q '^clippy-'; then
  rustup component add rustfmt clippy
fi
rustc --version
check_rust_version
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
  printf '%s\n' "$HOME/.cargo/bin" >> "$GITHUB_PATH"
fi
