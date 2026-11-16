#!/bin/sh
# Pinned offline registry checker/generator; compilation uses checked-in artifacts.
set -eu
case "$(uname -m)" in
  x86_64) ;;
  *) echo 'Weaver bootstrap currently supports Linux x86_64' >&2; exit 1 ;;
esac
schema_tools="${FABRIC_SCHEMA_TOOLS:-$HOME/.local/share/fabrico11y/schema-tools}"
weaver_bin="$schema_tools/weaver-0.28.0"
if [ -x "$weaver_bin" ] \
  && [ "$("$weaver_bin" --version 2>/dev/null || true)" = "weaver 0.28.0" ] \
  && printf '%s  %s\n' 6c67e08ba8abb2c60cf8a58c299b5db460c0fbe7226f7453d4878dcc4f6c3867 "$weaver_bin" | sha256sum -c - >/dev/null 2>&1; then
  "$weaver_bin" --version
  exit 0
fi
mkdir -p "$schema_tools"
schema_tmp=$(mktemp -d "$schema_tools/install.XXXXXXXX")
trap 'rm -rf "$schema_tmp"' EXIT HUP INT TERM
curl --proto '=https' --tlsv1.2 -fLsS https://github.com/open-telemetry/weaver/releases/download/v0.28.0/weaver-x86_64-unknown-linux-musl.tar.xz -o "$schema_tmp/weaver.tar.xz"
printf '%s  %s\n' 69069769ec3509c358ddbfbc29ff9965590fe4f8dd8b03e1c1e1be1ae56122e8 "$schema_tmp/weaver.tar.xz" | sha256sum -c -
tar -xJf "$schema_tmp/weaver.tar.xz" -C "$schema_tmp"
install -m 755 "$schema_tmp/weaver-x86_64-unknown-linux-musl/weaver" "$weaver_bin"
"$weaver_bin" --version
