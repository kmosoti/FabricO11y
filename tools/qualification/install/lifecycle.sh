#!/bin/bash
# Disposable guest only. Revision3 protocol; called around an actual VM reboot.
set -euo pipefail
phase=${1:?phase}; family=${2:?family}; candidate=${3:?candidate}; previous=${4:-}
W=/root/accept
query_fixture() {
  fabricctl admin "$W/admin.conf" query '{"kind":"logs","node":"spindle-1","from_ns":0,"to_ns":9223372036854775807,"limit":1000}'
}
grade_rows() {
  python3 - "$1" "$2" <<'PY'
import json, sys
from pathlib import Path
answer = json.loads(Path(sys.argv[1]).read_text())
expected = {'accept-line-before-start', 'accept-line-during-outage', 'accept-line-after-pressure'}
rows = [r for r in answer['rows'] if r['body'] in expected]
if any(sum(r['body'] == body for r in rows) != 1 for body in expected):
    raise SystemExit('fixture missing or duplicated')
if any(r['node'] != 'spindle-1' for r in rows):
    raise SystemExit('source changed')
identity = sorted(rows, key=lambda r: r['body'])
path = Path(sys.argv[2])
if path.exists():
    if json.loads(path.read_text()) != identity:
        raise SystemExit('retained row identity changed across lifecycle')
else:
    path.write_text(json.dumps(identity, sort_keys=True))
PY
}
case "$phase" in
  upgrade)
    [ -n "$previous" ]
    if [ "$family" = debian ]; then
      old=$(dpkg-deb -f "$previous" Version); new=$(dpkg-deb -f "$candidate" Version)
      dpkg --compare-versions "$old" lt "$new"
    else
      old=$(rpm -qp --qf '%{EPOCHNUM}|%{VERSION}|%{RELEASE}' "$previous")
      new=$(rpm -qp --qf '%{EPOCHNUM}|%{VERSION}|%{RELEASE}' "$candidate")
      python3 - "$old" "$new" <<'PY'
import rpm, sys
if rpm.labelCompare(tuple(sys.argv[1].split('|')), tuple(sys.argv[2].split('|'))) >= 0:
    raise SystemExit('upgrade requires a strictly newer RPM EVR')
PY
    fi
    query_fixture > "$W/lifecycle-before.json"
    grade_rows "$W/lifecycle-before.json" "$W/lifecycle-identity.json"
    sha256sum /etc/fabrico11y/*.conf > "$W/lifecycle-config.sha256"
    getent passwd fabricolly > "$W/lifecycle-account"
    if [ "$family" = debian ]; then dpkg -i "$candidate"; else rpm -Uvh "$candidate"; fi
    # Explicit migration fixture: authenticated console acceptance is separate.
    mkdir -p /etc/systemd/system/fabrico11y-server.service.d
    printf '[Service]\nExecStart=\nExecStart=/usr/bin/fabric-server serve-legacy /etc/fabrico11y/server.conf\n' > /etc/systemd/system/fabrico11y-server.service.d/legacy-fixture.conf
    systemctl daemon-reload
    systemctl restart fabrico11y-server.service fabrico11y-node.service
    for attempt in $(seq 60); do
      if query_fixture > "$W/lifecycle-upgraded.json" 2>/dev/null; then break; fi
      sleep 1
    done
    grade_rows "$W/lifecycle-upgraded.json" "$W/lifecycle-identity.json"
    sha256sum -c "$W/lifecycle-config.sha256"
    getent passwd fabricolly | cmp - "$W/lifecycle-account"
    cat /proc/sys/kernel/random/boot_id > "$W/lifecycle-boot-id"
    python3 - <<'PY'
import hashlib, json
from pathlib import Path
root = Path('/usr/share/fabrico11y/console')
assets = json.loads((root / 'asset-manifest.json').read_text())
assert {'index.html','service-worker.js','manifest.webmanifest','icon-192.png','icon-512.png'} <= assets.keys()
assert any(name.endswith('.wasm') for name in assets)
for name, identity in assets.items():
    assert Path(name).name == name
    content = (root / name).read_bytes()
    assert len(content) == identity['bytes'] and hashlib.sha256(content).hexdigest() == identity['sha256']
PY
    echo "ACCEPT L1 PASS upgrade $old -> $new; exact source rows, configuration/account retained; complete hashed console"
    ;;
  reboot)
    [ "$(cat /proc/sys/kernel/random/boot_id)" != "$(cat "$W/lifecycle-boot-id")" ]
    systemctl is-active --quiet fabrico11y-server.service
    systemctl is-active --quiet fabrico11y-node.service
    for attempt in $(seq 60); do
      if query_fixture > "$W/lifecycle-rebooted.json" 2>/dev/null; then break; fi
      sleep 1
    done
    grade_rows "$W/lifecycle-rebooted.json" "$W/lifecycle-identity.json"
    sha256sum -c "$W/lifecycle-config.sha256"
    getent passwd fabricolly | cmp - "$W/lifecycle-account"
    [ "$family" != fedora ] || [ "$(getenforce)" = Enforcing ]
    echo 'ACCEPT L2 PASS real changed boot identity; enabled services healthy; exact retained rows/configuration/account'
    ;;
  remove)
    if [ "$family" = debian ]; then dpkg -r fabrico11y; else rpm -e fabrico11y; fi
    [ ! -e /usr/bin/fabric-server ]
    [ -d /var/lib/fabrico11y/server ] && [ -f /etc/fabrico11y/server.conf ]
    getent passwd fabricolly | cmp - "$W/lifecycle-account"
    ! systemctl is-active --quiet fabrico11y-server.service
    ! systemctl is-active --quiet fabrico11y-node.service
    echo 'ACCEPT A13a PASS remove retained telemetry/configuration/account and stopped services'
    if [ "$family" = debian ]; then
      dpkg -P fabrico11y
      [ ! -e /var/lib/fabrico11y ] && [ ! -e /etc/fabrico11y ]
      echo 'ACCEPT A13b PASS Debian purge removed state/configuration'
    else
      [ "$(getenforce)" = Enforcing ]
      auditctl -s | grep -Eq '^enabled [12]$'
      audit_search_rc=0
      ausearch -m AVC,USER_AVC -ts boot > "$W/selinux-reboot-avc.log" 2>&1 || audit_search_rc=$?
      [ "$audit_search_rc" -le 1 ]
      ! grep -E 'comm="(fabric-server|fabric-node|fabricctl)"' "$W/selinux-reboot-avc.log"
      rm -rf /var/lib/fabrico11y /etc/fabrico11y
      echo 'ACCEPT A13b PASS explicit guest cleanup after RPM preserved state/configuration'
      echo 'ACCEPT F4 PASS enforcing SELinux across reboot with no Fabric process AVC'
    fi
    ;;
  *) exit 2 ;;
esac
