"""Exact-package production fixture shared by the registered local R3 cells.

The owner enrolls with a real browser WebAuthn ceremony. A separate scoped
workload principal performs benchmark reads; the visible UI remains active.
"""
from __future__ import annotations
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.request

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE / 'cross_family'))
from inputs import validate_inputs
sys.path.insert(0, str(ROOT / 'tools/bench/labs/completion'))
import cgroups
sys.path.insert(0, str(ROOT / 'tools/packaging'))
from stage_candidate import stage_candidate, digest
from console_bridge import ConsoleBridge, STORAGE, require_limits
from delivery_faults import free_port, openssl


def directory_bytes(root):
    return sum(p.stat().st_size for p in root.rglob('*') if p.is_file() and not p.is_symlink())


def process_kib(pid, name='VmRSS'):
    for line in Path(f'/proc/{pid}/status').read_text().splitlines():
        if line.startswith(name + ':'):
            return int(line.split()[1])
    raise RuntimeError('missing process memory observation')


class WorkloadReader:
    def __init__(self, origin, context, token):
        self.origin, self.context, self.token = origin, context, token
        self.observations = []

    def call(self, path, body=None, method=None):
        began = time.monotonic_ns()
        request = urllib.request.Request(self.origin + path,
                data=None if body is None else json.dumps(body).encode(), method=method,
                headers={'authorization': 'Bearer ' + self.token,
                         'x-fabric-client-version': '1', 'content-type': 'application/json'})
        with urllib.request.urlopen(request, context=self.context, timeout=15) as response:
            raw = response.read(8 * 1024**2 + 1)
            if len(raw) > 8 * 1024**2:
                raise RuntimeError('bounded console response exceeded 8 MiB')
            value = json.loads(raw)
        self.observations.append({'path': path, 'started_ns': began,
                                  'finished_ns': time.monotonic_ns(), 'bytes': len(raw)})
        if len(self.observations) > 10000:
            raise RuntimeError('bounded reader receipt exceeded 10000 requests')
        return value

    def query(self, body):
        return self.call('/v1/console/query', body, 'POST')

    def pages(self, body):
        began = time.monotonic()
        query, pages, size = dict(body), [], 0
        for _ in range(1000):
            page = self.query(query)
            size += len(json.dumps(page))
            if size > 16 * 1024**2:
                raise RuntimeError('query pagination exceeds registered fixture reply allowance')
            pages.append(page)
            if not page.get('next_page'):
                return time.monotonic() - began, pages
            query['page'] = page['next_page']
        raise RuntimeError('query exceeded finite page bound')


class CandidateFixture:
    def __init__(self, deb_receipt, rpm_receipt, out, *, mode='segment'):
        self.parent = require_limits()
        if mode not in ('segment', 'journal'):
            raise ValueError('unknown registered storage mode')
        subprocess.run(['systemctl', '--user', 'set-property', '--runtime', self.parent.name,
                        'MemoryMax=6G', 'MemoryHigh=5G', 'MemorySwapMax=0', 'TasksMax=1536'], check=True)
        if int((self.parent / 'memory.max').read_text()) != 6 * 1024**3:
            raise RuntimeError('whole-fixture cap not enforced')
        self.out = Path(out)
        if (not self.out.is_absolute() or self.out.exists() or self.out.is_symlink()
                or not self.out.parent.resolve(strict=True).is_relative_to(STORAGE / 'results')
                or not re.fullmatch('[a-z0-9][a-z0-9-]{0,79}', self.out.name)):
            raise ValueError('fresh data-drive result path required')
        # Allocated-byte admission includes hard-linked caches once, like du.
        allocated = int(subprocess.check_output(['du', '-s', '-B1', str(STORAGE)]).split()[0])
        if allocated + 5 * 1024**3 > 100_000_000_000:
            raise RuntimeError('5 GiB fixture reservation exceeds total laboratory disk budget')
        self.artifacts = validate_inputs(Path(deb_receipt), Path(rpm_receipt))
        self.out.mkdir(mode=0o700)
        self.work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / ('release-' + self.out.name)
        self.work.mkdir(mode=0o700)
        self.processes, self.logs, self.groups = [], [], []
        self.server = self.bridge = None
        self.closed = False
        self.receipt = {'state': 'starting', 'passed': False, 'inputs': self.artifacts['identity'],
                        'storage_allocated_before': allocated, 'reserved_bytes': 5 * 1024**3,
                        'mode': mode, 'processes': [], 'checks': [],
                        'host': {'uname': list(os.uname()), 'cpus': os.cpu_count(),
                                 'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip()}}
        try:
            self.parent = cgroups.delegate()
            cgroups.set_limits(self.parent / 'supervisor', 2 * 1024**3, 1700 * 1024**2, 512, 4)
            self.server_group, limits = cgroups.subgroup(self.parent, 'server', 4_000_000_000, 3_000_000_000, 512, 2)
            self.browser_group, _ = cgroups.subgroup(self.parent, 'browser', 1024**3, 850 * 1024**2, 512, 2)
            self.edge_group, _ = cgroups.subgroup(self.parent, 'edge', 128 * 1024**2, 96 * 1024**2, 128, 1)
            self.groups = [self.server_group, self.browser_group, self.edge_group]
            self.receipt['server_limits'] = limits
            if not {0, 1, 2, 3}.issubset(os.sched_getaffinity(0)):
                raise RuntimeError('registered four distinct logical CPUs unavailable')
            os.sched_setaffinity(0, set(os.sched_getaffinity(0)) - {0, 1})
            payload = self.work / 'payload'
            staged = stage_candidate(self.artifacts['debian'], Path(deb_receipt), payload)
            self.receipt['staged'] = staged
            self.bins = payload / 'usr/bin'
            self.console = payload / 'usr/share/fabrico11y/console'
            self.helpers = self.work / 'helpers'
            self.helpers.mkdir()
            for source in self.artifacts['helpers'].iterdir():
                if source.is_file() and not source.is_symlink():
                    shutil.copy2(source, self.helpers / source.name)
            self.input_hashes = {str(p.relative_to(self.work)): digest(p)
                                for directory in [self.bins, self.console, self.helpers]
                                for p in directory.rglob('*') if p.is_file()}
            self._certificates()
            port = free_port()
            self.origin = f'https://localhost:{port}'
            self.context = ssl.create_default_context(cafile=str(self.work / 'ca.pem'))
            secret = self.work / 'unused-legacy-token'
            secret.write_text(secrets.token_hex(32) + '\n'); secret.chmod(0o600)
            self.config = self.work / 'server.conf'
            self.config.write_text(
                f'listen=127.0.0.1:{port}\ntls_cert={self.work}/server.pem\ntls_key={self.work}/server.key\n'
                f'state_dir={self.work}/state\nadmin_token_file={secret}\nconsole_dir={self.console}\n'
                f'access_origin={self.origin}\naccess_rp_id=localhost\nself_spindle_ca={self.work}/ca.pem\n'
                f'journal_bytes=4294967296\njournal_file_bytes={16*1024**2 if mode == "segment" else 2*1024**3}\n'
                'retention_bytes=100000000000\nretention_s=86400\n')
            self.start_server()
            self.bridge = ConsoleBridge(self.origin, self.work / 'server.pem', self.work / 'browser',
                                        self.out / 'browser', browser_group=self.browser_group)
            self.owner = self.bridge.bootstrap_owner(self.work / 'state/access/access-bootstrap.secret')
            self.receipt['state'] = 'ready'
        except BaseException:
            self.close()
            raise

    def _certificates(self):
        work = self.work
        (work / 'san.ext').write_text('subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n')
        openssl(work, 'req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:P-256', '-nodes', '-keyout', 'ca.key', '-out', 'ca.pem', '-days', '1', '-subj', '/CN=Owned release fixture CA', '-addext', 'basicConstraints=critical,CA:TRUE', '-addext', 'keyUsage=critical,keyCertSign,cRLSign')
        openssl(work, 'req', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:P-256', '-nodes', '-keyout', 'server.key', '-out', 'server.csr', '-subj', '/CN=localhost')
        openssl(work, 'x509', '-req', '-in', 'server.csr', '-CA', 'ca.pem', '-CAkey', 'ca.key', '-CAcreateserial', '-out', 'server.pem', '-days', '1', '-extfile', 'san.ext')
        for name in ('ca.key', 'server.key'):
            (work / name).chmod(0o600)

    def spawn(self, command, label, group=None, cpus='2-3'):
        if group is not None:
            command = [sys.executable, '-B', str(ROOT / 'tools/bench/labs/completion/enter_group.py'), str(group), *map(str, command)]
        command = ['taskset', '-c', cpus, *map(str, command)]
        stdout = self.out / (label + '.out')
        stderr = self.out / (label + '.err')
        out, err = stdout.open('wb'), stderr.open('wb')
        self.logs.extend([out, err])
        child = subprocess.Popen(command, stdout=out, stderr=err, start_new_session=True)
        self.processes.append(child)
        self.receipt['processes'].append({'label': label, 'command': command, 'pid': child.pid})
        return child

    def start_server(self):
        self.server = self.spawn([self.bins / 'fabric-server', 'serve', self.config, '--timing-events'],
                                 'server-' + str(len(self.processes)), self.server_group, '0-1')
        deadline = time.monotonic() + 20
        while True:
            try:
                with urllib.request.urlopen(self.origin + '/console/index.html', context=self.context, timeout=2) as response:
                    if response.status == 200:
                        return
            except (OSError, urllib.error.URLError):
                pass
            if self.server.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError('exact production candidate did not start')
            time.sleep(.1)

    def stop_server(self):
        if self.server is not None and self.server.poll() is None:
            self.server.send_signal(signal.SIGTERM)
            if self.server.wait(timeout=60) != 0:
                raise RuntimeError('production server did not exit cleanly')
        if (self.server_group / 'cgroup.procs').read_text().strip():
            raise RuntimeError('server/companion remain alive after shutdown')

    def restart(self):
        self.bridge.poll_ui(False)
        self.stop_server()
        self.start_server()
        self.owner = self.bridge.login()

    def enroll(self, name, logs=()):
        status, value = self.bridge.request('/v1/console/nodes', 'POST',
                      {'name': name, 'logs': list(map(str, logs)), 'metric_interval_s': 15})
        if status != 200 or not value.get('token'):
            raise RuntimeError('production source enrollment failed')
        return value

    def reader(self, enrollments, *, ttl_s=1800):
        scope = dict(self.owner['scope'])
        scope.update(actions=['telemetry_read', 'inventory_read'], installation_wide=False,
                     enrollments=list(enrollments), signals=['logs', 'metrics', 'traces'],
                     max_query_rows=1000, max_query_window_s=86400, allowed_log_paths=[],
                     enrollment_namespace=None, max_enrollments=0)
        status, value = self.bridge.issue_workload('release-benchmark-reader', scope, ttl_s)
        if status != 200:
            raise RuntimeError('scoped workload enrollment failed')
        return WorkloadReader(self.origin, self.context, value['token'])

    def resource_sample(self):
        live = directory_bytes(self.work) + directory_bytes(self.out)
        if live > 5 * 1024**3:
            raise RuntimeError('registered live fixture disk budget exceeded')
        if directory_bytes(self.out) > 50 * 1024**2:
            raise RuntimeError('compact evidence exceeds registered 50 MiB budget')
        return {'monotonic_ns': time.monotonic_ns(), 'live_bytes': live,
                'server_rss_kib': process_kib(self.server.pid),
                'server_hwm_kib': process_kib(self.server.pid, 'VmHWM'),
                'groups': cgroups.snapshot(self.parent)}

    def close(self):
        if self.closed:
            return
        self.closed = True
        cleanup = True
        if self.bridge is not None:
            try:
                self.bridge.close()
            except Exception as error:
                self.receipt['browser_cleanup_error'] = type(error).__name__
                cleanup = False
        for child in reversed(self.processes):
            if child.poll() is None:
                try:
                    os.killpg(child.pid, signal.SIGTERM)
                    child.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL); child.wait(timeout=10)
                    cleanup = False
                except ProcessLookupError:
                    pass
        for log in self.logs:
            log.close()
        self.receipt['resource_final'] = cgroups.snapshot(self.parent)
        for group in self.groups:
            if group.exists() and 'populated 0' not in (group / 'cgroup.events').read_text().splitlines():
                cleanup = False
        if hasattr(self, 'input_hashes'):
            unchanged = all((self.work / name).is_file() and digest(self.work / name) == expected
                            for name, expected in self.input_hashes.items())
            self.receipt['immutable_inputs'] = unchanged
            cleanup = cleanup and unchanged
        self.receipt['cleanup_confirmed'] = cleanup
        self.receipt['state'] = 'closed' if cleanup else 'cleanup_failed'
        # Failed fixtures remain private (0700) on the data drive for diagnosis.
        # They contain ephemeral secrets and are never repository/wiki inputs.
        if cleanup and self.receipt.get('passed'):
            shutil.rmtree(self.work)
        self.receipt['scratch_removed'] = not self.work.exists()
        self.receipt['scratch_retained'] = str(self.work) if self.work.exists() else None
        (self.out / 'runtime.json').write_text(json.dumps(self.receipt, indent=2) + '\n')
        if not cleanup:
            raise RuntimeError('candidate fixture cleanup or frozen input identity failed')

    def __enter__(self):
        return self

    def __exit__(self, error_type, error, traceback):
        if error_type is not None:
            self.receipt['passed'] = False
            self.receipt['failure_type'] = error_type.__name__
        self.close()
