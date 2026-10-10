#!/usr/bin/env python3
"""Finite native companion/lifecycle investigation, not deployment qualification.

Run through resource_group.py --delegate. Protocol:
docs/experiments/formal/server-self-observation-protocol.md
"""
import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tools/bench/labs/completion'))
import cgroups
from resource_group import require_limits


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def live(pid):
    try:
        return Path(f'/proc/{pid}/stat').read_text().rpartition(') ')[2][0] != 'Z'
    except FileNotFoundError:
        return False


def exact(rows, expected):
    """Independent body/count check; a dropped, changed or repeated row fails."""
    return Counter(row['body'] for row in rows) == Counter(expected)


class Trial:
    def __init__(self, work, group, bins, report):
        self.work, self.group, self.bins, self.report = work, group, bins, report
        self.deadline = time.monotonic() + 280
        self.server, self.child = None, None
        self.generation = 0
        self.admin = os.urandom(32).hex()
        self.samples = report['samples'] = []
        self.exits = report['exits'] = []

    def remaining(self, cap=5):
        left = min(cap, self.deadline - time.monotonic())
        if left <= 0:
            raise TimeoutError('finite smoke deadline reached')
        return left

    def sample(self):
        size = sum(p.stat().st_size for p in self.work.rglob('*') if p.is_file())
        if size > 128 * 2**20:
            raise RuntimeError('128 MiB fixture cap exceeded')
        processes = {}
        for name, pid in [('server', self.server.pid if self.server else None), ('spindle', self.child)]:
            if pid and live(pid):
                status = dict(line.split(':', 1) for line in Path(f'/proc/{pid}/status').read_text().splitlines() if ':' in line)
                stat = Path(f'/proc/{pid}/stat').read_text().rpartition(') ')[2].split()
                processes[name] = dict(pid=pid, rss_bytes=int(status['VmRSS'].split()[0])*1024,
                    hwm_bytes=int(status['VmHWM'].split()[0])*1024,
                    cpu_s=(int(stat[11])+int(stat[12]))/os.sysconf('SC_CLK_TCK'))
        self.samples.append(dict(time_ns=time.time_ns(), fixture_bytes=size, processes=processes,
            cgroup={k: (self.group/k).read_text().strip() for k in
                    ('memory.current', 'memory.peak', 'memory.events',
                     'memory.swap.current', 'cpu.stat', 'io.stat', 'pids.current')}))

    def setup(self):
        commands = [
            ['req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:P-256', '-nodes', '-keyout', 'ca.key', '-out', 'ca.pem', '-days', '2', '-subj', '/CN=fabric test CA', '-addext', 'basicConstraints=critical,CA:TRUE', '-addext', 'keyUsage=critical,keyCertSign,cRLSign'],
            ['req', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:P-256', '-nodes', '-keyout', 'server.key', '-out', 'server.csr', '-subj', '/CN=127.0.0.1'],
            ['x509', '-req', '-in', 'server.csr', '-CA', 'ca.pem', '-CAkey', 'ca.key', '-CAcreateserial', '-out', 'server.pem', '-days', '2', '-extfile', 'san.ext']]
        (self.work/'san.ext').write_text('subjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n')
        for cmd in commands:
            subprocess.run(['openssl', *cmd], cwd=self.work, check=True,
                           capture_output=True, timeout=self.remaining(8))
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            self.port = sock.getsockname()[1]
        (self.work/'admin').write_text(self.admin)
        self.conf = self.work/'server.conf'
        self.conf.write_text(f'listen=127.0.0.1:{self.port}\ntls_cert={self.work}/server.pem\ntls_key={self.work}/server.key\nstate_dir={self.work}/state\nadmin_token_file={self.work}/admin\njournal_bytes=8388608\njournal_file_bytes=1048576\nretention_s=86400\nretention_bytes=8388608\nseal_workers=1\nself_spindle_ca={self.work}/ca.pem\n')
        self.ctx = ssl.create_default_context(cafile=str(self.work/'ca.pem'))

    def http(self, path, body=None, method=None):
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(f'https://127.0.0.1:{self.port}'+path,
            data=data, method=method, headers={'authorization': 'Bearer '+self.admin,
                                             'content-type': 'application/json'})
        with urllib.request.urlopen(request, context=self.ctx, timeout=self.remaining()) as response:
            raw = response.read(2**20+1)
            if len(raw) > 2**20:
                raise RuntimeError('response size cap exceeded')
            return json.loads(raw)

    def spawn(self, config=None):
        self.generation += 1
        self.err = self.work/f'server-{self.generation}.err'
        with self.err.open('wb') as err:
            return subprocess.Popen([sys.executable, '-B', str(Path(__file__).resolve()),
                '--enter', str(self.group), str(self.bins/'fabric-server'), 'serve',
                str(config or self.conf)], stdout=subprocess.DEVNULL, stderr=err)

    def start(self):
        self.server = self.spawn()
        until = time.monotonic()+self.remaining(12)
        while time.monotonic() < until:
            if self.server.poll() is not None:
                raise RuntimeError(f'early server exit: {self.err.read_text()}')
            match = re.search(r'dedicated spindle pid=(\d+)', self.err.read_text())
            if match:
                self.child = int(match[1])
                server_group = Path(f'/proc/{self.server.pid}/cgroup').read_text()
                assert Path(f'/proc/{self.child}/cgroup').read_text() == server_group
                assert str(self.group).removeprefix('/sys/fs/cgroup') in server_group
                self.http('/v1/admin/nodes')
                self.sample()
                return
            time.sleep(.05)
        raise TimeoutError('listener/companion startup')

    def query(self, contains):
        response = self.http('/v1/admin/query', dict(kind='logs', from_ns=0,
            to_ns=time.time_ns()+10**9, contains=contains, limit=1000))
        assert response.get('next_page') is None, 'unexpected pagination'
        assert response['complete'], 'incomplete diagnostic query'
        return response['rows']

    def logs(self):
        paths = [self.work/'state/diagnostics/server.log',
                 self.work/'state/self-spindle/spool/diagnostics/spindle.log']
        # Snapshot only current retained lines. Future periodic samples may arrive
        # later; predicate by exact body when comparing this finite prefix.
        until = time.monotonic()+self.remaining(30)
        while time.monotonic() < until:
            if all(p.exists() and 'event=process_sample' in p.read_text() for p in paths):
                break
            time.sleep(.2)
        expected = [line for p in paths for line in p.read_text().splitlines()]
        for component in ('server', 'spindle'):
            assert any(f'component={component}' in line and 'event=process_sample' in line for line in expected)
        while time.monotonic() < until:
            rows = self.query('component=')
            selected = [r for r in rows if r['body'] in expected]
            if exact(selected, expected):
                assert not exact(selected[:-1], expected), 'drop control was accepted'
                corrupt = [dict(row) for row in selected]
                corrupt[0]['body'] += ' CORRUPTED'
                assert not exact(corrupt, expected), 'changed-body control was accepted'
                assert not exact(selected+[selected[0]], expected), 'duplicate control was accepted'
                assert self.query('nonexistent-diagnostic-negative-control') == []
                self.report.setdefault('log_checks', []).append(dict(
                    generation=self.generation, expected=expected, rows=selected,
                    negative_controls_rejected=3, absent_query_empty=True))
                self.sample()
                return
            time.sleep(.25)
        raise AssertionError('diagnostic prefix was not delivered exactly')

    def stop(self, kind='normal'):
        self.server.send_signal(signal.SIGKILL if kind == 'abrupt' else signal.SIGTERM)
        code = self.server.wait(timeout=self.remaining(25))
        self.exits.append(dict(generation=self.generation, kind=kind, code=code))
        assert code == (-signal.SIGKILL if kind == 'abrupt' else 0)
        self.wait_child()
        self.server = None

    def wait_child(self):
        until = time.monotonic()+self.remaining(15)
        while live(self.child) and time.monotonic() < until:
            time.sleep(.05)
        assert not live(self.child), 'live orphan companion'
        self.child = None
        self.sample()

    def run(self):
        self.setup()
        self.start()
        self.logs()
        # Cross an actual timer boundary. Querying one's diagnostics must not
        # generate a per-query/per-ACK stream that recursively logs itself.
        paths = [self.work/'state/diagnostics/server.log',
                 self.work/'state/self-spindle/spool/diagnostics/spindle.log']
        before = [p.read_text().splitlines() for p in paths]
        until = time.monotonic()+16
        while time.monotonic() < until:
            self.remaining()
            self.query('component=')
            self.sample()
            time.sleep(.5)
        after = [p.read_text().splitlines() for p in paths]
        increments = [sum('event=process_sample' in x for x in a)-sum('event=process_sample' in x for x in b)
                      for a, b in zip(after, before)]
        assert all(1 <= n <= 2 for n in increments)
        assert all(len(a)-len(b) <= 6 for a, b in zip(after, before)), 'diagnostic feedback during quiet queries'
        self.report['quiet_timer_sample_increments'] = increments
        self.logs()
        token = self.work/'state/self-spindle/token'
        identity = self.work/'state/self-spindle/spool/identity'
        original = sha(token), sha(identity)
        assert token.read_text() != self.admin
        assert token.stat().st_mode & 0o077 == 0
        # A second supervisor must fail while the original remains available.
        duplicate = self.spawn()
        code = duplicate.wait(timeout=self.remaining(8))
        assert code != 0 and self.server.poll() is None and live(self.child)
        self.report['duplicate_exit'] = code
        self.http('/v1/admin/nodes/fabric-server-self/config',
                  dict(logs=[], metric_interval_s=15), method='PUT')
        self.stop()
        self.start()
        assert (sha(token), sha(identity)) == original
        self.logs()
        self.report['identity_reused'] = True
        # Unexpected collector death is a supervisor failure, not silent health.
        os.kill(self.child, signal.SIGKILL)
        code = self.server.wait(timeout=self.remaining(20))
        self.exits.append(dict(generation=self.generation, kind='child-killed', code=code))
        assert code != 0
        self.wait_child()
        self.server = None
        self.start()
        self.stop('abrupt')
        # Failure before listener startup must never report a launched child.
        for name, old, new in [('missing-executable', '', f'self_spindle_executable={self.work}/absent\n'),
                               ('invalid-tls', f'tls_cert={self.work}/server.pem', f'tls_cert={self.work}/absent.pem')]:
            conf = self.work/(name+'.conf')
            text = self.conf.read_text()
            conf.write_text(text+new if not old else text.replace(old, new))
            failed = self.spawn(conf)
            code = failed.wait(timeout=self.remaining(8))
            assert code != 0 and 'dedicated spindle pid=' not in self.err.read_text()
            self.report.setdefault('startup_failures', []).append(dict(case=name, code=code))
        # Valid CA and certificate, wrong SAN: PEM parsing alone cannot detect
        # this. The companion's first authenticated request must fail startup.
        (self.work/'wrong.ext').write_text('subjectAltName=IP:127.0.0.2\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n')
        subprocess.run(['openssl', 'x509', '-req', '-in', 'server.csr', '-CA', 'ca.pem',
            '-CAkey', 'ca.key', '-CAcreateserial', '-out', 'wrong.pem', '-days', '2',
            '-extfile', 'wrong.ext'], cwd=self.work, capture_output=True, check=True,
            timeout=self.remaining(8))
        conf = self.work/'wrong-san.conf'
        conf.write_text(self.conf.read_text().replace('tls_cert='+str(self.work/'server.pem'),
                                                    'tls_cert='+str(self.work/'wrong.pem')))
        self.server = self.spawn(conf)
        code = self.server.wait(timeout=self.remaining(20))
        assert code != 0
        match = re.search(r'dedicated spindle pid=(\d+)', self.err.read_text())
        assert match, 'wrong-SAN case never exercised the actual child TLS client'
        self.child = int(match[1])
        self.wait_child()
        self.server = None
        self.report['startup_failures'].append(dict(case='wrong-san', code=code))
        # The documented self-signed default uses tls_cert itself as the local
        # trust anchor; exercise it without any companion CA override.
        subprocess.run(['openssl', 'x509', '-req', '-in', 'server.csr', '-signkey', 'server.key',
            '-out', 'self.pem', '-days', '2', '-extfile', 'san.ext'], cwd=self.work,
            capture_output=True, check=True, timeout=self.remaining(8))
        original_conf, original_ctx = self.conf, self.ctx
        self.conf = self.work/'self-signed.conf'
        self.conf.write_text(original_conf.read_text().replace(
            'tls_cert='+str(self.work/'server.pem'), 'tls_cert='+str(self.work/'self.pem')).replace(
            f'self_spindle_ca={self.work}/ca.pem\n', ''))
        self.ctx = ssl.create_default_context(cafile=str(self.work/'self.pem'))
        self.start()
        self.logs()
        self.stop()
        self.report['self_signed_default_delivered'] = True
        self.conf, self.ctx = original_conf, original_ctx
        # Wrong/missing bootstrap secrets cannot silently enroll a replacement.
        control = self.work/'state/control.json'
        control_before, token_before = sha(control), token.read_bytes()
        for name in ('wrong-token', 'missing-token'):
            if name == 'wrong-token':
                token.write_text('a'*64 if token_before != b'a'*64 else 'b'*64)
            else:
                token.unlink()
            failed = self.spawn()
            code = failed.wait(timeout=self.remaining(8))
            assert code != 0 and 'dedicated spindle pid=' not in self.err.read_text()
            assert sha(control) == control_before
            assert not token.exists() if name == 'missing-token' else token.read_text() != token_before.decode()
            token.write_bytes(token_before)
            token.chmod(0o600)
            self.report['startup_failures'].append(dict(case=name, code=code))
        self.start()
        self.http('/v1/admin/nodes/fabric-server-self/revoke', {}, method='POST')
        self.stop()
        revoked = sha(control)
        failed = self.spawn()
        code = failed.wait(timeout=self.remaining(8))
        assert code != 0 and sha(control) == revoked and token.read_bytes() == token_before
        self.report['startup_failures'].append(dict(case='revoked-token', code=code))
        assert not (self.group/'cgroup.procs').read_text().strip()
        self.report['no_live_orphans'] = True
        self.sample()

    def cleanup(self):
        # Own descendant group only. This also handles a test failing mid-start.
        (self.group/'cgroup.kill').write_text('1')
        if self.server:
            self.server.wait(timeout=10)


def main():
    if sys.argv[1:2] == ['--enter']:
        cgroups.enter(sys.argv[2])
        os.execvp(sys.argv[3], sys.argv[3:])
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    require_limits()
    args.out.mkdir(parents=True, exist_ok=False)
    parent = cgroups.delegate()
    group, limits = cgroups.subgroup(parent, 'self-observation', 3999997952,
                                      3200000000, 256, 2)
    work = Path(tempfile.mkdtemp(prefix='self-', dir=os.environ['TMPDIR']))
    bins = Path(os.environ['CARGO_TARGET_DIR'])/'debug'
    report = dict(command=sys.argv, scratch=str(work), limits=limits,
        source_sha256={str(p.relative_to(ROOT)): sha(p)
            for base in ('src', 'crates') for p in (ROOT/base).rglob('*.rs')},
        harness_sha256={name: sha(ROOT/name) for name in
            ('tools/self_observation_smoke.py', 'tools/bench/labs/completion/cgroups.py',
             'tools/resource_group.py', 'docs/experiments/formal/server-self-observation-protocol.md',
             'Cargo.toml', 'Cargo.lock', 'crates/fabric-server/Cargo.toml')},
        binary_sha256={name: sha(bins/name) for name in ('fabric-server', 'fabric-node')})
    trial = Trial(work, group, bins, report)
    failed = False
    try:
        trial.run()
        report['status'] = 'passed'
    except BaseException as error:
        failed = True
        report.update(status='failed', error=repr(error))
        raise
    finally:
        trial.cleanup()
        report['cgroup'] = cgroups.snapshot(parent)
        for path in work.glob('*.err'):
            shutil.copyfile(path, args.out/path.name)
        if failed:
            # Retain private fixtures on the data drive; never publish secrets.
            report['failure_scratch_retained'] = str(work)
        else:
            shutil.rmtree(work)
        report['scratch_removed'] = not work.exists()
        group.rmdir()
        report['cgroup_removed'] = not group.exists()
        (args.out/'result.json').write_text(json.dumps(report, indent=2)+'\n')


if __name__ == '__main__':
    main()
