"""G15 placement for Guest; provisioning and oracle behavior remain in Guest."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import select
import shlex
import subprocess
import time

if __package__:
    from .guest import Guest, Q
else:
    from guest import Guest, Q


def remote_command(host, arguments):
    if host != 'olo@olo-g15':
        raise ValueError('only the owner-selected G15 edge host is admitted')
    return ['ssh', '-T', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10',
            '-o', 'ServerAliveInterval=5', '-o', 'ServerAliveCountMax=3', host,
            shlex.join(list(map(str, arguments)))]


def placement_options(parser):
    parser.add_argument('--edge-host', choices=('olo@olo-g15',))
    parser.add_argument('--edge-storage-root', type=Path)
    parser.add_argument('--edge-source-root', type=Path)
    parser.add_argument('--edge-input-manifest', type=Path)


def validate_placement(args):
    values = (args.edge_host, args.edge_storage_root, args.edge_source_root, args.edge_input_manifest)
    if any(value is not None for value in values) and not all(value is not None for value in values):
        raise ValueError('remote edge placement requires host, storage, source and pinned manifest together')
    if args.edge_host and os.environ.get('FABRIC_RESOURCE_JOB_PROFILE') != 'split-server':
        raise ValueError('split cells require the registered Blade split-server launcher profile')


def edge_guest(args, family, root, results, key, data_port, *, network=None):
    validate_placement(args)
    if args.edge_host:
        return RemoteGuest(family, root, results, key, host=args.edge_host,
                           storage_root=args.edge_storage_root, source_root=args.edge_source_root,
                           manifest=args.edge_input_manifest, data_port=data_port,
                           run_id=args.run_id, network=network)
    return Guest('edge', family, root, results, key, 1536, network=network)


def completed_remote(response, receipt, *, run_id, boot_id, unit, command, transport_exit):
    """Both lifecycle and original launcher receipt must describe the same job."""
    child = receipt.get('command_receipt') if type(receipt) is dict else None
    return (type(response) is dict and type(receipt) is dict
            and response.get('event') == 'stopped' and response.get('run_id') == run_id
            and response.get('boot_id') == boot_id and response.get('qemu_stopped') is True
            and type(response.get('exit')) is int and response['exit'] == 0
            and type(transport_exit) is int and transport_exit == 0
            and receipt.get('unit') == unit and receipt.get('command') == command
            and type(receipt.get('exit')) is int and receipt['exit'] == 0
            and receipt.get('stop_confirmed') is True
            and type(child) is dict and child.get('unit') == unit
            and child.get('command') == command and child.get('state') == 'completed'
            and type(child.get('exit')) is int and child['exit'] == 0)


def shutdown_remote(receipt, *, unit, command):
    """A failed or incomplete command can still have confirmed whole-group shutdown."""
    return (type(receipt) is dict and receipt.get('unit') == unit
            and receipt.get('command') == command and receipt.get('stop_confirmed') is True)


class RemoteGuest(Guest):
    def __init__(self, family, root, results, key, *, host, storage_root,
                 source_root, manifest, data_port, run_id, network=None):
        manifest_sha256 = hashlib.sha256(manifest.read_bytes()).hexdigest()
        if storage_root != Path('/home/olo/.local/share/fabrico11y-release'):
            raise ValueError('unregistered G15 storage root')
        if (not source_root.is_relative_to(storage_root / 'inputs')
                or '..' in source_root.parts
                or not re.fullmatch(r'[a-z0-9][a-z0-9-]{0,79}', run_id)
                or not re.fullmatch(r'[0-9a-f]{64}', manifest_sha256)):
            raise ValueError('invalid pinned remote source/run')
        remote_command(host, ['true'])
        super().__init__('edge', family, root, results, key, 1536, network=network)
        self.host, self.storage_root, self.source_root = host, storage_root, source_root
        self.manifest_sha256, self.run_id = manifest_sha256, run_id
        self.manifest = json.loads(manifest.read_text())
        self.forward_data_port = data_port
        self.remote_root = storage_root / 'scratch' / ('remote-' + run_id)
        self.remote_results = storage_root / 'results' / ('remote-' + run_id)
        self.tunnel = None
        self.unit = None
        self.buffer = b''
        self.messages = []
        self.remote_boot_id = None
        self.remote_command = None
        self.remote_cgroup = None
        self.completion_confirmed = False
        self.shutdown_confirmed = False

    def message(self, deadline):
        while b'\n' not in self.buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([self.child.stdout], [], [], max(0, remaining))[0]:
                raise TimeoutError('remote lifecycle response deadline exceeded')
            block = os.read(self.child.stdout.fileno(), 8192)
            if not block:
                raise RuntimeError('remote lifecycle transport ended without required evidence')
            self.buffer += block
            if len(self.buffer) > 65536:
                raise ValueError('remote lifecycle response exceeds bound')
        line, self.buffer = self.buffer.split(b'\n', 1)
        value = json.loads(line)
        if type(value) is not dict:
            raise ValueError('remote lifecycle response is not an object')
        self.messages.append(value)
        (self.results / 'edge-remote-messages.json').write_text(json.dumps(self.messages, indent=2) + '\n')
        return value

    def start(self):
        # Private per-cell credentials never touch live host credentials/configuration.
        Q.run(remote_command(self.host, ['mkdir', '-m', '0700', str(self.remote_root)]), timeout=20)
        for path in (self.key, self.key.with_suffix('.pub')):
            Q.run(['scp', '-q', '-o', 'BatchMode=yes', str(path),
                   self.host + ':' + str(self.remote_root / path.name)], timeout=30)
        command = ['env', 'FABRIC_STORAGE_ROOT=' + str(self.storage_root),
                   'python3', '-B', self.source_root / 'tools/resource_group.py',
                   '--job-profile', 'edge', '--', 'python3', '-B',
                   self.source_root / 'tools/qualification/cross_family/remote_worker.py',
                   '--run-id', self.run_id, '--family', self.family,
                   '--manifest-sha256', self.manifest_sha256]
        if self.network:
            command += ['--network', self.network]
        self.remote_command = list(map(str, command[8:]))
        self.command = remote_command(self.host, command)
        with (self.results / 'edge-remote-stderr.txt').open('wb') as errors:
            self.child = subprocess.Popen(self.command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors)
        # Keep the guest's own deadline unchanged and allow its stopped receipt
        # to arrive before declaring a transport timeout.
        deadline = time.monotonic() + Q.BOOT_TIMEOUT_S + 30
        started = self.message(deadline)
        if (started.get('event') != 'started' or started.get('run_id') != self.run_id
                or not re.fullmatch(r'fabric-work-[0-9a-f]{32}', started.get('unit', ''))):
            raise ValueError('wrong remote run/service identity')
        self.unit = started['unit']
        group = Path(started.get('cgroup', ''))
        if (not group.is_absolute() or not str(group).startswith('/sys/fs/cgroup/')
                or '..' in group.parts or group.name != self.unit + '.service'):
            raise ValueError('remote service did not supply its actual cgroup identity')
        self.remote_cgroup = group
        ready = self.message(deadline)
        if (ready.get('event') != 'ready' or type(ready.get('port')) is not int
                or not 1 <= ready['port'] <= 65535
                or not re.fullmatch(r'[0-9a-f-]{36}', ready.get('boot_id', ''))):
            raise ValueError('remote guest did not supply its boot/port identity')
        self.remote_boot_id = ready['boot_id']
        forwards = ['-N', '-o', 'ExitOnForwardFailure=yes',
                    '-L', f'127.0.0.1:{self.port}:127.0.0.1:{ready["port"]}',
                    '-R', f'127.0.0.1:{self.forward_data_port}:127.0.0.1:{self.forward_data_port}']
        tunnel_command = remote_command(self.host, [])[:-1]
        tunnel_command[-1:-1] = forwards
        with (self.results / 'edge-forward-stderr.txt').open('wb') as errors:
            self.tunnel = subprocess.Popen(tunnel_command, stdin=subprocess.DEVNULL,
                                           stdout=subprocess.DEVNULL, stderr=errors)
        self.forward_command = tunnel_command

    def ready(self, *, deadline=None):
        super().ready(deadline=deadline)
        if self.boot_id != self.remote_boot_id:
            raise ValueError('forwarded guest boot identity differs from remote capture')
        if self.tunnel.poll() is not None:
            raise RuntimeError('owned data/management forward exited')
        (self.results / 'edge-placement.json').write_text(json.dumps(dict(
            host=self.host, storage_root=str(self.storage_root),
            source_root=str(self.source_root), manifest_sha256=self.manifest_sha256,
            worker_command=self.command, forward_command=self.forward_command,
            boot_id=self.boot_id, clock=self.correlation()), indent=2) + '\n')

    def copy(self, path, destination):
        # Immutable packages/helpers are staged once by content digest. Fresh
        # fixture files keep Guest's ordinary SCP path over the management forward.
        digest = hashlib.sha256(Path(path).read_bytes()).hexdigest()
        staged = self.manifest.get('objects', {}).get(digest)
        if staged is None:
            return super().copy(path, destination)
        request = dict(action='copy', path=str(self.storage_root / staged),
                       sha256=digest, destination=destination)
        self.child.stdin.write((json.dumps(request) + '\n').encode())
        self.child.stdin.flush()
        response = self.message(time.monotonic() + 60)
        if response != dict(event='copied', sha256=digest, destination=destination):
            raise ValueError('remote staged copy lacks matching receipt')

    def stop(self):
        if self.child is None:
            return True
        self.completion_confirmed = False
        self.shutdown_confirmed = False
        errors = []
        value = None
        response = next((m for m in reversed(self.messages) if m.get('event') == 'stopped'), None)
        try:
            if self.child.poll() is None and response is None:
                try:
                    self.child.stdin.write(b'{"action":"stop"}\n')
                    self.child.stdin.flush()
                    response = self.message(time.monotonic() + 30)
                except (OSError, ValueError, RuntimeError) as error:
                    errors.append(str(error))
            if response is not None and self.child.poll() is None:
                self.child.wait(timeout=20)
        except (OSError, subprocess.SubprocessError) as error:
            errors.append(str(error))
        finally:
            if self.unit:
                # This independent management connection also handles loss of
                # the original SSH pipe. Never infer stop from transport exit.
                try:
                    stopped = Q.run(remote_command(self.host, ['systemctl', '--user', 'stop', self.unit]),
                                    timeout=20, check=False)
                    self.management_stop_exit = stopped.returncode
                except (OSError, subprocess.SubprocessError) as error:
                    errors.append(str(error))
            if self.child.poll() is None:
                try:
                    self.child.stdin.close()
                    self.child.wait(timeout=20)
                except (OSError, subprocess.SubprocessError) as error:
                    errors.append(str(error))
                    self.child.terminate()
                    try:
                        self.child.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        self.child.kill()
                        self.child.wait(timeout=10)
        try:
            if self.unit:
                receipt_path = self.source_root / 'target/resource-containment/runs' / (self.unit + '.json')
                receipt = Q.run(remote_command(self.host, ['cat', receipt_path]), timeout=20, check=False)
                if receipt.returncode == 0:
                    (self.results / 'edge-resource-receipt.json').write_text(receipt.stdout)
                    try:
                        value = json.loads(receipt.stdout)
                    except ValueError as error:
                        errors.append('partial original resource receipt: ' + str(error))
                    self.shutdown_confirmed = shutdown_remote(value, unit=self.unit, command=self.remote_command)
                    self.completion_confirmed = completed_remote(response, value, run_id=self.run_id,
                        boot_id=self.remote_boot_id, unit=self.unit, command=self.remote_command,
                        transport_exit=self.child.returncode)
                else:
                    errors.append('original remote resource receipt unavailable: ' + receipt.stderr)
                if self.remote_cgroup is not None:
                    probe = ('import json,sys; from pathlib import Path; p=Path(sys.argv[1]); '
                             'exists=p.exists(); events=(p/"cgroup.events").read_text() if exists else ""; '
                             'print(json.dumps(dict(path=str(p),exists=exists,events=events,'
                             'stopped=not exists or "populated 0" in events.splitlines())))')
                    result = Q.run(remote_command(self.host, ['python3', '-B', '-c', probe, self.remote_cgroup]), timeout=20)
                    kernel = json.loads(result.stdout)
                    (self.results / 'edge-kernel-stop.json').write_text(json.dumps(kernel, indent=2) + '\n')
                    if kernel.get('path') == str(self.remote_cgroup) and kernel.get('stopped') is True:
                        self.shutdown_confirmed = True
                Q.run(['scp', '-q', '-r', '-o', 'BatchMode=yes',
                       self.host + ':' + str(self.remote_results),
                       str(self.results / 'edge-remote-evidence')], timeout=60)
        except (OSError, ValueError, subprocess.SubprocessError, RuntimeError) as error:
            errors.append(str(error))
            self.completion_confirmed = False
        finally:
            if self.tunnel is not None and self.tunnel.poll() is None:
                self.tunnel.terminate()
                try:
                    self.tunnel.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    self.tunnel.kill()
                    self.tunnel.wait(timeout=10)
            (self.results / 'edge-shutdown.json').write_text(json.dumps(dict(
                unit=self.unit, command=self.remote_command, transport_exit=self.child.returncode,
                original_command_receipt=value, completion_confirmed=self.completion_confirmed,
                shutdown_confirmed=self.shutdown_confirmed, errors=errors), indent=2) + '\n')
        return self.shutdown_confirmed
