#!/bin/sh
# Build fabrico11y_<version>_amd64.deb from release binaries.
# Usage: packaging/build-deb.sh <OUT_DIR>
# Requires the pinned toolchain (rustc 1.98.0) and dpkg-deb. Builds with
# --locked and a fixed SOURCE_DATE_EPOCH so two clean builds can be compared.
set -eu
out=$(realpath "$1")
root=$(cd "$(dirname "$0")/.." && pwd)
version=0.1.0~alpha.1
rustc --version | grep -q '^rustc 1\.98\.0 ' || { echo "rustc 1.98.0 required" >&2; exit 1; }
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct)}"
export RUSTFLAGS="--remap-path-prefix=$root=/build/fabric_o11y --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo"
target="${CARGO_TARGET_DIR:-$root/target/package-build}"
cargo build --offline --locked --release --workspace --bins --manifest-path "$root/Cargo.toml" --target-dir "$target"
stage="$out/stage"
rm -rf "$stage"
mkdir -p "$stage/DEBIAN" "$stage/usr/bin" "$stage/usr/lib/systemd/system" "$stage/usr/lib/sysusers.d" \
         "$stage/usr/share/doc/fabrico11y/examples"
for bin in fabric-node fabric-server fabricctl; do
  install -m 0755 "$target/release/$bin" "$stage/usr/bin/$bin"
done
install -m 0644 "$root"/packaging/systemd/* "$stage/usr/lib/systemd/system/"
install -m 0644 "$root/packaging/sysusers.d/fabrico11y.conf" "$stage/usr/lib/sysusers.d/"
install -m 0644 "$root"/packaging/etc/*.example "$stage/usr/share/doc/fabrico11y/examples/"
cat > "$stage/DEBIAN/control" <<CONTROL
Package: fabrico11y
Version: $version
Architecture: amd64
Maintainer: Fabric O11y maintainers
Depends: systemd (>= 257)
Section: admin
Priority: optional
Description: Fabric O11y controlled alpha: node, server and fabricctl
 Outbound Linux nodes, one central server with durable delivery, control
 and retained history, and an operator CLI. Alpha; see the project docs.
CONTROL
install -m 0755 "$root/packaging/debian/preinst" "$root/packaging/debian/postinst" \
                "$root/packaging/debian/prerm" "$root/packaging/debian/postrm" "$stage/DEBIAN/"
find "$stage" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
dpkg-deb --root-owner-group -Zxz --build "$stage" "$out/fabrico11y_${version}_amd64.deb" >/dev/null
( cd "$out" && sha256sum "fabrico11y_${version}_amd64.deb" > "fabrico11y_${version}_amd64.deb.sha256" )
cat "$out/fabrico11y_${version}_amd64.deb.sha256"
