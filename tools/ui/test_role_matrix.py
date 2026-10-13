"""ADR-derived route coverage; origin: issue45/C-027 and the missing legacy rows."""
import ast
import copy
from contextlib import contextmanager
import ctypes
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import live_browser_check as browser

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / 'docs/experiments/formal/fixtures/console-route-roles.json'


def registered_routes(source):
    routes = set()
    for match in re.finditer(r'\.route\(\s*"([^"]+)"\s*,', source):
        depth, end = 1, match.end()
        while depth:
            depth += (source[end] == '(') - (source[end] == ')')
            end += 1
        for method in re.findall(r'\b(get|post|put|patch|delete|head|options)\s*\(', source[match.end():end]):
            routes.add((method.upper(), match.group(1)))
    return routes


def witnessed_calls(request, check):
    if hasattr(browser, 'run_role_matrix'):
        browser.run_role_matrix(request, check)
        return
    # Execute ONLY the retained selected-matrix AST against synthetic transports.
    # No main(), browser, sockets, subprocesses or native authority are invoked.
    main = next(node for node in ast.parse(Path(browser.__file__).read_text()).body
                if isinstance(node, ast.FunctionDef) and node.name == 'main')
    statements = next(node.body for node in ast.walk(main) if isinstance(node, ast.Try)
                      and any(isinstance(n, ast.Assign) and any(isinstance(t, ast.Name) and t.id == 'owner_routes'
                                                               for t in n.targets) for n in node.body))
    begin = next(i for i, node in enumerate(statements) if isinstance(node, ast.Assign)
                 and any(isinstance(t, ast.Name) and t.id == 'owner_routes' for t in node.targets))
    end = next(i for i in range(begin + 1, len(statements)) if isinstance(statements[i], ast.Assign)
               and any(isinstance(t, ast.Name) and t.id == 'read' for t in statements[i].targets))
    namespace = {'credential': 'reader', 'spindle_token': 'spindle', 'check': check,
                 'bearer_api': lambda role, path, method='GET', body=None: request(role or 'anonymous', method, path, body),
                 'api': lambda path, method='GET', body=None: request('owner', method, path, body),
                 'expect': lambda answer, status, message, label: check(browser.semantic_rejection(answer, status, message), label)}
    exec(compile(ast.Module(body=statements[begin:end], type_ignores=[]), '<retained-matrix>', 'exec'), namespace)


class RoleMatrixTests(unittest.TestCase):
    def setUp(self):
        self.fixture = json.loads(FIXTURE.read_text())
        self.expected = {(role, method, path): outcome
                         for method, _, path, _, outcomes in self.fixture['rows']
                         for role, outcome in zip(self.fixture['roles'], outcomes)}

    def request(self, role, method, path, body=None):
        # The retained fixture targeted browser-edge; the successor owns matrix.
        path = path.replace('/v1/console/nodes/browser-edge/', '/v1/console/nodes/matrix/')
        status, error = self.expected.get((role, method, path), (200, None))
        field = self.fixture['refusal_fields'].get(path, 'error')
        return status, {field: error} if error else {}

    def test_fixture_covers_both_registered_routers_and_all_control_actions(self):
        rows = self.fixture['rows']
        self.assertEqual(self.fixture['roles'], ['anonymous', 'spindle', 'reader', 'owner'])
        self.assertEqual(len(rows), 36)
        self.assertEqual(len(self.expected), 144)
        registered = set().union(*(registered_routes((ROOT / 'crates/fabric-server/src' / name).read_text())
                                  for name in ('console.rs', 'http.rs')))
        self.assertEqual({(row[0], row[1]) for row in rows}, registered)
        for prefix in ('/v1/console', '/v1/admin'):
            self.assertEqual({row[2].rsplit('/', 1)[1] for row in rows
                              if row[1] == prefix + '/nodes/{name}/{action}'}, {'pause', 'resume', 'revoke'})

    def test_live_matrix_calls_all_credentials_on_every_route_and_legacy_action(self):
        calls = []

        def request(role, method, path, body):
            calls.append((role, method, path))
            return self.request(role, method, path, body)

        witnessed_calls(request, self.assertTrue)
        self.assertEqual(set(calls), set(self.expected),
                         'live role matrix missing cells: ' + repr(sorted(set(self.expected) - set(calls))))
        self.assertEqual(len(calls), 144, 'each registered role-route cell must run exactly once')

    def test_reader_admission_mutation_is_rejected_by_the_live_checker(self):
        def request(role, method, path, body):
            if (role, method, path) == ('reader', 'GET', '/v1/console/audit'):
                return 200, {'events': []}
            return self.request(role, method, path, body)

        with self.assertRaisesRegex(AssertionError, 'reader.*GET /v1/console/audit'):
            witnessed_calls(request, self.assertTrue)

    def test_independent_outcomes_never_accept_admission_refusal_or_role_crossover(self):
        for role, path, expected in [
            ('spindle', '/v1/console/status', [401, 'sign in required']),
            ('reader', '/v1/console/audit', [403, 'role not permitted']),
            ('owner', '/v1/admin/nodes', [401, 'use a console credential']),
        ]:
            self.assertEqual(self.expected[(role, 'GET', path)], expected)
            self.assertFalse(browser.semantic_rejection((429, {'error': expected[1]}), *expected))
            self.assertFalse(browser.semantic_rejection((200, {'error': expected[1]}), *expected))
            self.assertFalse(browser.semantic_rejection((expected[0], {'error': 'wrong refusal'}), *expected))
        missing = copy.deepcopy(self.fixture['rows'])
        missing.remove(next(row for row in missing if row[0:3] ==
                            ['POST', '/v1/admin/nodes/{name}/{action}', '/v1/admin/nodes/matrix/resume']))
        self.assertEqual(len({(r[0], r[1]) for r in missing}), len({(r[0], r[1]) for r in self.fixture['rows']}))
        self.assertNotEqual({(r[0], r[2]) for r in missing}, {(r[0], r[2]) for r in self.fixture['rows']})

    def test_transports_refuse_expired_or_storage_limited_dispatch_and_progressing_body(self):
        # Origin: Miso's independent review of0de4b46; no main/network/process.
        source = ast.parse(Path(browser.__file__).read_text())
        main = next(n for n in source.body if isinstance(n, ast.FunctionDef) and n.name == 'main')
        functions = [n for n in source.body if isinstance(n, ast.FunctionDef) and n.name != 'main']
        functions += [n for n in main.body if isinstance(n, ast.FunctionDef) and n.name in ('call', 'bearer_api')]
        for transport in ('call', 'bearer_api'):
            for case in ('expired', 'storage', 'paced_expiry', 'timeout', 'progressing'):
                if transport == 'call' and case == 'paced_expiry':continue
                with self.subTest(transport=transport, case=case):
                    now, opened, reads = [2.0 if case == 'expired' else 0.0], [], []
                    class Response:
                        status = 200
                        fp = SimpleNamespace(raw=SimpleNamespace(_sock=SimpleNamespace(settimeout=lambda _: None)))
                        def __enter__(self):return self
                        def __exit__(self, *_):pass
                        def read(self, size=None):
                            reads.append('whole')
                            if case == 'progressing':now[0] = 2.0
                            return b'{"value":{"ready":true}}'
                        def read1(self, size):
                            reads.append('chunk')
                            if case == 'progressing':now[0] = 2.0
                            return b'{"value":{"ready":true}}' if len(reads) == 1 else b''
                    def open_response(request, **kwargs):
                        opened.append(kwargs['timeout'])
                        return Response()
                    namespace = dict(vars(browser))
                    namespace.update(time=SimpleNamespace(monotonic=lambda:now[0], monotonic_ns=lambda:int(now[0]*1e9)),
                                     deadline=1.0, storage_violation=SimpleNamespace(is_set=lambda:case == 'storage'),
                                     endpoint='http://localhost:1', origin='https://localhost:1', receipt={},
                                     args=SimpleNamespace(inject_role_defect=False), tls_opener=SimpleNamespace(open=open_response),
                                     pace_api=lambda:now.__setitem__(0, 2.0 if case == 'paced_expiry' else now[0]),
                                     cadence=SimpleNamespace(clock=lambda:now[0], last=None),
                                     urllib=SimpleNamespace(request=SimpleNamespace(Request=browser.urllib.request.Request, urlopen=open_response),
                                                            error=browser.urllib.error))
                    exec(compile(ast.Module(body=functions, type_ignores=[]), '<transport-boundary>', 'exec'), namespace)
                    invoke = (lambda:namespace['call']('GET', '/status')) if transport == 'call' else (lambda:namespace['bearer_api'](None, '/v1/console/nodes'))
                    if case == 'timeout':
                        invoke()
                        self.assertEqual(len(opened), 1)
                        self.assertGreater(opened[0], 0)
                        self.assertLessEqual(opened[0], 1.0, 'socket timeout exceeds remaining inner deadline')
                    else:
                        with self.assertRaisesRegex(RuntimeError, 'deadline|storage|512 MiB'):
                            invoke()
                        if case == 'progressing':self.assertEqual(reads, ['chunk'], 'wall bound must stop a progressively readable body')
                        else:self.assertEqual(opened, [], 'expired/storage-limited dispatch reached network')

    def test_actual_slow_headers_and_chunk_metadata_cannot_outlive_inner_io_bounds(self):
        # Origin: CPython read1 still reads chunk metadata/header lines internally.
        source = ast.parse(Path(browser.__file__).read_text())
        main = next(n for n in source.body if isinstance(n, ast.FunctionDef) and n.name == 'main')
        functions = [n for n in source.body if isinstance(n, ast.FunctionDef) and n.name != 'main']
        functions += [n for n in main.body if isinstance(n, ast.FunctionDef) and n.name == 'call']
        for case in ('headers', 'chunk_metadata', 'storage'):
            with self.subTest(case=case):
                storage, stop, errors = threading.Event(), threading.Event(), []
                with socket.socket() as listener:
                    listener.bind(('127.0.0.1', 0));listener.listen(1);listener.settimeout(2)
                    def peer():
                        try:
                            connection, _ = listener.accept()
                            with connection:
                                connection.settimeout(1);request=b''
                                while b'\r\n\r\n' not in request:request+=connection.recv(4096)
                                body=b'{"value":{"ready":true}}'
                                prefix=(b'HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n'+f'{len(body):x};note='.encode()
                                        if case == 'chunk_metadata' else b'HTTP/1.1 200 OK\r\nX-Progress: ')
                                connection.sendall(prefix);began=time.monotonic()
                                while time.monotonic()-began<.6 and not stop.is_set():
                                    if case == 'storage' and time.monotonic()-began>=.12:storage.set()
                                    connection.sendall(b'x');stop.wait(.01)
                                suffix=(b'\r\n'+body+b'\r\n0\r\n\r\n' if case == 'chunk_metadata'
                                        else b'\r\nContent-Length: '+str(len(body)).encode()+b'\r\n\r\n'+body)
                                if not stop.is_set():connection.sendall(suffix)
                        except (BrokenPipeError, ConnectionResetError):pass
                        except Exception as error:errors.append(error)
                    worker=threading.Thread(target=peer, daemon=True);worker.start()
                    namespace=dict(vars(browser));began=time.monotonic()
                    namespace.update(endpoint=f'http://127.0.0.1:{listener.getsockname()[1]}',
                                     deadline=began+(2 if case == 'storage' else .12),storage_violation=storage)
                    exec(compile(ast.Module(body=functions, type_ignores=[]), '<actual-io-boundary>', 'exec'), namespace)
                    previous_handler=browser.signal.getsignal(browser.signal.SIGALRM)
                    previous_timer=browser.signal.getitimer(browser.signal.ITIMER_REAL)
                    try:
                        with self.assertRaisesRegex(RuntimeError, 'storage|512 MiB' if case == 'storage' else 'deadline'):
                            namespace['call']('GET', '/status')
                        elapsed=time.monotonic()-began
                    finally:
                        stop.set();worker.join(1)
                    self.assertFalse(worker.is_alive(), 'synthetic peer did not stop')
                    self.assertEqual(errors, [], 'synthetic peer failed before exercising the bound')
                    self.assertEqual(browser.signal.getsignal(browser.signal.SIGALRM), previous_handler)
                    self.assertEqual(browser.signal.getitimer(browser.signal.ITIMER_REAL), previous_timer)
                    self.assertLessEqual(elapsed, .35, 'continuous metadata progress evaded the absolute I/O bound')

    def test_actual_browser_dispatch_gap_survives_delayed_pace_continuation(self):
        # Origin: Miso's 5ms continuation counterexample; execute actual injected JS.
        source=ast.parse(Path(browser.__file__).read_text())
        scripts=[n.args[0].value for n in ast.walk(source) if isinstance(n, ast.Call)
                 and isinstance(n.func, ast.Name) and n.func.id == 'js' and n.args
                 and isinstance(n.args[0], ast.Constant) and isinstance(n.args[0].value, str)
                 and 'window.fixturePaceQueue=Promise.resolve()' in n.args[0].value]
        self.assertEqual(len(scripts), 1, 'one actual fetch scheduler must supply the control')
        runner=browser.DATA/'tools/bun-1.4.0/bun'
        if not runner.is_file():runner=Path(browser.shutil.which('bun') or str(runner))
        self.assertTrue(runner.is_file(), 'Bun is required for the actual JavaScript control')
        script='const setup='+json.dumps(scripts[0])+r''';
          let now=0,delayed=false;const actual=[];
          const window={fetch:async(url)=>{actual.push({path:url,started_ms:now});return {status:200,headers:{get:()=>null}}}};
          new Function('window','performance','setTimeout','location','return function(){'+setup+'}')(
            window,{now:()=>now},(resolve,delay)=>{now+=delay;resolve()},{href:'https://fixture.invalid/console/'})(550);
          const pace=window.fixturePace;
          window.fixturePace=async(...args)=>{const value=await pace(...args);if(!delayed){delayed=true;now+=5}return value};
          await window.fetch('/v1/console/auth/login/start');await window.fetch('/v1/console/auth/login/finish');
          window.fixtureUnpacedBurst=true;
          await Promise.all(Array.from({length:16},()=>window.fetch('/v1/console/auth/login/start')));
          console.log(JSON.stringify({actual,recorded:window.fixtureApiHeaders,truncated:window.fixtureApiHeadersTruncated}));
        '''
        result=subprocess.run([str(runner),'--eval',script],capture_output=True,text=True,timeout=5)
        self.assertEqual(result.returncode,0,result.stderr)
        witness=json.loads(result.stdout);actual=witness['actual'];recorded=witness['recorded']
        self.assertEqual(len(actual),18,'two paced requests and the unchanged16-request unpaced control')
        self.assertEqual([r['started_ms'] for r in actual],[r['started_ms'] for r in recorded], 'receipt stamps actual fetch dispatch')
        self.assertEqual([r['status'] for r in recorded],[200]*18)
        self.assertEqual([r['paced'] for r in recorded],[True,True]+[False]*16)
        self.assertEqual(len({r['started_ms'] for r in actual[2:]}),1,'registered burst remains unpaced')
        self.assertFalse(witness['truncated'])
        self.assertGreaterEqual(actual[1]['started_ms']-actual[0]['started_ms'],550,
                                '5ms pace-continuation delay shrank actual dispatch gap: '+repr(actual[:2]))


def owned_browser_identity(pid):
    process = Path('/proc') / str(pid)
    fields = (process / 'stat').read_text().rsplit(')', 1)[1].split()
    if process.stat().st_uid != os.getuid() or int(fields[19]) <= 0:
        raise AssertionError('unowned browser control process')
    return int(fields[19]), int(fields[2])


@contextmanager
def browser_cleanup_fixture(modes, blocked_sampler=False):
    # Every process is real but synthetic, with a finite 45-second lifespan.
    sys.path.insert(0, str(ROOT / 'tools'))
    from resource_group import require_limits
    require_limits()
    libc, previous = ctypes.CDLL(None, use_errno=True), ctypes.c_int()
    if libc.prctl(37, ctypes.byref(previous), 0, 0, 0) or libc.prctl(36, 1, 0, 0, 0):
        raise RuntimeError('browser process controls require Linux subreaper')
    directory = None
    groups, identities, process_logs = [], [], []
    sampler = None
    stop_samples, release_sampler = threading.Event(), threading.Event()
    try:
        directory = Path(tempfile.mkdtemp(dir=os.environ['FABRIC_SCRATCH_ROOT'], prefix='browser-cleanup-control-'))
        work, out = directory / 'work', directory / 'out'
        work.mkdir()
        out.mkdir()
        script = directory / 'owned-process.py'
        script.write_text('''import os, select, signal, sys, time
from pathlib import Path
directory, work, mode = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
expires = time.monotonic() + 45
def atomic(name, text):
    pending = directory / (name + '.pending')
    with pending.open('x') as stream:
        stream.write(text); stream.flush(); os.fsync(stream.fileno())
    pending.replace(directory / name)
if mode == 'timeout': signal.signal(signal.SIGTERM, signal.SIG_IGN)
gate_read, gate_write = os.pipe()
child = os.fork()
if child == 0:
    os.close(gate_write)
    if not select.select([gate_read], [], [], 5)[0] or os.read(gate_read, 1) != b'1': os._exit(1)
    os.close(gate_read)
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    held = (work / ('held-' + directory.name)).open('xb')
    held.write(b'literal browser private fixture'); held.flush()
    atomic('ready', 'ready')
    time.sleep(max(0, expires - time.monotonic()))
    os._exit(0)
os.close(gate_read)
try:
    process = Path('/proc') / str(child)
    fields = (process / 'stat').read_text().rsplit(')', 1)[1].split()
    if process.stat().st_uid != os.getuid() or int(fields[19]) <= 0 or int(fields[1]) != os.getpid() or int(fields[2]) != os.getpid():
        raise RuntimeError('fork child is outside owned private group')
    atomic('child.pid', f'{child} {fields[19]}')
    os.write(gate_write, b'1')
finally:
    os.close(gate_write)
deadline = time.monotonic() + 5
while not (directory / 'ready').exists():
    if time.monotonic() >= deadline: raise RuntimeError('child readiness deadline')
    time.sleep(.01)
if mode != 'exited': time.sleep(max(0, expires - time.monotonic()))
''')
        for index, mode in enumerate(modes):
            group_dir = directory / ('group-' + str(index))
            group_dir.mkdir()
            log = (group_dir / 'process.log').open('xb')
            process_logs.append(log)
            process = subprocess.Popen([sys.executable, '-B', str(script), str(group_dir), str(work), mode],
                                       start_new_session=True, stdout=log, stderr=log)
            groups.append({'process': process, 'directory': group_dir})
            start, pgid = owned_browser_identity(process.pid)
            descriptor = os.pidfd_open(process.pid)
            identities.append((process.pid, start, descriptor))
            if owned_browser_identity(process.pid) != (start, process.pid) or pgid != process.pid:
                raise AssertionError('owned leader identity changed')
            deadline = time.monotonic() + 5
            while not (group_dir / 'ready').exists():
                if time.monotonic() >= deadline:
                    raise AssertionError('owned child did not publish readiness')
                time.sleep(.01)
            marker = group_dir / 'child.pid'
            info = marker.lstat()
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1 or info.st_size > 128:
                raise AssertionError('invalid child identity marker')
            child, child_start = map(int, marker.read_text().split())
            if child <= 1 or child_start <= 0 or owned_browser_identity(child) != (child_start, process.pid):
                raise AssertionError('unbound owned child identity')
            child_descriptor = os.pidfd_open(child)
            identities.append((child, child_start, child_descriptor))
            if owned_browser_identity(child) != (child_start, process.pid):
                raise AssertionError('child identity changed during pidfd binding')
            groups[-1].update(child=child, child_start=child_start)
            if mode == 'exited':
                process.wait(timeout=5)
        sampler = threading.Thread(target=lambda: release_sampler.wait(4) if blocked_sampler else stop_samples.wait(4))
        sampler.start()
        namespace = dict(vars(browser))
        namespace.update(session=None, processes=[('group-' + str(i), item['process']) for i, item in enumerate(groups)],
                         receipt={'exit': 0}, stop_samples=stop_samples, sampler=sampler,
                         out=out, samples=[], logs=[], work=work)
        yield namespace, groups
    finally:
        try:
            errors = []
            stop_samples.set()
            release_sampler.set()
            if sampler is not None:
                try:
                    sampler.join(timeout=5)
                    if sampler.is_alive():
                        errors.append('sampler did not stop')
                except RuntimeError as error:
                    errors.append(str(error))
            for pid, start, descriptor in identities:
                try:
                    try:
                        current, _ = owned_browser_identity(pid)
                    except FileNotFoundError:
                        continue
                    if current != start:
                        raise AssertionError('refuse cleanup of reused PID')
                    try:
                        signal.pidfd_send_signal(descriptor, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    deadline = time.monotonic() + 5
                    while True:
                        try:
                            reaped, _ = os.waitpid(pid, os.WNOHANG)
                        except ChildProcessError:
                            reaped = 0
                        if reaped == pid:
                            break
                        try:
                            current, _ = owned_browser_identity(pid)
                        except FileNotFoundError:
                            break
                        if current != start or time.monotonic() >= deadline:
                            raise AssertionError('owned process identity/cleanup not confirmed')
                        time.sleep(.01)
                except (OSError, ValueError, IndexError, AssertionError) as error:
                    errors.append(str(error))
                finally:
                    try:
                        os.close(descriptor)
                    except OSError as error:
                        errors.append(str(error))
            for item in groups:
                try:
                    os.killpg(item['process'].pid, 0)
                except ProcessLookupError:
                    continue
                except OSError as error:
                    errors.append(str(error))
                else:
                    errors.append('owned group is still populated')
            for log in process_logs:
                try:
                    log.close()
                except OSError as error:
                    errors.append(str(error))
            if errors:
                raise AssertionError('control scratch retained at ' + str(directory) + ': ' + '; '.join(errors))
            if directory is not None:
                shutil.rmtree(directory)
        finally:
            if libc.prctl(36, previous.value, 0, 0, 0):
                raise RuntimeError('restore previous test subreaper setting')


def actual_browser_finally(namespace):
    # Execute the actual main finally; no browser/server or main setup is invoked.
    source = ast.parse(Path(browser.__file__).read_text())
    main = next(node for node in source.body if isinstance(node, ast.FunctionDef) and node.name == 'main')
    finalizers = [node.finalbody for node in main.body if isinstance(node, ast.Try) and node.finalbody]
    if len(finalizers) != 1:
        raise AssertionError('one actual browser main finalizer must supply the control')
    exec(compile(ast.Module(body=finalizers[0], type_ignores=[]), browser.__file__, 'exec'), namespace)


class BrowserFixtureCleanupTests(unittest.TestCase):
    def assert_zero_population(self, groups):
        for item in groups:
            self.assertFalse((Path('/proc') / str(item['child'])).exists(), 'actual finalizer left an unreaped owned child')
            with self.assertRaises(ProcessLookupError, msg='actual finalizer did not confirm zero private-group population'):
                os.killpg(item['process'].pid, 0)

    def test_actual_finally_normal_empty_fixture_has_confirmed_cleanup(self):
        with browser_cleanup_fixture([]) as (namespace, groups):
            actual_browser_finally(namespace)
            self.assertFalse(namespace['sampler'].is_alive())
            self.assertFalse(namespace['work'].exists())
            self.assertTrue((namespace['out'] / 'browser.json').is_file())
            self.assertTrue((namespace['out'] / 'resource-samples.json').is_file())

    def test_actual_finally_stops_child_when_leader_already_exited(self):
        with browser_cleanup_fixture(['exited']) as (namespace, groups):
            self.assertEqual(groups[0]['process'].returncode, 0)
            actual_browser_finally(namespace)
            self.assert_zero_population(groups)

    def test_actual_finally_stops_term_resistant_child_after_leader_termination(self):
        with browser_cleanup_fixture(['normal']) as (namespace, groups):
            actual_browser_finally(namespace)
            self.assert_zero_population(groups)

    def test_actual_finally_timeout_escalation_stops_whole_group(self):
        with browser_cleanup_fixture(['timeout']) as (namespace, groups):
            process = groups[0]['process']
            original_wait, requested = process.wait, []

            def shortened_wait(timeout=None):
                requested.append(timeout)
                return original_wait(timeout=min(timeout, .2) if timeout is not None else None)

            # TEST ONLY real wait shortening; production 15s/5s limits stay fixed.
            with patch.object(process, 'wait', side_effect=shortened_wait):
                actual_browser_finally(namespace)
            self.assertIn(15, requested, 'the actual production TERM wait supplied the timeout path')
            self.assert_zero_population(groups)

    def test_actual_finally_attempts_all_groups_and_retains_scratch_after_failed_stop(self):
        with browser_cleanup_fixture(['normal', 'normal']) as (namespace, groups):
            attempted, real_killpg = [], os.killpg
            failed = groups[-1]['process'].pid

            def fail_one_group(pgid, sig):
                if sig:
                    attempted.append(pgid)
                if pgid == failed and sig:
                    raise PermissionError('literal owned group stop failure')
                return real_killpg(pgid, sig)

            with patch.object(browser.os, 'killpg', side_effect=fail_one_group):
                try:
                    actual_browser_finally(namespace)
                except (OSError, RuntimeError):
                    pass
            self.assertEqual(set(attempted), {item['process'].pid for item in groups},
                             'a first stop error must not suppress other owned group attempts')
            self.assertTrue(namespace['work'].is_dir(), 'unconfirmed stop must retain private fixture scratch')
            self.assertTrue((Path('/proc') / str(groups[-1]['child'])).exists(), 'injected refusal must reach a real living group')
            self.assertNotIn('process groups stopped', namespace['receipt'].get('cleanup', ''), 'do not claim cleanup on refusal')

    def test_actual_finally_preserves_scratch_until_bounded_sampler_is_stopped(self):
        with browser_cleanup_fixture([], blocked_sampler=True) as (namespace, groups):
            started = time.monotonic()
            try:
                actual_browser_finally(namespace)
            except RuntimeError:
                pass
            self.assertTrue(namespace['stop_samples'].is_set(), 'actual finalizer must reach sampler stop')
            self.assertLess(time.monotonic() - started, 3, 'sampler refusal must remain bounded')
            self.assertTrue(namespace['sampler'].is_alive(), 'literal sampler must still be alive at refusal')
            self.assertTrue(namespace['work'].is_dir(), 'scratch must not be removed while sampler lives')
            self.assertNotIn('process groups stopped', namespace['receipt'].get('cleanup', ''), 'unconfirmed sampler stop is not cleanup')


if __name__ == '__main__':
    unittest.main()
