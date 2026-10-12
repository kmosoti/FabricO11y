#!/bin/sh
# Re-version the registered historical Debian package as the upgrade-test
# predecessor. Payload and maintainer scripts stay byte-identical; only the
# control Version becomes 0.1.0~alpha.0, so it sorts strictly below the
# 0.1.0~alpha.1 candidate as lifecycle check L1 requires.
# Usage: packaging/reversion-predecessor-deb.sh HISTORICAL_DEB OUT_DIR
set -eu
[ "$#" -eq 2 ] || { echo "usage: reversion-predecessor-deb.sh HISTORICAL_DEB OUT_DIR" >&2; exit 1; }
registered=ac71995b3c8b88cbb6f7dd05d9854e347b7a783caadc1b602f6590d6460f4a2f
[ "$(sha256sum "$1" | cut -d' ' -f1)" = "$registered" ] || { echo "not the registered historical Debian package" >&2; exit 1; }
version=0.1.0~alpha.0
out=$(realpath "$2")
work=$(mktemp -d "${TMPDIR:-/tmp}/predecessor-XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM
dpkg-deb -R "$1" "$work/root"
control="$work/root/DEBIAN/control"
stamp=$(stat -c %Y "$control")
sed -i "s/^Version: .*/Version: $version/" "$control"
touch -d "@$stamp" "$control"
arch=$(sed -n 's/^Architecture: //p' "$control")
deb="fabrico11y_${version}_${arch}.deb"
SOURCE_DATE_EPOCH=$stamp dpkg-deb --root-owner-group -Zxz --build "$work/root" "$out/$deb" >/dev/null
( cd "$out" && sha256sum "$deb" > "$deb.sha256" && cat "$deb.sha256" )
