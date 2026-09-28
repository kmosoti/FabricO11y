#!/bin/bash
# Run the running-installation acceptance in a disposable Debian 13 systemd
# container. Protocol: docs/experiments/formal/installation-acceptance-protocol.md.
#
# Usage: run.sh <DEB> <OUT_DIR> [--mutate root-user|no-collision-check|no-memory-max]
# Needs Docker. The exit status is acceptance.sh's: 0 every check passed, 1 a
# check failed, 3 a check could not run here (for example MemoryHigh on a
# legacy cgroup hierarchy). A mutation injects one packaging defect that the
# acceptance must reject.
set -eu
deb=$(realpath "$1"); out=$(realpath -m "$2"); mutate=${4:-}
[ "${3:-}" = --mutate ] || [ -z "${3:-}" ] || { echo "usage: run.sh <DEB> <OUT_DIR> [--mutate NAME]" >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$out"
# On a host without the unified hierarchy, systemd 257 boots only in legacy
# mode, and only when forced; acceptance.sh then reports what cannot run there.
init_args=()
if [ "$(stat -fc %T /sys/fs/cgroup)" != cgroup2fs ]; then
  init_args=(systemd.unified_cgroup_hierarchy=0 SYSTEMD_CGROUP_ENABLE_LEGACY_FORCE=1)
  echo "host cgroup hierarchy: legacy ($(stat -fc %T /sys/fs/cgroup))" > "$out/host.txt"
else
  echo "host cgroup hierarchy: unified" > "$out/host.txt"
fi
uname -r >> "$out/host.txt"
cp "$deb" "$out/fabrico11y.deb"
if [ -n "$mutate" ]; then
  work=$(mktemp -d); dpkg-deb -R "$deb" "$work/pkg"
  case "$mutate" in
    root-user) sed -i 's/^User=fabricolly$/User=root/' "$work/pkg/usr/lib/systemd/system/fabrico11y-node.service" ;;
    no-collision-check) printf '#!/bin/sh\nexit 0\n' > "$work/pkg/DEBIAN/preinst" ;;
    no-memory-max) sed -i '/^MemoryMax=/d' "$work/pkg/usr/lib/systemd/system/fabrico11y-node.service" ;;
    *) echo "unknown mutation $mutate" >&2; exit 2 ;;
  esac
  dpkg-deb --root-owner-group -Zxz --build "$work/pkg" "$out/fabrico11y.deb" >/dev/null
  rm -rf "$work"
fi
sha256sum "$out/fabrico11y.deb" > "$out/deb.sha256"
# PROXY_CA_FILE: optional CA bundle of an HTTPS proxy, used only while building.
ctx=$(mktemp -d); cp "$here/Dockerfile" "$ctx/"
if [ -n "${PROXY_CA_FILE:-}" ]; then cp "$PROXY_CA_FILE" "$ctx/proxy-ca.crt"; else : > "$ctx/proxy-ca.crt"; fi
docker build -q -t fabric-install-host:trixie --network host --build-arg PROXY="${HTTPS_PROXY:-}" "$ctx" > "$out/image.txt"
rm -rf "$ctx"
name=fabric-accept-$$
docker run -d --name "$name" --privileged --cgroupns=private --tmpfs /run --tmpfs /run/lock \
  -e SYSTEMD_CGROUP_ENABLE_LEGACY_FORCE=1 fabric-install-host:trixie /sbin/init "${init_args[@]}" > "$out/container.txt"
trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT
for _ in $(seq 60); do
  state=$(docker exec "$name" systemctl is-system-running 2>/dev/null || true)
  case "$state" in running|degraded) break ;; esac
  sleep 1
done
echo "system state: $state" > "$out/system-state.txt"
docker cp "$out/fabrico11y.deb" "$name:/root/fabrico11y.deb"
docker cp "$here/acceptance.sh" "$name:/root/acceptance.sh"
set +e
docker exec "$name" bash /root/acceptance.sh /root/fabrico11y.deb > "$out/acceptance.txt" 2>&1
rc=$?
set -e
docker exec "$name" journalctl --no-pager -q -u fabrico11y-node -u fabrico11y-server > "$out/journal.txt" 2>&1 || true
echo "exit $rc" >> "$out/acceptance.txt"
cat "$out/acceptance.txt"
exit $rc
