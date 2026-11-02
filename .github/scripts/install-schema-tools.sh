#!/bin/sh
# Pinned offline registry checker/generator; compilation uses checked-in artifacts.
set -eu
case "$(uname -m)" in
  x86_64) ;;
  *) echo 'Weaver bootstrap currently supports Linux x86_64' >&2; exit 1 ;;
esac
schema_tools="${FABRIC_SCHEMA_TOOLS:-$HOME/.local/share/fabrico11y/schema-tools}"
mkdir -p "$schema_tools"
schema_tmp=$(mktemp -d "$schema_tools/install.XXXXXXXX")
trap 'rm -rf "$schema_tmp"' EXIT HUP INT TERM
curl --proto '=https' --tlsv1.2 -fLsS https://github.com/open-telemetry/weaver/releases/download/v0.28.0/weaver-x86_64-unknown-linux-musl.tar.xz -o "$schema_tmp/weaver.tar.xz"
printf '%s  %s\n' 69069769ec3509c358ddbfbc29ff9965590fe4f8dd8b03e1c1e1be1ae56122e8 "$schema_tmp/weaver.tar.xz" | sha256sum -c -
tar -xJf "$schema_tmp/weaver.tar.xz" -C "$schema_tmp"
install -m 755 "$schema_tmp/weaver-x86_64-unknown-linux-musl/weaver" "$schema_tools/weaver-0.28.0"
"$schema_tools/weaver-0.28.0" --version
