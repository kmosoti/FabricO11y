#!/bin/sh
# Build fabrico11y_<version>_<arch>.deb from release binaries for Debian-family
# distributions (Debian 12 and 13, Ubuntu 22.04 and 24.04 and their derivatives;
# ADR-0025): the architecture is the build host's, library dependencies are
# computed from the binaries by dpkg-shlibdeps, and the build fails if a binary
# needs a glibc newer than 2.34 (packaging/check-glibc.sh).
# Usage: packaging/build-deb.sh <OUT_DIR> <CONSOLE_BUILD_DIR>
# Requires the pinned toolchain (rustc 1.99.0), dpkg-deb, dpkg-shlibdeps and
# objdump. Builds with --locked and a fixed SOURCE_DATE_EPOCH so two clean
# builds can be compared.
set -eu
[ "$#" -eq 2 ] || { echo "usage: build-deb.sh OUT_DIR CONSOLE_BUILD_DIR" >&2; exit 1; }
out=$(realpath "$1")
root=$(cd "$(dirname "$0")/.." && pwd)
version=0.1.0~alpha.2
arch=$(dpkg --print-architecture)
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct)}"
stage=$(mktemp -d "${TMPDIR:?resource launcher scratch required}/deb-stage-XXXXXX")
rmdir "$stage"
trap 'rm -rf "$stage"' EXIT HUP INT TERM
"$root/packaging/build-stage.sh" "$stage" "$2"
mkdir -p "$stage/DEBIAN"
# Library dependencies from the binaries themselves (for example
# "libc6 (>= 2.34), libgcc-s1 (>= 4.2)"); systemd 249 is the oldest whose unit
# directives and systemd-sysusers behaviour the package relies on.
shlibs=$(
  scratch=$(mktemp -d)
  mkdir -p "$scratch/debian"
  printf 'Source: fabrico11y\n\nPackage: fabrico11y\nArchitecture: any\n' > "$scratch/debian/control"
  cd "$scratch" && dpkg-shlibdeps -O -e "$stage/usr/bin/fabric-node" -e "$stage/usr/bin/fabric-server" \
    -e "$stage/usr/bin/fabricctl" 2>/dev/null | sed -n 's/^shlibs:Depends=//p'
  rm -rf "$scratch"
)
[ -n "$shlibs" ] || { echo "dpkg-shlibdeps produced no dependencies" >&2; exit 1; }
cat > "$stage/DEBIAN/control" <<CONTROL
Package: fabrico11y
Version: $version
Architecture: $arch
Maintainer: Fabric O11y maintainers
Depends: $shlibs, systemd (>= 249)
Section: admin
Priority: optional
Description: Fabric O11y controlled alpha: node, server, CLI and console
 Outbound Linux nodes, one central server with durable delivery, control
 and retained history, an operator CLI and a same-origin console.
 Alpha; see the project docs.
CONTROL
install -m 0755 "$root/packaging/debian/preinst" "$root/packaging/debian/postinst" \
                "$root/packaging/debian/prerm" "$root/packaging/debian/postrm" "$stage/DEBIAN/"
find "$stage" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
dpkg-deb --root-owner-group -Zxz --build "$stage" "$out/fabrico11y_${version}_${arch}.deb" >/dev/null
( cd "$out" && sha256sum "fabrico11y_${version}_${arch}.deb" > "fabrico11y_${version}_${arch}.deb.sha256" )
cat "$out/fabrico11y_${version}_${arch}.deb.sha256"
