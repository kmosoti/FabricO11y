#!/bin/sh
# Stage FabricO11y release binaries, systemd user units, configuration examples,
# and audited license notices into an FHS (.deb) or portable (.tar.zst) root.
set -eu

bin_dir=""
dest=""
layout="fhs"
skip_notices=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --bin-dir)
      bin_dir=$2
      shift 2
      ;;
    --dest)
      dest=$2
      shift 2
      ;;
    --layout)
      layout=$2
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

[ -n "$bin_dir" ] && [ -n "$dest" ] || {
  echo "usage: stage-root.sh --bin-dir DIR --dest DIR [--layout fhs|portable] [--skip-notices]" >&2
  exit 1
}

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)

case "$layout" in
  fhs)
    bindir="$dest/usr/bin"
    unitdir="$dest/usr/lib/systemd/user"
    docdir="$dest/usr/share/doc/fabrico11y"
    licensedir="$dest/usr/share/licenses/fabrico11y"
    ;;
  portable)
    bindir="$dest/bin"
    unitdir="$dest/lib/systemd/user"
    docdir="$dest/share/doc/fabrico11y"
    licensedir="$dest/share/licenses/fabrico11y"
    ;;
  *)
    echo "unsupported layout: $layout" >&2
    exit 1
    ;;
esac

install -d "$bindir" "$unitdir" "$docdir/examples" "$licensedir"

binaries="fabric spindle fabric-server fabric-adapter fabric-query fabric-mcp"
for bin in $binaries; do
  [ -x "$bin_dir/$bin" ] || {
    echo "missing executable binary: $bin_dir/$bin" >&2
    exit 1
  }
  install -m 0755 "$bin_dir/$bin" "$bindir/$bin"
done

if [ "${FABRIC_SKIP_GLIBC_CHECK:-0}" != "1" ]; then
  "$root/packaging/deb/check-glibc.sh" \
    "$bindir/fabric" \
    "$bindir/spindle" \
    "$bindir/fabric-server" \
    "$bindir/fabric-adapter" \
    "$bindir/fabric-query" \
    "$bindir/fabric-mcp"
fi

install -m 0644 "$root/packaging/deb/units/"* "$unitdir/"
cp -a "$root/config" "$root/examples" "$docdir/examples/"
install -m 0644 \
  "$root/docs/collection.md" \
  "$root/docs/deployment-verification.md" \
  "$root/packaging/deb/README.md" \
  "$docdir/"
install -m 0644 "$root/LICENSE" "$root/NOTICE" "$licensedir/"
if [ "$layout" = "fhs" ]; then
  install -m 0644 "$root/LICENSE" "$docdir/copyright"
fi

if [ "$skip_notices" -eq 0 ]; then
  (cd "$root" && python3 packaging/dependency-notices.py "$licensedir/dependencies")
fi
