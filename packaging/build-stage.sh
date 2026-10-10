#!/bin/sh
# Shared package staging; called inside the enforced resource launcher.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
python3 -B -c "import sys; sys.path.insert(0, '$root/tools'); from resource_group import require_limits; require_limits()"
stage=$1
console=$2
[ ! -e "$stage" ] || { echo 'stage must be fresh' >&2; exit 1; }
rustc --version | grep -q '^rustc 1\.99\.0 ' || { echo 'rustc 1.99.0 required' >&2; exit 1; }
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct)}"
export RUSTFLAGS="--remap-path-prefix=$root=/build/fabric_o11y --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo"
target="${CARGO_TARGET_DIR:?resource launcher target required}"
cargo build --offline --locked --release --workspace --bins --manifest-path "$root/Cargo.toml" --target-dir "$target"
mkdir -p "$stage/usr/bin" "$stage/usr/lib/systemd/system" "$stage/usr/lib/sysusers.d" "$stage/usr/share/doc/fabrico11y/examples"
for bin in fabric-node fabric-server fabricctl; do
  install -m 0755 "$target/release/$bin" "$stage/usr/bin/$bin"
done
"$root/packaging/check-glibc.sh" "$stage"/usr/bin/*
install -m 0644 "$root"/packaging/systemd/* "$stage/usr/lib/systemd/system/"
install -m 0644 "$root/packaging/sysusers.d/fabrico11y.conf" "$stage/usr/lib/sysusers.d/"
install -m 0644 "$root/LICENSE" "$root/NOTICE" "$stage/usr/share/doc/fabrico11y/"
install -m 0644 "$root"/packaging/etc/*.example "$stage/usr/share/doc/fabrico11y/examples/"
python3 -B "$root/tools/packaging/stage_console.py" "$console" "$stage/usr/share/fabrico11y/console"
python3 -B "$root/tools/packaging/dependency_notices.py" "$stage/usr/share/doc/fabrico11y"
