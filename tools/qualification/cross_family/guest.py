"""Owned two-OS placement primitives; imports no application implementation."""
from __future__ import annotations
import importlib.util
import json
from pathlib import Path
import subprocess
import time

SPEC = importlib.util.spec_from_file_location('install_qemu', Path(__file__).resolve().parent.parent / 'install/run-qemu.py')
Q = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(Q)
IMAGES = {
    'debian': ('debian-13-genericcloud-amd64.qcow2', 'sha512', Q.IMAGE_SHA512),
    'fedora': ('Fedora-Cloud-Base-Generic-44-1.7.x86_64.qcow2', 'sha256',
               '28680fe5b371a5a82ebf43a31926e086a168e59949d03969c5093e7071f90b7f'),
}


def image(family):
    name, algorithm, expected = IMAGES[family]
    path = Q.DATA / 'cache/installation-qemu' / name
    if Q.digest(path, algorithm) != expected:
        raise ValueError('official pinned guest base image is unavailable or changed')
    return path


class Guest:
    def __init__(self, role, family, root, results, key, memory_mib, data_port=None):
        self.role = role
        self.family = family
        self.root = root / role
        self.results = results
        self.key = key
        self.memory_mib = memory_mib
        self.port = Q.free_port()
        self.data_port = data_port
        self.child = None
        self.boot_id = None
        self.root.mkdir(exist_ok=False)
        self.ssh = Q.ssh_base(self.port, key, self.root / 'known_hosts')

    def script(self, text, timeout=30, check=True):
        return Q.run([*self.ssh, 'sudo', 'bash', '-s'], input_text=text, timeout=timeout, check=check)

    def copy(self, path, destination):
        # SSH argv has no local shell quoting. Destinations are fixed guest paths.
        command = [Q.executable('scp'), '-P', str(self.port), '-i', str(self.key),
                   '-o', 'IdentitiesOnly=yes', '-o', 'BatchMode=yes',
                   '-o', 'StrictHostKeyChecking=accept-new',
                   '-o', 'UserKnownHostsFile=' + str(self.root / 'known_hosts'),
                   str(path), 'accept@127.0.0.1:' + destination]
        Q.run(command, timeout=60)

    def start(self):
        config = Q.cloud_config(self.key.with_suffix('.pub').read_text(), self.role)
        if self.family == 'fedora':
            config = config.replace('groups: [sudo]', 'groups: [wheel]')
            config = config.replace('  - procps\n', '  - procps-ng\n  - rpm\n  - audit\n  - policycoreutils\n')
            config = config.replace('enable, --now, ssh]', 'enable, --now, sshd]')
            config += '  - [systemctl, enable, --now, auditd]\n'
        config = config.replace('  - sudo\n', '  - sudo\n  - nftables\n')
        config += '  - [swapoff, -a]\n'
        (self.root / 'user-data').write_text(config)
        (self.root / 'meta-data').write_text('instance-id: cross-' + self.role + '\nlocal-hostname: cross-' + self.role + '\n')
        seed = self.root / 'seed.iso'
        Q.run([Q.executable('genisoimage'), '-quiet', '-output', str(seed), '-volid', 'cidata', '-joliet', '-rock', str(self.root / 'user-data'), str(self.root / 'meta-data')], timeout=30)
        disk = self.root / 'guest.qcow2'
        Q.run([Q.executable('qemu-img'), 'create', '-q', '-f', 'qcow2', '-F', 'qcow2', '-b', str(image(self.family)), str(disk)], timeout=30)
        Q.run([Q.executable('qemu-img'), 'resize', str(disk), str(6 * 1024**3)], timeout=30)
        if json.loads(Q.run([Q.executable('qemu-img'), 'info', '--output=json', str(disk)]).stdout)['virtual-size'] != 6 * 1024**3:
            raise ValueError('guest disk cap not applied')
        net = f'user,id=net0,hostfwd=tcp:127.0.0.1:{self.port}-:22'
        if self.data_port is not None:
            net += f',hostfwd=tcp:127.0.0.1:{self.data_port}-:7443'
        command = [Q.executable('qemu-system-x86_64'), '-accel', 'tcg,thread=multi', '-cpu', 'max', '-smp', '2', '-m', str(self.memory_mib),
                   '-drive', f'file={disk},if=virtio,format=qcow2', '-drive', f'file={seed},media=cdrom,readonly=on',
                   '-netdev', net, '-device', 'virtio-net-pci,netdev=net0', '-display', 'none',
                   '-serial', f'file:{self.results / (self.role + "-serial.txt")}', '-monitor', 'none']
        with (self.results / (self.role + '-qemu-stderr.txt')).open('wb') as stream:
            self.child = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=stream)
        self.command = command

    def ready(self):
        deadline = time.monotonic() + Q.BOOT_TIMEOUT_S
        attempts = []
        while time.monotonic() < deadline:
            if self.child.poll() is not None:
                raise RuntimeError(self.role + ' guest stopped during boot')
            try:
                result = Q.run([*self.ssh, 'sudo', 'cloud-init', 'status', '--long'], timeout=15, check=False)
                attempts.append(result.stdout + result.stderr)
                if result.returncode == 0 and 'status: done' in result.stdout:
                    break
                if 'status: error' in result.stdout:
                    raise RuntimeError(self.role + ' cloud-init failed')
            except subprocess.TimeoutExpired:
                pass
            time.sleep(2)
        else:
            raise RuntimeError(self.role + ' guest boot deadline exceeded')
        (self.results / (self.role + '-bootstrap.txt')).write_text('\n'.join(attempts))
        facts = self.script('. /etc/os-release; echo os=$ID:$VERSION_ID; cat /proc/sys/kernel/random/boot_id; stat -fc %T /sys/fs/cgroup; cat /proc/swaps; test "$(wc -l < /proc/swaps)" = 1; getenforce 2>/dev/null || true\n').stdout
        expected = 'os=fedora:44' if self.family == 'fedora' else 'os=debian:13'
        if expected not in facts or 'cgroup2fs' not in facts or (self.family == 'fedora' and 'Enforcing' not in facts):
            raise RuntimeError(self.role + ' OS/cgroup/SELinux prerequsites unavailable')
        self.boot_id = facts.splitlines()[1]
        (self.results / (self.role + '-guest.txt')).write_text(facts)

    def install(self, package, helpers):
        self.copy(package, '/home/accept/package.' + ('rpm' if self.family == 'fedora' else 'deb'))
        for helper in ('spool_dump', 'server_dump'):
            self.copy(helpers / helper, '/home/accept/' + helper)
        install = 'rpm -Uvh /home/accept/package.rpm' if self.family == 'fedora' else 'dpkg -i /home/accept/package.deb'
        command = 'set -eu\n' + install + '\ninstall -d -m 0755 /root/cross\ninstall -m 0700 /home/accept/spool_dump /home/accept/server_dump /root/cross/\nsha256sum /usr/bin/fabric-server /usr/bin/fabric-node /usr/bin/fabricctl\n'
        result = self.script(command, timeout=120)
        (self.results / (self.role + '-install.txt')).write_text(result.stdout + result.stderr)

    def clock(self):
        return self.correlation()['monotonic_before_ns']

    def correlation(self):
        before = time.monotonic_ns()
        result = Q.run([*self.ssh, 'python3', '-'], input_text='''import json,time
from pathlib import Path
before=time.monotonic_ns()
wall=time.time_ns()
after=time.monotonic_ns()
print(json.dumps({'boot_id':Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
 'monotonic_before_ns':before,'unix_ns':wall,'monotonic_after_ns':after}))
''', timeout=15)
        after = time.monotonic_ns()
        value = json.loads(result.stdout)
        if (value['boot_id'] != self.boot_id or value['monotonic_after_ns'] < value['monotonic_before_ns']):
            raise ValueError('guest clock identity or sampling order changed')
        return {**value, 'host_request_before_ns': before, 'host_request_after_ns': after,
                'ssh_roundtrip_ns': after - before}

    def stop(self):
        if self.child is not None and self.child.poll() is None:
            self.child.terminate()
            try:
                self.child.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait(timeout=10)
        return self.child is None or self.child.poll() is not None
