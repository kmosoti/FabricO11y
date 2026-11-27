#!/bin/sh
# Build fabrico11y_<version>_<arch>.deb natively against Debian 12 (bookworm)
# glibc 2.36 so binaries run unchanged on Debian 12 and Debian 13.
set -eu

resolve_disk_tmp() {
  for cand in "${TMPDIR:-}" "/data/tmp" "/var/tmp"; do
    [ -n "$cand" ] || continue
    case "$cand/" in
      /tmp/*|/dev/shm/*|/run/shm/*) continue ;;
    esac
    if mkdir -p "$cand" 2>/dev/null; then
      printf '%s\n' "$cand"
      return 0
    fi
  done
  echo "no non-tmpfs temporary directory available (set TMPDIR)" >&2
  exit 1
}

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
bin_dir=""
out_dir=""
default_ver=$(sed -En 's/^version = "([^"]+)"$/\1/p' "$root/fabric/Cargo.toml" | head -n 1)
version=$(printf '%s' "$default_ver" | sed 's/-/~/')
skip_notices=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --bin-dir)
      bin_dir=$2
      shift 2
      ;;
    --out-dir)
      out_dir=$2
      shift 2
      ;;
    --version)
      version=$2
      shift 2
      ;;
    --skip-notices)
      skip_notices=1
      shift
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

[ -n "$out_dir" ] || {
  echo "usage: build-deb.sh --out-dir DIR [--bin-dir DIR] [--version VER] [--skip-notices]" >&2
  exit 1
}

mkdir -p "$out_dir"
out_dir=$(CDPATH= cd -- "$out_dir" && pwd)
tmp_base=$(resolve_disk_tmp)
export TMPDIR="$tmp_base"

if [ -z "$bin_dir" ]; then
  : "${CARGO_BUILD_TARGET_DIR:=/data/cargo-target}"
  export CARGO_BUILD_TARGET_DIR
  (cd "$root" && cargo build --locked --offline --release -p fabric -p fabric-cli -p fabric-query -p fabric-server -p fabric-mcp --bins -j 2)
  bin_dir="$CARGO_BUILD_TARGET_DIR/release"
fi

arch=$(dpkg --print-architecture)
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct 2>/dev/null || date -u +%s)}"

work=$(mktemp -d "$tmp_base/fabric-deb-build.XXXXXXXX")
trap 'rm -rf -- "$work"' EXIT HUP INT TERM
stage="$work/stage"

if [ "$skip_notices" -eq 1 ]; then
  "$root/packaging/deb/stage-root.sh" --bin-dir "$bin_dir" --dest "$stage" --layout fhs --skip-notices
else
  "$root/packaging/deb/stage-root.sh" --bin-dir "$bin_dir" --dest "$stage" --layout fhs
fi

mkdir -p "$stage/DEBIAN" "$work/shlibs/debian"
printf 'Source: fabrico11y\n\nPackage: fabrico11y\nArchitecture: any\n' > "$work/shlibs/debian/control"
shlibs=$(
  cd "$work/shlibs" && dpkg-shlibdeps -O \
    -e "$stage/usr/bin/fabric" \
    -e "$stage/usr/bin/spindle" \
    -e "$stage/usr/bin/fabric-server" \
    -e "$stage/usr/bin/fabric-adapter" \
    -e "$stage/usr/bin/fabric-query" \
    -e "$stage/usr/bin/fabric-mcp" 2>/dev/null \
    | sed -n 's/^shlibs:Depends=//p'
)
[ -n "$shlibs" ] || {
  echo "dpkg-shlibdeps produced empty library dependencies" >&2
  exit 1
}

cat > "$stage/DEBIAN/control" <<CONTROL
Package: fabrico11y
Version: $version
Architecture: $arch
Maintainer: Kennedy Mosoti <47609243+kmosoti@users.noreply.github.com>
Depends: $shlibs, systemd (>= 252)
Section: admin
Priority: optional
Homepage: https://github.com/kmosoti/FabricO11y
Description: CLI-first Linux observability and durable telemetry custody
 FabricO11y collects host, cgroup v2, agent, and Spindle observations into
 embedded redb and ZSTD Parquet storage with optional loopback-authenticated
 canonical custody forwarding.
CONTROL

install -m 0755 \
  "$root/packaging/deb/debian/postinst" \
  "$root/packaging/deb/debian/prerm" \
  "$root/packaging/deb/debian/postrm" \
  "$stage/DEBIAN/"

find "$stage" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
deb_name="fabrico11y_${version}_${arch}.deb"
dpkg-deb --root-owner-group -Zxz --build "$stage" "$out_dir/$deb_name" >/dev/null
(cd "$out_dir" && sha256sum "$deb_name" > "$deb_name.sha256")
cat "$out_dir/$deb_name.sha256"
