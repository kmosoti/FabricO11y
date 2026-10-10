#!/usr/bin/env python3
"""Run the existing install acceptance in a disposable Debian 13 QEMU guest.

This is a local-measurement path. It leaves acceptance.sh and its A1-A13
assertions unchanged. Invoke through tools/resource_group.py; the guest runs
under QEMU TCG, so neither KVM nor host privileges are needed.
"""
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import sys
import time

DATA = Path('/run/media/kmosoti/data/FabricO11y')
IMAGE_NAME = 'debian-13-genericcloud-amd64.qcow2'
IMAGE_URL = (
    'https://cloud.debian.org/images/cloud/trixie/latest/' + IMAGE_NAME
)
MANIFEST_URL = 'https://cloud.debian.org/images/cloud/trixie/latest/SHA512SUMS'
# SHA-512 in Debian's official SHA512SUMS, observed 2026-10-09.
IMAGE_SHA512 = (
    'f46f0671a6e5bdec5291ab8972bae2f10e5408c2f64a74078f11efc2f06a436a'
    '9d0313ed50e0472542eeabf780e9f7c792ac0a314c6c20507fcd9fd81b468c3d'
)
MEMORY_LIMIT_BYTES = 20 * 1024**3
GUEST_MEMORY_MIB = 6144
GUEST_DISK_BYTES = 8 * 1024**3
BOOT_TIMEOUT_S = 600
ACCEPTANCE_TIMEOUT_S = 900
TOTAL_TIMEOUT_S = 26 * 60
MUTATIONS = {'root-user', 'no-collision-check', 'no-memory-max'}


def run(args: list[str], *, timeout: int | None = None,
        input_text: str | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, text=True, input=input_text, capture_output=True,
                          timeout=timeout, check=check)


def output_text(value: str | bytes | None) -> str:
    if value is None:
        return ''
    return value.decode(errors='replace') if isinstance(value, bytes) else value


def require_containment() -> dict[str, str]:
    if not os.environ.get('FABRIC_RESOURCE_RUNTIME_SECONDS'):
        raise RuntimeError('invoke through python3 tools/resource_group.py --')
    rel = next((line[3:] for line in Path('/proc/self/cgroup').read_text().splitlines()
                if line.startswith('0::')), None)
    if rel is None:
        raise RuntimeError('host command is not in cgroup v2')
    group = Path('/sys/fs/cgroup') / rel.lstrip('/')
    memory = (group / 'memory.max').read_text().strip()
    swap = (group / 'memory.swap.max').read_text().strip()
    if memory == 'max' or int(memory) > MEMORY_LIMIT_BYTES or swap != '0':
        raise RuntimeError(f'host cgroup not bounded: memory.max={memory}, memory.swap.max={swap}')
    return {'cgroup': rel, 'memory_max': memory, 'memory_swap_max': swap}


def digest(path: Path, algorithm: str) -> str:
    value = hashlib.new(algorithm)
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def executable(name: str) -> str:
    found = shutil.which(name)
    if found is None:
        raise RuntimeError(f'required host tool is unavailable: {name}')
    return found


def ssh_base(port: int, key: Path, known_hosts: Path) -> list[str]:
    return [executable('ssh'), '-p', str(port), '-i', str(key),
            '-o', 'IdentitiesOnly=yes', '-o', 'BatchMode=yes',
            '-o', 'StrictHostKeyChecking=accept-new',
            '-o', 'UserKnownHostsFile=' + str(known_hosts),
            '-o', 'ConnectTimeout=5', '-o', 'ServerAliveInterval=15',
            '-o', 'ServerAliveCountMax=2', 'accept@127.0.0.1']


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def cloud_config(public_key: str, run_id: str) -> str:
    return f'''#cloud-config
users:
  - name: accept
    gecos: Disposable acceptance operator
    groups: [sudo]
    shell: /bin/bash
    lock_passwd: true
    sudo: ["ALL=(ALL) NOPASSWD:ALL"]
    ssh_authorized_keys:
      - {public_key.strip()}
disable_root: true
ssh_pwauth: false
package_update: true
packages:
  - acl
  - ca-certificates
  - curl
  - dbus
  - openssl
  - procps
  - sudo
runcmd:
  - [systemctl, enable, --now, ssh]
'''


def prepare_guest_package(ssh: list[str], mutation: str) -> None:
    script = r'''set -eu
name=$1
work=/root/accept-mutation
rm -rf "$work"
mkdir -p "$work"
dpkg-deb -R /home/accept/fabrico11y.deb "$work/pkg"
case "$name" in
  root-user) sed -i 's/^User=fabricolly$/User=root/' "$work/pkg/usr/lib/systemd/system/fabrico11y-node.service" ;;
  no-collision-check) printf '#!/bin/sh\nexit 0\n' > "$work/pkg/DEBIAN/preinst"; chmod 0755 "$work/pkg/DEBIAN/preinst" ;;
  no-memory-max) sed -i '/^MemoryMax=/d' "$work/pkg/usr/lib/systemd/system/fabrico11y-node.service" ;;
  *) exit 2 ;;
esac
dpkg-deb --root-owner-group -Zxz --build "$work/pkg" /home/accept/fabrico11y-mutated.deb >/dev/null
chown accept:accept /home/accept/fabrico11y-mutated.deb
rm -rf "$work"
'''
    result = run([*ssh, 'sudo', 'bash', '-s', '--', mutation],
                 input_text=script, timeout=60, check=False)
    if result.returncode != 0:
        raise RuntimeError('guest mutation failed: ' + result.stderr[-2000:])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--deb', required=True, type=Path,
                        help='package built from the separately frozen source snapshot')
    parser.add_argument('--source-commit', required=True,
                        help='commit ID of the private frozen source snapshot')
    parser.add_argument('--run-id', required=True,
                        help='unique result name using letters, digits, dot, dash or underscore')
    parser.add_argument('--mutate', choices=sorted(MUTATIONS),
                        help='run one registered negative-control package mutation')
    args = parser.parse_args()
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]{0,79}', args.run_id):
        parser.error('--run-id contains unsupported characters')
    if not re.fullmatch(r'[0-9a-fA-F]{40,64}', args.source_commit):
        parser.error('--source-commit must be a 40- or 64-character hex object ID')

    containment = require_containment()
    if not DATA.is_dir() or not Path('/run/media/kmosoti/data').is_mount():
        raise RuntimeError(f'mounted data drive unavailable: {DATA}')
    for tool in ('curl', 'qemu-system-x86_64', 'qemu-img', 'genisoimage',
                 'ssh-keygen', 'ssh', 'scp', 'sha512sum'):
        executable(tool)
    deb = args.deb.resolve(strict=True)
    if deb.suffix != '.deb':
        raise RuntimeError(f'package path must end in .deb: {deb}')
    acceptance = Path(__file__).with_name('acceptance.sh').resolve(strict=True)
    result_dir = DATA / 'results' / 'installation-qemu' / args.run_id
    work_dir = DATA / 'scratch' / ('installation-qemu-' + args.run_id)
    if result_dir.exists() or work_dir.exists():
        raise RuntimeError(f'refusing to reuse existing run path: {result_dir} or {work_dir}')
    result_dir.mkdir(parents=True)
    work_dir.mkdir(mode=0o700)

    started = dt.datetime.now(dt.timezone.utc).isoformat()
    start_clock = time.monotonic()
    qemu: subprocess.Popen[bytes] | None = None
    qemu_log = work_dir / 'serial.log'
    qemu_stderr = work_dir / 'qemu.stderr'
    rc: int | None = None
    error: str | None = None
    cleanup_ok = False
    ssh_log = ''
    acceptance_log = ''
    journal_log = ''
    guest_info = ''
    package_sha256 = digest(deb, 'sha256')
    acceptance_sha256 = digest(acceptance, 'sha256')
    process_cleanup_confirmed = False
    try:
        manifest = work_dir / 'SHA512SUMS'
        run([executable('curl'), '--fail', '--location', '--silent', '--show-error',
             '--proto', '=https', '--tlsv1.2', '--output', str(manifest), MANIFEST_URL],
            timeout=90)
        rows = [line.split() for line in manifest.read_text().splitlines()
                if line.split() and line.split()[-1] == IMAGE_NAME]
        if len(rows) != 1 or rows[0][0] != IMAGE_SHA512:
            raise RuntimeError('official Debian manifest no longer matches the reviewed pinned image hash')
        image_cache = DATA / 'cache' / 'installation-qemu'
        image_cache.mkdir(parents=True, exist_ok=True)
        image = image_cache / IMAGE_NAME
        if image.exists():
            if digest(image, 'sha512') != IMAGE_SHA512:
                raise RuntimeError(f'cached base image hash mismatch; inspect manually: {image}')
        else:
            partial = work_dir / IMAGE_NAME
            run([executable('curl'), '--fail', '--location', '--silent', '--show-error',
                 '--proto', '=https', '--tlsv1.2', '--output', str(partial), IMAGE_URL],
                timeout=300)
            if digest(partial, 'sha512') != IMAGE_SHA512:
                raise RuntimeError('downloaded Debian image hash mismatch')
            partial.replace(image)

        pubkey = work_dir / 'id_ed25519.pub'
        private_key = work_dir / 'id_ed25519'
        run([executable('ssh-keygen'), '-q', '-t', 'ed25519', '-N', '',
             '-C', args.run_id, '-f', str(private_key)], timeout=30)
        (work_dir / 'user-data').write_text(cloud_config(pubkey.read_text(), args.run_id))
        (work_dir / 'meta-data').write_text(
            'instance-id: fabric-install-' + args.run_id +
            '\nlocal-hostname: fabric-install\n')
        seed = work_dir / 'seed.iso'
        run([executable('genisoimage'), '-quiet', '-output', str(seed), '-volid', 'cidata',
             '-joliet', '-rock', str(work_dir / 'user-data'), str(work_dir / 'meta-data')], timeout=30)

        disk = work_dir / 'guest.qcow2'
        run([executable('qemu-img'), 'create', '-q', '-f', 'qcow2', '-F', 'qcow2',
             '-b', str(image), str(disk)], timeout=30)
        run([executable('qemu-img'), 'resize', str(disk), str(GUEST_DISK_BYTES)], timeout=30)
        qinfo = run([executable('qemu-img'), 'info', '--output=json', str(disk)], timeout=30)
        if json.loads(qinfo.stdout).get('virtual-size') != GUEST_DISK_BYTES:
            raise RuntimeError('QEMU guest disk did not reach the 8 GiB virtual-size cap')

        port = free_port()
        command = [executable('qemu-system-x86_64'), '-accel', 'tcg,thread=multi',
                   '-cpu', 'max', '-smp', '2', '-m', str(GUEST_MEMORY_MIB),
                   '-drive', f'file={disk},if=virtio,format=qcow2',
                   '-drive', f'file={seed},media=cdrom,readonly=on',
                   '-netdev', f'user,id=net0,hostfwd=tcp:127.0.0.1:{port}-:22',
                   '-device', 'virtio-net-pci,netdev=net0', '-display', 'none',
                   '-serial', f'file:{qemu_log}', '-monitor', 'none', '-no-reboot']
        with qemu_stderr.open('wb') as error_stream:
            qemu = subprocess.Popen(command, stdin=subprocess.DEVNULL,
                                    stdout=subprocess.DEVNULL, stderr=error_stream,
                                    start_new_session=True)
        known_hosts = work_dir / 'known_hosts'
        ssh = ssh_base(port, private_key, known_hosts)
        connect_deadline = time.monotonic() + BOOT_TIMEOUT_S
        while time.monotonic() < connect_deadline:
            if qemu.poll() is not None:
                raise RuntimeError(f'QEMU exited before guest SSH became ready: {qemu.returncode}')
            try:
                ready = run([*ssh, 'sudo', 'cloud-init', 'status', '--long'],
                            timeout=15, check=False)
                ssh_log += ready.stdout + ready.stderr
            except subprocess.TimeoutExpired as exc:
                ssh_log += output_text(exc.stdout) + output_text(exc.stderr)
                time.sleep(2)
                continue
            if 'status: error' in ready.stdout.lower():
                raise RuntimeError('guest cloud-init failed: ' + ready.stdout[-2000:])
            if ready.returncode == 0 and re.search(r'^status: done$', ready.stdout,
                                                    flags=re.MULTILINE):
                break
            time.sleep(2)
        else:
            raise RuntimeError('guest SSH/cloud-init was not ready within 10 minutes: ' + ssh_log[-2000:])

        guest_probe = '''set -eu
printf 'debian='; cat /etc/debian_version
printf 'systemd='; systemctl --version | head -1
printf 'pid1='; ps -p 1 -o comm=
printf 'cgroup='; stat -fc %T /sys/fs/cgroup
printf 'controllers='; cat /sys/fs/cgroup/cgroup.controllers
grep '^MemTotal:' /proc/meminfo
nproc
'''
        info = run([*ssh, 'sudo', 'bash', '-s'], input_text=guest_probe, timeout=30)
        guest_info = info.stdout
        if (not re.search(r'^debian=13(?:\.|$)', guest_info, re.MULTILINE)
                or not re.search(r'^systemd=systemd 257(?:\s|$)', guest_info, re.MULTILINE)
                or not re.search(r'^pid1=systemd\s*$', guest_info, re.MULTILINE)
                or not re.search(r'^cgroup=cgroup2fs$', guest_info, re.MULTILINE)):
            raise RuntimeError('guest is not Debian 13/systemd 257 PID 1 with unified cgroup v2: '
                               + guest_info)

        scp = [executable('scp'), '-P', str(port), '-i', str(private_key),
               '-o', 'IdentitiesOnly=yes', '-o', 'BatchMode=yes',
               '-o', 'StrictHostKeyChecking=accept-new',
               '-o', 'UserKnownHostsFile=' + str(known_hosts)]
        for source, target in ((deb, 'accept@127.0.0.1:/home/accept/fabrico11y.deb'),
                               (acceptance, 'accept@127.0.0.1:/home/accept/acceptance.sh')):
            run([*scp, str(source), target], timeout=120)
        setup_script = '''set -eu
chown accept:accept /home/accept/fabrico11y.deb /home/accept/acceptance.sh
chmod 0644 /home/accept/fabrico11y.deb
chmod 0755 /home/accept/acceptance.sh
sha256sum /home/accept/fabrico11y.deb /home/accept/acceptance.sh
'''
        setup = run([*ssh, 'sudo', 'bash', '-s'], input_text=setup_script,
                    timeout=30, check=False)
        if setup.returncode != 0:
            raise RuntimeError('guest file setup failed: ' + setup.stderr[-2000:])
        received = {Path(line.split(maxsplit=1)[1]).name: line.split()[0]
                    for line in setup.stdout.splitlines() if len(line.split()) == 2}
        if (received.get('fabrico11y.deb') != package_sha256
                or received.get('acceptance.sh') != acceptance_sha256):
            raise RuntimeError('guest package or acceptance script hash changed during transfer')

        if args.mutate:
            prepare_guest_package(ssh, args.mutate)
        package = '/home/accept/fabrico11y.deb'
        if args.mutate:
            package = '/home/accept/fabrico11y-mutated.deb'
        if time.monotonic() - start_clock > TOTAL_TIMEOUT_S - ACCEPTANCE_TIMEOUT_S:
            raise RuntimeError('bootstrap consumed the reserved acceptance time budget')
        try:
            accepted = run([*ssh, 'sudo', 'bash', '/home/accept/acceptance.sh', package,
                            '--self-spindle-ca', '/etc/fabrico11y/ca.pem'],
                           timeout=ACCEPTANCE_TIMEOUT_S, check=False)
        except subprocess.TimeoutExpired as exc:
            acceptance_log = output_text(exc.stdout) + output_text(exc.stderr)
            raise RuntimeError(f'acceptance timed out after {ACCEPTANCE_TIMEOUT_S}s') from exc
        rc = accepted.returncode
        acceptance_log = accepted.stdout + accepted.stderr
        journal = run([*ssh, 'sudo', 'journalctl', '--no-pager', '-q',
                       '-u', 'fabrico11y-node', '-u', 'fabrico11y-server'],
                      timeout=60, check=False)
        journal_log = journal.stdout + journal.stderr
        if journal.returncode != 0:
            journal_log += f'\n[journalctl exit {journal.returncode}]\n'
    except (OSError, RuntimeError, subprocess.SubprocessError, ValueError, InterruptedError) as exc:
        error = f'{type(exc).__name__}: {exc}'
        rc = 2
    finally:
        if qemu is not None and qemu.poll() is None:
            qemu.terminate()
            try:
                qemu.wait(timeout=15)
            except subprocess.TimeoutExpired:
                qemu.kill()
                try:
                    qemu.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    pass
        process_cleanup_confirmed = qemu is None or qemu.poll() is not None
        cleanup_ok = process_cleanup_confirmed
        for name, value in (('acceptance.txt', acceptance_log), ('journal.txt', journal_log),
                            ('guest.txt', guest_info),
                            ('serial.txt', qemu_log.read_text(errors='replace') if qemu_log.exists() else ''),
                            ('qemu-stderr.txt', qemu_stderr.read_text(errors='replace')
                             if qemu_stderr.exists() else ''),
                            ('bootstrap.txt', ssh_log)):
            if value:
                (result_dir / name).write_text(value)
        if work_dir.exists() and cleanup_ok and rc == 0 and error is None:
            shutil.rmtree(work_dir)

    receipt = {
        'run_id': args.run_id,
        'classification': 'local VM measurement; not deployment qualification',
        'source_snapshot_commit': args.source_commit,
        'mutation': args.mutate,
        'package': str(deb),
        'package_sha256': package_sha256,
        'acceptance_sha256': acceptance_sha256,
        'debian_image_url': IMAGE_URL,
        'debian_image_sha512': IMAGE_SHA512,
        'host_cgroup': containment,
        'virtualization': {'accelerator': 'tcg', 'guest_memory_mib': GUEST_MEMORY_MIB,
                           'guest_disk_virtual_bytes': GUEST_DISK_BYTES, 'vcpus': 2},
        'guest': guest_info.strip(),
        'acceptance_exit': rc,
        'error': error,
        'started_utc': started,
        'elapsed_seconds': round(time.monotonic() - start_clock, 3),
        'cleanup_confirmed': cleanup_ok,
        'qemu_process_cleanup_confirmed': process_cleanup_confirmed,
        'scratch_removed': not work_dir.exists(),
        'scratch_retained': str(work_dir) if work_dir.exists() else None,
    }
    (result_dir / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt, indent=2))
    if not cleanup_ok:
        return 2
    return 0 if rc == 0 else 1 if rc == 1 else 3 if rc == 3 else 2


if __name__ == '__main__':
    def stop(_signum: int, _frame: object) -> None:
        raise InterruptedError('host runner interrupted')

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        sys.exit(main())
    except (OSError, RuntimeError, InterruptedError, subprocess.SubprocessError, ValueError) as exc:
        print(f'installation VM: NOT RUN: {exc}', file=sys.stderr)
        sys.exit(2)

