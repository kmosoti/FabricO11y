"""Owned two-OS placement primitives; imports no application implementation."""
from __future__ import annotations
import importlib.util
import base64
import hashlib
import json
import os
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
    def __init__(self, role, family, root, results, key, memory_mib, data_port=None, *, network=None, acceleration='tcg'):
        if network not in (None, '192.0.2.0/24'):
            raise ValueError('only the registered walkthrough network may override usernet')
        if acceleration not in ('tcg', 'kvm'):
            raise ValueError('unregistered guest acceleration')
        self.acceleration = acceleration
        self.role = role
        self.family = family
        self.root = root / role
        self.results = results
        self.key = key
        self.memory_mib = memory_mib
        self.port = Q.free_port()
        self.data_port = data_port
        self.network = network
        self.child = None
        self.boot_id = None
        self.clock_channel = None
        self.root.mkdir(exist_ok=False)
        self.ssh = Q.ssh_base(self.port, key, self.root / 'known_hosts')

    def script(self, text, timeout=30, check=True):
        try:
            return Q.run([*self.ssh, 'sudo', 'bash', '-s'], input_text=text, timeout=timeout, check=check)
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
            def output(value):
                if isinstance(value, bytes):
                    return {'encoding': 'base64', 'data': base64.b64encode(value).decode('ascii')}
                return value or ''
            evidence = {'command': error.cmd, 'stdin_sha256': hashlib.sha256(text.encode()).hexdigest(),
                        'boot_id': getattr(self, 'boot_id', None),
                        'exit': getattr(error, 'returncode', None),
                        'timeout_s': getattr(error, 'timeout', None),
                        'stdout': output(error.stdout), 'stderr': output(error.stderr)}
            try:
                number = 1
                path = self.results / (self.role + '-command-failure-' + f'{number:04}.json')
                while path.exists():
                    number += 1
                    path = self.results / (self.role + '-command-failure-' + f'{number:04}.json')
                fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                with os.fdopen(fd, 'w') as stream:
                    json.dump(evidence, stream, indent=2)
                    stream.write('\n')
            except (OSError, TypeError, ValueError) as failure:
                error.add_note('command failure evidence unavailable: ' + str(failure))
            raise

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
        # Defer naming until the final stage rather than depending on an early
        # hostname setup in the pinned Fedora image. Readiness remains strict.
        config = config.replace('#cloud-config\n', '#cloud-config\npreserve_hostname: true\n', 1)
        if self.family == 'fedora':
            config = config.replace('groups: [sudo]', 'groups: [wheel]')
            config = config.replace('  - procps\n', '  - procps-ng\n  - rpm\n  - audit\n  - policycoreutils\n')
            config = config.replace('enable, --now, ssh]', 'enable, --now, sshd]')
            config += '  - [systemctl, enable, --now, auditd]\n'
        config = config.replace('  - sudo\n', '  - sudo\n  - nftables\n')
        config += '  - [hostnamectl, set-hostname, cross-' + self.role + ']\n'
        if self.family == 'fedora':
            # A queued zram swap activation can otherwise run after swapoff.
            # Mask and stop the disposable guest's units, never the execution host's.
            config += '  - [systemctl, mask, --now, dev-zram0.swap, systemd-zram-setup@zram0.service]\n'
        config += '  - [swapoff, -a]\n'
        (self.root / 'user-data').write_text(config)
        # No early metadata hostname: cloud-init can process it before userdata's
        # preserve_hostname option is available. The final-stage command names it.
        (self.root / 'meta-data').write_text('instance-id: cross-' + self.role + '\n')
        seed = self.root / 'seed.iso'
        Q.run([Q.executable('genisoimage'), '-quiet', '-output', str(seed), '-volid', 'cidata', '-joliet', '-rock', str(self.root / 'user-data'), str(self.root / 'meta-data')], timeout=30)
        disk = self.root / 'guest.qcow2'
        Q.run([Q.executable('qemu-img'), 'create', '-q', '-f', 'qcow2', '-F', 'qcow2', '-b', str(image(self.family)), str(disk)], timeout=30)
        Q.run([Q.executable('qemu-img'), 'resize', str(disk), str(6 * 1024**3)], timeout=30)
        if json.loads(Q.run([Q.executable('qemu-img'), 'info', '--output=json', str(disk)]).stdout)['virtual-size'] != 6 * 1024**3:
            raise ValueError('guest disk cap not applied')
        net = f'user,id=net0,hostfwd=tcp:127.0.0.1:{self.port}-:22'
        if self.network is not None:
            net += ',net=' + self.network
        if self.data_port is not None:
            net += f',hostfwd=tcp:127.0.0.1:{self.data_port}-:7443'
        command = [Q.executable('qemu-system-x86_64'), '-accel', 'kvm' if self.acceleration == 'kvm' else 'tcg,thread=multi', '-cpu', 'host' if self.acceleration == 'kvm' else 'max', '-smp', '2', '-m', str(self.memory_mib),
                   '-drive', f'file={disk},if=virtio,format=qcow2', '-drive', f'file={seed},media=cdrom,readonly=on',
                   '-netdev', net, '-device', 'virtio-net-pci,netdev=net0', '-display', 'none',
                   '-serial', f'file:{self.results / (self.role + "-serial.txt")}', '-monitor', 'none']
        with (self.results / (self.role + '-qemu-stderr.txt')).open('wb') as stream:
            self.child = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=stream)
        self.command = command

    def ready(self, *, deadline=None):
        deadline = deadline if deadline is not None else time.monotonic() + Q.BOOT_TIMEOUT_S
        attempts = []
        while time.monotonic() < deadline:
            if self.child.poll() is not None:
                raise RuntimeError(self.role + ' guest stopped during boot')
            try:
                result = Q.run([*self.ssh, 'sudo', 'cloud-init', 'status', '--long'],
                               timeout=min(15, deadline - time.monotonic()), check=False)
                attempts.append('exit=' + str(result.returncode) + '\n' + result.stdout + result.stderr)
                (self.results / (self.role + '-bootstrap.txt')).write_text('\n'.join(attempts))
                if 'status: done' in result.stdout and result.returncode != 0:
                    try:
                        diagnostic = self.script('tail -n 200 /var/log/cloud-init.log\ncat /proc/swaps\n',
                                                 timeout=10, check=False)
                        (self.results / (self.role + '-bootstrap-diagnostic.txt')).write_text(
                            diagnostic.stdout + diagnostic.stderr)
                    except (OSError, subprocess.SubprocessError) as error:
                        (self.results / (self.role + '-bootstrap-diagnostic.txt')).write_text(str(error))
                    raise RuntimeError(self.role + ' cloud-init completed unsuccessfully: ' + result.stdout)
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
        try:
            facts = self.script('set -eu\n. /etc/os-release; echo os=$ID:$VERSION_ID; cat /proc/sys/kernel/random/boot_id; stat -fc %T /sys/fs/cgroup; cat /proc/swaps; test "$(wc -l < /proc/swaps)" = 1; getenforce 2>/dev/null || true\n').stdout
        except subprocess.CalledProcessError as error:
            (self.results / (self.role + '-guest.txt')).write_text((error.stdout or '') + (error.stderr or ''))
            raise
        (self.results / (self.role + '-guest.txt')).write_text(facts)
        expected = 'os=fedora:44' if self.family == 'fedora' else 'os=debian:13'
        if expected not in facts or 'cgroup2fs' not in facts or (self.family == 'fedora' and 'Enforcing' not in facts):
            raise RuntimeError(self.role + ' OS/cgroup/SELinux prerequsites unavailable')
        self.boot_id = facts.splitlines()[1]
        package_query = ("rpm -qa --qf '%{NAME} %{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\\n'\n"
                         if self.family == 'fedora' else
                         "dpkg-query -W -f='${binary:Package} ${Version} ${Architecture}\\n'\n")
        environment = self.script('set -eu\nuname -r\nsystemctl --version\nreadlink /proc/1/exe\n' + package_query)
        (self.results / (self.role + '-environment.txt')).write_text(environment.stdout + environment.stderr)

    def install(self, package, helpers):
        self.copy(package, '/home/accept/package.' + ('rpm' if self.family == 'fedora' else 'deb'))
        for helper in ('spool_dump', 'server_dump'):
            self.copy(helpers / helper, '/home/accept/' + helper)
        install = 'rpm -Uvh /home/accept/package.rpm' if self.family == 'fedora' else 'dpkg -i /home/accept/package.deb'
        command = 'set -eu\n' + install + '\ninstall -d -m 0755 /root/cross\ninstall -m 0700 /home/accept/spool_dump /home/accept/server_dump /root/cross/\nsha256sum /usr/bin/fabric-server /usr/bin/fabric-node /usr/bin/fabricctl\n'
        command += ("rpm -q --qf '%{NAME} %{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\\n' fabrico11y\n"
                    if self.family == 'fedora' else
                    "dpkg-query -W -f='${binary:Package} ${Version} ${Architecture}\\n' fabrico11y\n")
        result = self.script(command, timeout=120)
        (self.results / (self.role + '-install.txt')).write_text(result.stdout + result.stderr)

    def clock(self):
        return self.correlation()['monotonic_before_ns']

    def correlation(self):
        if self.clock_channel is not None:
            return self.clock_channel.correlation()
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

    def start_clock_channel(self):
        if self.clock_channel is not None:
            raise ValueError('clock channel already owned')
        spec = importlib.util.spec_from_file_location('cross_clock_channel', Path(__file__).with_name('clock_channel.py'))
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        self.clock_channel = module.ClockChannel(self)
        self.clock_channel.start()

    def stop_clock_channel(self):
        return self.clock_channel is None or self.clock_channel.close()

    def stop(self):
        if self.child is not None and self.child.poll() is None:
            self.child.terminate()
            try:
                self.child.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait(timeout=10)
        return self.child is None or self.child.poll() is not None
