#!/bin/sh
# Build reproducible Linux x86_64 .tar.zst release archive from Debian 12 (glibc <= 2.36) binaries.
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
version="0.1.0-alpha.3"
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
  echo "usage: build-tarball.sh --out-dir DIR [--bin-dir DIR] [--version VER] [--skip-notices]" >&2
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

arch=$(uname -m)
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct 2>/dev/null || date -u +%s)}"

work=$(mktemp -d "$tmp_base/fabric-tar-build.XXXXXXXX")
trap 'rm -rf -- "$work"' EXIT HUP INT TERM

pkg_dir="fabrico11y-${version}-${arch}-unknown-linux-gnu"
stage="$work/$pkg_dir"

if [ "$skip_notices" -eq 1 ]; then
  "$root/packaging/deb/stage-root.sh" --bin-dir "$bin_dir" --dest "$stage" --layout portable --skip-notices
else
  "$root/packaging/deb/stage-root.sh" --bin-dir "$bin_dir" --dest "$stage" --layout portable
fi

tar_name="${pkg_dir}.tar.zst"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$SOURCE_DATE_EPOCH" \
  -C "$work" -cf - "$pkg_dir" | zstd -T0 -19 -q -o "$out_dir/$tar_name"
(cd "$out_dir" && sha256sum "$tar_name" > "$tar_name.sha256")
cat "$out_dir/$tar_name.sha256"
