#!/bin/sh
# Fedora package: same binaries, unit bounds and exact verified console as Debian.
# Usage: packaging/build-rpm.sh OUT_DIR CONSOLE_BUILD_DIR (inside resource launcher).
set -eu
[ "$#" -ge 2 ] && [ "$#" -le 5 ] || { echo 'usage: build-rpm.sh OUT_DIR CONSOLE_BUILD_DIR' >&2; exit 1; }
out=$(realpath "$1")
[ -d "$out" ] || { echo 'create the package output directory before building' >&2; exit 1; }
root=$(cd "$(dirname "$0")/.." && pwd)
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct)}"
work=$(mktemp -d "${TMPDIR:?resource launcher scratch required}/rpm-build-XXXXXX")
cleanup() {
  status=$?
  if [ "$status" -ne 0 ]; then
    evidence=$(mktemp -d "$out/failed-rpm-XXXXXX")
    for file in "$work/SPECS/fabrico11y.spec" "$work/rpmbuild.log"; do
      if [ -f "$file" ]; then cp "$file" "$evidence/"; fi
    done
    printf 'exit=%s\nsource_date_epoch=%s\n' "$status" "$SOURCE_DATE_EPOCH" > "$evidence/status.txt"
    echo "RPM failure evidence: $evidence" >&2
  fi
  rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
release=0.alpha.2
preinst="$root/packaging/debian/preinst"
mutation="${3:-}"
if [ "${3:-}" = --historical-deb ]; then
  release=0.alpha.1
  python3 -B "$root/tools/packaging/stage_predecessor.py" "$2" "$work/stage"
  preinst="$work/stage/historical-preinst"
  "$root/packaging/check-glibc.sh" "$work/stage"/usr/bin/*
elif [ "${3:-}" = --candidate-deb ]; then
  [ "$#" -ge 4 ] || { echo 'candidate build receipt required' >&2; exit 1; }
  python3 -B "$root/tools/packaging/stage_candidate.py" "$2" "$4" "$work/stage" > "$work/candidate-provenance.json"
  SOURCE_DATE_EPOCH=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["source_date_epoch"])' "$work/candidate-provenance.json")
  export SOURCE_DATE_EPOCH
  preinst="$work/stage/candidate-preinst"
  "$root/packaging/check-glibc.sh" "$work/stage"/usr/bin/*
  mutation="${5:-}"
else
  "$root/packaging/build-stage.sh" "$work/stage" "$2"
fi
if [ "${3:-}" != --historical-deb ]; then
  case "$mutation" in
    '') ;;
    --mutate=root-user) sed -i 's/^User=fabricolly$/User=root/' "$work/stage/usr/lib/systemd/system/fabrico11y-node.service" ;;
    --mutate=no-collision-check)
      preinst="$work/no-collision-preinst"
      printf '#!/bin/sh\nexit 0\n' > "$preinst"
      ;;
    --mutate=no-memory-max) sed -i '/^MemoryMax=/d' "$work/stage/usr/lib/systemd/system/fabrico11y-node.service" ;;
    *) echo 'unknown fixture mode' >&2; exit 1 ;;
  esac
  if [ -n "$mutation" ]; then
    python3 -c 'import json,sys; from pathlib import Path; Path(sys.argv[1]).write_text(json.dumps({"role":"deliberately defective RPM negative control; not a candidate package","mutation":sys.argv[2],"baseline_payload":"CANDIDATE-PROVENANCE.json records the original unmutated Debian inputs"},indent=2)+"\n")' "$work/stage/usr/share/doc/fabrico11y/MUTATION-FIXTURE.json" "$mutation"
  fi
fi
mkdir -p "$work/tmp" "$work/BUILD" "$work/BUILDROOT" "$work/RPMS" "$work/SOURCES" "$work/SPECS" "$work/SRPMS"
cat > "$work/SPECS/fabrico11y.spec" <<SPEC
Name: fabrico11y
Version: 0.1.0
Release: $release%{?dist}
Summary: Fabric O11y bounded Linux telemetry and operator console
License: Apache-2.0
Requires: systemd >= 249
Requires(pre): /usr/bin/getent, /usr/bin/id
Requires(post): systemd
Requires(preun): systemd
Requires(postun): systemd
# Already built with the pinned Rust compiler; do not strip/rewrite hashed assets.
%global debug_package %{nil}
%global __os_install_post %{nil}
# RPM's native sysusers header would create identities before the collision
# guard. Keep the installed declarations, but create them only in guarded %post.
# ELF and other automatic dependencies remain enabled.
%global __sysusers_path ^$

%description
Central telemetry server, outbound Spindles, operator CLI and same-origin console.

%install
mkdir -p %{buildroot}
cp -a "$work/stage/." %{buildroot}/

%pre
/bin/sh -s install <<'ACCOUNT_CHECK'
SPEC
cat "$preinst" >> "$work/SPECS/fabrico11y.spec"
cat >> "$work/SPECS/fabrico11y.spec" <<'SPEC'
ACCOUNT_CHECK

%post
systemd-sysusers /usr/lib/sysusers.d/fabrico11y.conf
install -d -m 0750 -o root -g fabricolly /etc/fabrico11y
if command -v restorecon >/dev/null 2>&1; then
  restorecon -R /etc/fabrico11y /usr/bin/fabric-node /usr/bin/fabric-server /usr/bin/fabricctl
  if [ -d /usr/share/fabrico11y ]; then restorecon -R /usr/share/fabrico11y; fi
fi
systemctl daemon-reload >/dev/null 2>&1 || :
echo 'fabrico11y: configure /etc/fabrico11y from /usr/share/doc/fabrico11y/examples, then enable the required service'

%preun
if [ "$1" -eq 0 ]; then
  systemctl stop fabrico11y-node.service fabrico11y-server.service >/dev/null 2>&1 || :
fi

%postun
systemctl daemon-reload >/dev/null 2>&1 || :
# Removal and upgrades preserve operator configuration and telemetry.

%files
%defattr(-,root,root,-)
/usr/bin/fabric-node
/usr/bin/fabric-server
/usr/bin/fabricctl
/usr/lib/systemd/system/fabrico11y-node.service
/usr/lib/systemd/system/fabrico11y-server.service
/usr/lib/systemd/system/system-fabrico11y.slice
/usr/lib/sysusers.d/fabrico11y.conf
/usr/share/doc/fabrico11y
SPEC
if [ -d "$work/stage/usr/share/fabrico11y" ]; then
  echo /usr/share/fabrico11y >> "$work/SPECS/fabrico11y.spec"
fi
rm -f "$work/stage/historical-preinst" "$work/stage/candidate-preinst"
actual_tmp=$(rpmbuild --define "_tmppath $work/tmp" --eval '%{_tmppath}')
[ "$actual_tmp" = "$work/tmp" ] || { echo 'RPM scratch macro is not owned disk storage' >&2; exit 1; }
find "$work/stage" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
rpmbuild --define "_tmppath $work/tmp" --define "use_source_date_epoch_as_buildtime 1" --define "clamp_mtime_to_source_date_epoch 1" --define "_topdir $work" --define "_buildhost reproducible" --define "dist %{nil}" -bb "$work/SPECS/fabrico11y.spec"
find "$work/RPMS" -name '*.rpm' -exec cp {} "$out/" \;
for package in "$out"/fabrico11y-0.1.0-"$release"*.rpm; do
  if rpm --querytags | grep -qx SYSUSERS; then
    rpm -qp --qf '[%{SYSUSERS}\n]' "$package" > "$work/sysusers.header"
    [ ! -s "$work/sysusers.header" ] || { echo 'RPM would create accounts before the collision guard' >&2; exit 1; }
  else
    # Debian12's RPM4.18 predates native sysusers header generation (4.19).
    # Unknown newer implementations without the query tag fail closed.
    rpm_version=$(rpm --version)
    case "$rpm_version" in
      'RPM version 4.18.'*) ;;
      *) echo "Cannot verify native sysusers metadata: $rpm_version" >&2; exit 1 ;;
    esac
  fi
  (cd "$out" && sha256sum "$(basename "$package")" > "$(basename "$package").sha256")
done
