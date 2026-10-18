"""Finite virtual-passkey checks against the production TLS server and Spindle."""
import argparse
import base64
from contextlib import contextmanager
import hashlib
import json
import os
import re
from pathlib import Path
import secrets
import select
import shutil
import signal
import socket
import ssl
import stat
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from browser_producer import TRACE_ID, traces
from fixture_cadence import FixtureCadence

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import STORAGE as DATA, require_limits
sys.path.insert(0, str(ROOT / 'tools/packaging'))
from stage_console import stage
sys.path.insert(0, str(ROOT / 'tools/bench/labs/completion'))
import cgroups

VERSION = '155.0.8059.39'
TOOLS = DATA / 'tools/ui' / f'chrome-for-testing-{VERSION}'
ROLE_FIXTURE = ROOT / 'docs/experiments/formal/fixtures/console-route-roles.json'


def browser_group_check(ok, reason):
    if not ok:
        raise RuntimeError('browser-group-' + reason)


def browser_directory(record):
    path = record['path']
    actual, held = path.lstat(), os.fstat(record['fd'])
    browser_group_check(path.resolve(strict=True) == path and stat.S_ISDIR(actual.st_mode)
        and (actual.st_dev, actual.st_ino) == (held.st_dev, held.st_ino)
        == (record['dev'], record['ino']), 'identity')


def browser_pid(pid):
    path = Path('/proc') / str(pid)
    fields = (path / 'stat').read_text().rsplit(')', 1)[1].split()
    browser_group_check(path.stat().st_uid == os.getuid(), 'identity')
    return int(fields[19]), fields[0]


def browser_pidfd(fd):
    return int(next(line.split(':', 1)[1] for line in
        (Path('/proc/self/fdinfo') / str(fd)).read_text().splitlines() if line.startswith('Pid:')))


def browser_provisional_cleanup(parent, record):
    group, proc = record['group'], record.get('process')
    if proc is None:
        return
    browser_directory(parent); browser_directory(group)
    browser_group_check(group['exclusive'] is True and group['path'].parent == parent['path'], 'ownership')
    pidfd = group.get('pidfd')
    if pidfd is not None and proc.poll() is None:
        browser_group_check(browser_pidfd(pidfd) == proc.pid
            and browser_pid(proc.pid)[0] == group['start_ticks'], 'identity')
        try:
            signal.pidfd_send_signal(pidfd, signal.SIGKILL)
        except ProcessLookupError:
            pass
    try:
        proc.wait(timeout=min(5, max(0, parent['deadline_monotonic_ns'] / 1e9 - time.monotonic())))
    except subprocess.TimeoutExpired as error:
        if group.get('fd') is None:
            record['cleanup_error'] = 'provisional child did not stop and group descriptor is absent'
            raise RuntimeError('browser-provisional-stop') from error
        kill_fd = os.open('cgroup.kill', os.O_WRONLY | os.O_NOFOLLOW | os.O_CLOEXEC,
                          dir_fd=group['fd'])
        try:
            if os.write(kill_fd, b'1') != 1:
                raise RuntimeError('browser-provisional-cgroup-kill')
        finally:
            os.close(kill_fd)
        try:
            proc.wait(timeout=min(5, max(0, parent['deadline_monotonic_ns'] / 1e9 - time.monotonic())))
        except subprocess.TimeoutExpired as cleanup_error:
            record['cleanup_error'] = 'owned provisional cgroup did not stop within its bound'
            raise RuntimeError('browser-provisional-stop') from cleanup_error
    if (Path('/proc') / str(proc.pid)).exists():
        browser_group_check(pidfd is not None and browser_pidfd(pidfd) == -1, 'identity')
    browser_directory(group)
    browser_group_check('populated 0' in (group['path'] / 'cgroup.events').read_text().splitlines(), 'stop')
    record['children_reaped'] = proc.returncode is not None and not (Path('/proc') / str(proc.pid)).exists()
    browser_group_check(record['children_reaped'], 'reap')
    if pidfd is not None:
        os.close(pidfd); group['pidfd'] = None
    os.close(group['fd']); group['fd'] = None
    record['state'] = 'cleaned'


def browser_members(group):
    known = {row['pid']: row for row in group.setdefault('members', [])}
    paths = [group['path'], *(p for p in group['path'].rglob('*') if p.is_dir())]
    for path in paths:
        for text in (path / 'cgroup.procs').read_text().split():
            pid = int(text)
            if pid in known:
                browser_group_check(browser_pid(pid)[0] == known[pid]['start_ticks'], 'identity')
                continue
            descriptor = os.pidfd_open(pid)
            try:
                start = browser_pid(pid)[0]
                placement = cgroups.ROOT / next(line[3:] for line in
                    (Path('/proc') / text / 'cgroup').read_text().splitlines() if line.startswith('0::')).lstrip('/')
                browser_group_check(placement == group['path'] or placement.is_relative_to(group['path']), 'placement')
                browser_group_check(browser_pidfd(descriptor) == pid and browser_pid(pid)[0] == start, 'identity')
                row = dict(pid=pid, start_ticks=start, pidfd=descriptor)
                group['members'].append(row); known[pid] = row
            except BaseException:
                os.close(descriptor)
                raise


def bind_browser_group(parent, group, proc):
    binding = None
    try:
        browser_directory(parent); browser_directory(group)
        browser_group_check(parent['path'].name.startswith('fabric-work-')
            and cgroups.current() == parent['path'] / 'supervisor', 'parent')
        browser_group_check({'memory', 'cpu', 'pids', 'io'} <=
            set((parent['path'] / 'cgroup.subtree_control').read_text().split()), 'delegation')
        browser_group_check(group['path'].parent == parent['path'] and group['exclusive'] is True, 'ownership')
        browser_group_check((group['path'] / 'cgroup.kill').is_file(), 'capability')
        browser_group_check(time.monotonic_ns() < parent['deadline_monotonic_ns'], 'deadline')
        browser_group_check(isinstance(proc, subprocess.Popen) and proc is group['process']
            and proc.pid == group['pid'] and browser_pid(proc.pid)[0] == group['start_ticks']
            and browser_pidfd(group['pidfd']) == proc.pid, 'identity')
        binding = {'parent': dict(parent), 'group': dict(group, members=[])}
        binding['group']['pidfd_inode'] = os.fstat(group['pidfd']).st_ino
        browser_members(binding['group'])
        browser_group_check(any(row['pid'] == proc.pid for row in binding['group']['members']), 'placement')
        return binding
    except (OSError, KeyError, ValueError, StopIteration, RuntimeError) as error:
        if binding is not None:
            for row in binding['group']['members']: os.close(row['pidfd'])
        if isinstance(error, RuntimeError): raise
        raise RuntimeError('browser-group-identity') from None


def stop_browser_groups(parent, bindings):
    errors = []
    for binding in reversed(bindings):
        group = binding['group']
        try:
            browser_directory(parent); browser_directory(binding['parent']); browser_directory(group)
            browser_group_check(all(binding['parent'][key] == parent[key] for key in
                ('path', 'fd', 'dev', 'ino', 'deadline_monotonic_ns')) and group['path'].parent == parent['path']
                and group['exclusive'] is True, 'identity')
            proc = group['process']
            browser_group_check(isinstance(proc, subprocess.Popen) and proc.pid == group['pid']
                and os.fstat(group['pidfd']).st_ino == group['pidfd_inode'], 'identity')
            observed_pid = browser_pidfd(group['pidfd'])
            browser_group_check(observed_pid == group['pid'] or
                (observed_pid == -1 and proc.returncode is not None and not (Path('/proc') / str(group['pid'])).exists()), 'identity')
            if 'namespaces' in parent:
                browser_group_check(browser_namespaces() == parent['namespaces'], 'namespace')
            browser_members(group)
            for row in group['members']:
                try:
                    browser_group_check(browser_pid(row['pid'])[0] == row['start_ticks']
                        and browser_pidfd(row['pidfd']) == row['pid'], 'identity')
                    signal.pidfd_send_signal(row['pidfd'], signal.SIGTERM)
                except ProcessLookupError:
                    pass
                except FileNotFoundError:
                    browser_group_check(browser_pidfd(row['pidfd']) == -1, 'identity')
            try:
                proc.wait(timeout=min(15, max(0, parent['deadline_monotonic_ns'] / 1e9 - time.monotonic())))
            except subprocess.TimeoutExpired:
                pass
            end = min(time.monotonic() + 5, parent['deadline_monotonic_ns'] / 1e9)
            browser_directory(group)
            if 'populated 1' in (group['path'] / 'cgroup.events').read_text().splitlines():
                descriptor = os.open('cgroup.kill', os.O_WRONLY | os.O_NOFOLLOW, dir_fd=group['fd'])
                try: os.write(descriptor, b'1')
                finally: os.close(descriptor)
            # Reap only retained process identities, including adopted zombies.
            while True:
                remaining = []
                for row in group['members']:
                    path = Path('/proc') / str(row['pid'])
                    if path.exists():
                        browser_group_check(browser_pid(row['pid'])[0] == row['start_ticks'], 'identity')
                        if row['pid'] == proc.pid:
                            proc.poll()  # Popen alone consumes and records its leader's status.
                        else:
                            try: os.waitid(os.P_PIDFD, row['pidfd'], os.WEXITED | os.WNOHANG)
                            except ChildProcessError: pass
                    if path.exists(): remaining.append(row['pid'])
                group['kernel_empty'] = 'populated 0' in (group['path'] / 'cgroup.events').read_text().splitlines()
                group['children_reaped'] = not remaining
                if group['kernel_empty'] and group['children_reaped']: break
                if time.monotonic() >= end: raise RuntimeError('browser-group-stop')
                time.sleep(.01)
            proc.wait(timeout=0)
        except (OSError, RuntimeError, KeyError, ValueError, subprocess.TimeoutExpired) as error:
            errors.append(str(error))
            group['stop_error'] = str(error)
    if errors:
        raise RuntimeError('browser-group-stop')


def browser_namespaces():
    return {name: os.readlink('/proc/self/ns/' + name) for name in ('pid', 'mnt', 'cgroup')}


def browser_hold_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    info = os.fstat(descriptor)
    return dict(path=path, fd=descriptor, dev=info.st_dev, ino=info.st_ino)


def begin_browser_domain(limits, deadline):
    import ctypes
    browser_group_check(os.uname().machine == 'x86_64' and ctypes.sizeof(ctypes.c_void_p) == 8, 'signal-abi')
    class Action(ctypes.Structure):
        _fields_ = [('handler', ctypes.c_void_p), ('mask', ctypes.c_ulong * 16),
                    ('flags', ctypes.c_int), ('restorer', ctypes.c_void_p)]
    libc, action, previous = ctypes.CDLL(None, use_errno=True), Action(), ctypes.c_int()
    browser_group_check(libc.sigaction(signal.SIGCHLD, None, ctypes.byref(action)) == 0
        and action.handler is None and not action.flags & 2, 'signal-ownership')
    try:
        os.waitid(os.P_ALL, 0, os.WEXITED | os.WNOHANG | os.WNOWAIT | 0x40000000)
    except ChildProcessError:
        pass
    else:
        raise RuntimeError('browser-group-foreign-child')
    values = {name: (limits / name).read_text().strip() for name in
        ('memory.max', 'memory.high', 'memory.swap.max', 'cpu.max', 'pids.max')}
    browser_group_check(0 < int(values['memory.high']) <= 3500000000
        and int(values['memory.high']) <= int(values['memory.max']) <= 4000000000
        and values['memory.swap.max'] == '0' and values['cpu.max'] == '200000 100000'
        and values['pids.max'] == '512', 'limits')
    parent = browser_hold_directory(limits)
    parent.update(deadline_monotonic_ns=int(deadline * 1e9), namespaces=browser_namespaces(),
                  browser_launch_attempts=0)
    try:
        browser_group_check(libc.prctl(37, ctypes.byref(previous), 0, 0, 0) == 0
            and libc.prctl(36, 1, 0, 0, 0) == 0, 'subreaper')
        parent['previous_subreaper'] = previous.value
        actual = ctypes.c_int()
        browser_group_check(libc.prctl(37, ctypes.byref(actual), 0, 0, 0) == 0 and actual.value == 1, 'subreaper')
        browser_group_check(cgroups.delegate() == limits, 'parent')
        cgroups.set_limits(limits / 'supervisor', 4000000000, 3500000000, 512, 2)
        return parent
    except BaseException:
        if 'previous_subreaper' in parent: libc.prctl(36, previous.value, 0, 0, 0)
        os.close(parent['fd'])
        raise


def browser_group_launch(parent, bindings, label, command, log, environment, *, entry_helper,
                         entry_helper_sha256, clock=None):
    browser_directory(parent)
    browser_group_check(browser_namespaces() == parent['namespaces'], 'namespace')
    attempt = parent['browser_launch_attempts']
    browser_group_check(isinstance(attempt, int) and not isinstance(attempt, bool) and attempt >= 0,
                        'launch-counter')
    parent['browser_launch_attempts'] = attempt + 1
    path, _ = cgroups.subgroup(parent['path'], 'browser-' + str(attempt),
                               4000000000, 3500000000, 512, 2)
    group = browser_hold_directory(path)
    group['exclusive'] = True
    browser_group_check(entry_helper.is_absolute() and entry_helper.is_file()
        and hashlib.sha256(entry_helper.read_bytes()).hexdigest() == entry_helper_sha256, 'entry-helper')
    target = list(command)
    inherited_fds = []
    inherited={}
    if clock is not None:
        browser_group_check(not any(key in environment for key in ('LD_PRELOAD','LD_AUDIT','FABRIC_CLOCK_FD')), 'clock-environment')
        helper=ROOT/'tools/ui/session_clock_exec.py'
        target=[sys.executable,'-B',str(helper),str(path),str(clock[0]),str(clock[1]),*command]
        inherited_fds.append(clock[1])
    ready_read, ready_write = os.pipe()
    release_read, release_write = os.pipe()
    handshake_deadline = min(time.monotonic() + 5, parent['deadline_monotonic_ns'] / 1e9)
    group.update(process=None, pid=None, pidfd=None, start_ticks=None)
    record = {'parent': parent, 'group': group, 'state': 'starting', 'process': None}
    parent.setdefault('provisional_children', []).append(record)
    proc = None
    binding = None
    transferred = False
    try:
        arguments=[str(entry_helper),'browser-enter-exec','--group',str(path),
            '--ready-fd',str(ready_write),'--release-fd',str(release_read),
            '--timeout-ms','5000','--',*target]
        inherited_fds.extend((ready_write, release_read))
        inherited['pass_fds']=tuple(inherited_fds)
        proc = subprocess.Popen(arguments, stdout=log, stderr=subprocess.STDOUT,
            start_new_session=True, env=environment, **inherited)
        os.close(ready_write); ready_write = None
        os.close(release_read); release_read = None
        group.update(process=proc, pid=proc.pid)
        record['process'] = proc
        group['pidfd'] = os.pidfd_open(proc.pid)
        group['start_ticks'] = browser_pid(proc.pid)[0]
        record['state'] = 'spawned'
        poller = select.poll()
        poller.register(ready_read, select.POLLIN | select.POLLHUP | select.POLLERR)
        ready = bytearray()
        while True:
            remaining = handshake_deadline - time.monotonic()
            browser_group_check(remaining > 0, 'entry-ready-deadline')
            remaining_ms = max(1, int(remaining * 1000 + 0.999))
            events = poller.poll(remaining_ms)
            browser_group_check(bool(events) and time.monotonic() < handshake_deadline, 'entry-ready-deadline')
            browser_group_check(events[0][1] & select.POLLERR == 0, 'entry-ready-pipe-error')
            chunk = os.read(ready_read, 2)
            if not chunk:
                browser_group_check(bytes(ready) == b'R', 'entry-ready-framing')
                break
            ready.extend(chunk)
            browser_group_check(bytes(ready) == b'R', 'entry-ready-framing')
        record['ready_monotonic_ns'] = time.monotonic_ns()
        record['entry_helper_sha256'] = entry_helper_sha256
        browser_group_check(proc.poll() is None and browser_pidfd(group['pidfd']) == proc.pid
            and browser_pid(proc.pid)[0] == group['start_ticks'], 'identity')
        placement = cgroups.ROOT / next(line[3:] for line in
            (Path('/proc') / str(proc.pid) / 'cgroup').read_text().splitlines()
            if line.startswith('0::')).lstrip('/')
        browser_group_check(placement == path, 'placement')
        browser_group_check(time.monotonic() < handshake_deadline, 'entry-ready-deadline')
        binding = bind_browser_group(parent, group, proc)
        binding['group']['label'] = label
        bindings.append(binding)
        record['state'] = 'bound'
        transferred = True
        record['release_monotonic_ns'] = time.monotonic_ns()
        browser_group_check(time.monotonic() < handshake_deadline, 'entry-release-deadline')
        binding['group']['entry_helper_sha256'] = entry_helper_sha256
        binding['group']['entry_elapsed_ns'] = record['release_monotonic_ns'] - record['ready_monotonic_ns']
        if os.write(release_write, b'G') != 1:
            raise RuntimeError('browser-entry-release')
        os.close(release_write); release_write = None
        record['state'] = 'released'
        return proc
    except BaseException:
        if release_write is not None:
            os.close(release_write); release_write = None
        if not transferred and proc is not None:
            try:
                browser_provisional_cleanup(parent, record)
            except BaseException as cleanup_error:
                record['cleanup_error'] = str(cleanup_error)
        elif not transferred:
            os.close(group['fd']); group['fd'] = None
            record['state'] = 'cleaned'
        raise
    finally:
        for descriptor in (ready_read, ready_write, release_read, release_write):
            if descriptor is not None:
                try: os.close(descriptor)
                except OSError: pass


def browser_reap_domain(parent, bindings):
    deadline = min(time.monotonic() + 5, parent['deadline_monotonic_ns'] / 1e9)
    while True:
        if 'namespaces' in parent:
            browser_group_check(browser_namespaces() == parent['namespaces'], 'namespace')
        try:
            observed = os.waitid(os.P_ALL, 0, os.WEXITED | os.WNOHANG | os.WNOWAIT | 0x40000000)
        except ChildProcessError:
            parent['own_children_empty'] = True
            return
        if observed is not None:
            pid = observed.si_pid
            start, state = browser_pid(pid)
            fields = (Path('/proc') / str(pid) / 'stat').read_text().rsplit(')', 1)[1].split()
            placement = cgroups.ROOT / next(line[3:] for line in
                (Path('/proc') / str(pid) / 'cgroup').read_text().splitlines() if line.startswith('0::')).lstrip('/')
            binding = next((b for b in bindings if placement == b['group']['path']
                or placement.is_relative_to(b['group']['path'])), None)
            browser_group_check(binding is not None and int(fields[1]) == os.getpid() and state == 'Z', 'foreign-child')
            group = binding['group']; browser_directory(parent); browser_directory(group)
            descriptor = os.pidfd_open(pid)
            try:
                browser_group_check(browser_pidfd(descriptor) == pid and browser_pid(pid)[0] == start, 'identity')
                result = os.waitid(os.P_PIDFD, descriptor, os.WEXITED | os.WNOHANG | 0x40000000)
                browser_group_check(result is not None and result.si_pid == pid
                    and not (Path('/proc') / str(pid)).exists(), 'reap')
                group.setdefault('late_reaped', []).append(dict(pid=pid, start_ticks=start, status=result.si_status))
                browser_group_check(len(group['late_reaped']) <= 512, 'tasks')
            finally:
                os.close(descriptor)
        browser_group_check(time.monotonic() < deadline, 'stop')
        if observed is None: time.sleep(.01)


def browser_cleanup_provisionals(parent):
    for record in reversed(parent.get('provisional_children', [])):
        if record['state'] in ('bound', 'released', 'cleaned'):
            continue
        browser_provisional_cleanup(parent, record)


def end_browser_domain(parent, bindings):
    if 'previous_subreaper' not in parent: return  # Synthetic source fixtures own their subreaper.
    import ctypes
    browser_group_check(parent.get('own_children_empty') is True, 'stop')
    browser_group_check(ctypes.CDLL(None).prctl(36, parent['previous_subreaper'], 0, 0, 0) == 0, 'subreaper')
    descriptors = {parent['fd']}
    for binding in bindings:
        group = binding['group']; descriptors.update((group['fd'], group['pidfd']))
        descriptors.update(row['pidfd'] for row in group['members'])
    for record in parent.get('provisional_children', []):
        group = record['group']
        descriptors.update(fd for fd in (group.get('fd'), group.get('pidfd')) if fd is not None)
    for descriptor in descriptors: os.close(descriptor)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, url):
        return None


def run_role_matrix(request, check, observations=None):
    fixture = json.loads(ROLE_FIXTURE.read_text())
    observations = [] if observations is None else observations
    for method, template, path, body, outcomes in fixture['rows']:
        for role, (expected_status, expected_refusal) in zip(fixture['roles'], outcomes):
            status, response = request(role, method, path, body)
            field = fixture['refusal_fields'].get(path, 'error')
            refusal = response.get(field) if status>=400 and isinstance(response, dict) else None
            observations.append({'role':role,'method':method,'template':template,'path':path,
                                 'status':status,'refusal':refusal})
            check(status == expected_status and (expected_refusal is None or refusal == expected_refusal),
                  f'{role} {method} {path}: expected {expected_status} {expected_refusal}')
    cells = {(item['role'], item['method'], item['path']) for item in observations}
    check(len(observations) == len(cells) == 144, 'all 144 route-role cells dispatched exactly once')
    return observations


def semantic_rejection(answer, status, message):
    code, body = answer
    return code == status and isinstance(body, dict) and body.get('error') == message


def native_clock_sources(hashes,root):
    rows=[];roots=set()
    if not isinstance(hashes,dict):raise ValueError('native-clock-source-binding')
    for name in ('session_clock.py','session_clock.c','session_clock_probe.rs'):
        suffix='/tools/ui/'+name
        keys=[key for key in hashes if isinstance(key,str) and key.endswith(suffix)]
        if len(keys)!=1:raise ValueError('native-clock-source-binding')
        key=keys[0];path=Path(key)
        if not path.is_absolute() or '..' in path.parts or str(path)!=key:raise ValueError('native-clock-source-binding')
        current=hashlib.sha256((root/'tools/ui'/name).read_bytes()).hexdigest()
        if hashes[key]!=current:raise ValueError('native-clock-source-binding')
        roots.add(key[:-len(suffix)]);rows.append({'recorded_path':key,'current_sha256':current})
    if len(roots)!=1:raise ValueError('native-clock-source-binding')
    return rows


def dispatch_timeout(deadline, storage, ceiling):
    if storage.is_set():raise RuntimeError('owned fixture exceeds 512 MiB storage ceiling')
    remaining=deadline-time.monotonic()
    if remaining<=0:raise RuntimeError('browser fixture inner deadline expired')
    return min(ceiling,remaining)


@contextmanager
def io_deadline(deadline, storage):
    if threading.current_thread() is not threading.main_thread():raise RuntimeError('fixture I/O guard requires the main thread')
    previous_mask=signal.pthread_sigmask(signal.SIG_BLOCK,{signal.SIGALRM})
    previous_handler=signal.getsignal(signal.SIGALRM)
    try:
        if signal.SIGALRM in previous_mask or any(signal.getitimer(signal.ITIMER_REAL)) or signal.SIGALRM in signal.sigpending():
            raise RuntimeError('fixture I/O guard conflicts with existing signal ownership')
        def tick(*_):
            signal.setitimer(signal.ITIMER_REAL,dispatch_timeout(deadline,storage,.1))
        signal.signal(signal.SIGALRM,tick)
        try:
            tick()
            signal.pthread_sigmask(signal.SIG_SETMASK,previous_mask)
            yield
        finally:
            signal.pthread_sigmask(signal.SIG_BLOCK,{signal.SIGALRM})
            signal.setitimer(signal.ITIMER_REAL,0)
            if signal.SIGALRM in signal.sigpending():signal.sigwait({signal.SIGALRM})
            signal.signal(signal.SIGALRM,previous_handler)
    finally:
        signal.pthread_sigmask(signal.SIG_SETMASK,previous_mask)


def read_response(response, maximum, deadline, storage, ceiling):
    reader=response.fp if isinstance(response,urllib.error.HTTPError) else response
    chunks=bytearray()
    while reader.fp is not None:
        timeout=dispatch_timeout(deadline,storage,ceiling)
        sock=getattr(getattr(reader.fp,'raw',None),'_sock',None)
        if sock is None:raise RuntimeError('cannot enforce fixture receive timeout')
        sock.settimeout(timeout)
        part=reader.read1(65536 if maximum is None else min(65536,maximum+1-len(chunks)))
        dispatch_timeout(deadline,storage,ceiling)
        if not part:break
        chunks.extend(part)
        if maximum is not None and len(chunks)>maximum:raise RuntimeError('TLS response exceeds existing query response bound')
    dispatch_timeout(deadline,storage,ceiling)
    return bytes(chunks)


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def validate_product_source_options(args, parser):
    values = (args.product_source_root, args.product_source_commit,
              args.console_receipt_sha256, args.console_source_helper,
              args.console_source_helper_sha256)
    if not any(value is not None for value in values):
        return False
    if not all(value is not None for value in values):
        parser.error('explicit console product source requires the complete five-operand tuple')
    if args.packaged_console is None:
        parser.error('explicit console product source requires --packaged-console')
    if not args.product_source_root.is_absolute() or not args.console_source_helper.is_absolute():
        parser.error('explicit console source root and helper must be absolute paths')
    if not re.fullmatch('[a-f0-9]{40}', args.product_source_commit):
        parser.error('explicit console product commit must be a full lowercase SHA-1')
    if not re.fullmatch('[a-f0-9]{64}', args.console_receipt_sha256):
        parser.error('explicit console receipt pin must be a full lowercase SHA-256')
    if not re.fullmatch('[a-f0-9]{64}', args.console_source_helper_sha256):
        parser.error('explicit console helper pin must be a full lowercase SHA-256')
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build', type=Path, required=True)
    parser.add_argument('--server', type=Path, required=True)
    parser.add_argument('--spindle', type=Path, required=True)
    parser.add_argument('--packaged-console',type=Path,help='use extracted package assets after cross-checking the frozen build')
    parser.add_argument('--product-source-root', type=Path)
    parser.add_argument('--product-source-commit')
    parser.add_argument('--console-receipt-sha256')
    parser.add_argument('--console-source-helper', type=Path)
    parser.add_argument('--console-source-helper-sha256')
    parser.add_argument('--browser-entry-helper', type=Path)
    parser.add_argument('--browser-entry-helper-sha256')
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--inject-rejection-defect',choices=('error','status'))
    parser.add_argument('--inject-api-cache-defect', action='store_true')
    parser.add_argument('--inject-role-defect', action='store_true',
                        help='negative control: report a reader token as admitted on an owner route')
    parser.add_argument('--inject-nonresident-enrollment', action='store_true',
                        help='negative control: weaken only browser enrollment options')
    parser.add_argument('--browser', choices=('chrome','firefox'), default='chrome')
    parser.add_argument('--certutil',type=Path,default=Path(shutil.which('certutil') or str(DATA/'tools/ui/nss-tools-3.129.0-1.fc44/usr/bin/certutil')))
    parser.add_argument('--expired-cursor-probe',action='store_true')
    parser.add_argument('--recovery',action='store_true')
    parser.add_argument('--shell-lifecycle',action='store_true')
    parser.add_argument('--pressure',action='store_true',help='finite authenticated body/admission timeout probe')
    parser.add_argument('--poll-seconds',type=int,choices=(0,120),default=0)
    parser.add_argument('--role-matrix',action='store_true',help='complete 144-cell role matrix; finish after existing setup')
    parser.add_argument('--session-expiry-helper',type=Path,help='separate controlled-time AB/C cell using an already admitted native helper')
    parser.add_argument('--session-expiry-admission',type=Path,help='successful exact-helper native admission receipt')
    parser.add_argument('--candidate-commit',help='exact candidate commit for controlled-time receipts')
    parser.add_argument('--package',type=Path,help='exact candidate package for controlled-time receipts')
    parser.add_argument('--security-probes',action='store_true',help='signed virtual-key, replay, fixation and real reader expiry cell')
    parser.add_argument('--inject-security-defect',choices=('registration-replay','login-replay','wrong-origin','wrong-rp','missing-uv','attacker-cookie','previous-cookie','reader-expiry'))
    parser.add_argument('--ui-responses',action='store_true',help='isolated diagnostic query response handling')
    parser.add_argument('--inject-ui-response-defect',choices=('empty','incomplete','gap','410','429','503','transport','timeout','403','401'))
    parser.add_argument('--migration',action='store_true',help='isolated packaged legacy-to-passkey transition and format1 refusal')
    parser.add_argument('--fabricctl',type=Path,help='exact extracted candidate CLI for migration')
    parser.add_argument('--announcement-probes',action='store_true',help='isolated actual console AT, keyboard and target inventory cell')
    parser.add_argument('--announcement-xvfb-admission',type=Path)
    parser.add_argument('--announcement-orca-admission',type=Path)
    parser.add_argument('--suppress-announcement',action='store_true',help='negative control: disable the original status live region')
    parser.add_argument('--security-key-probe',action='store_true',help='isolated actual virtual credential export/DER/signature admission')
    args = parser.parse_args()
    if args.browser_entry_helper is None or args.browser_entry_helper_sha256 is None:
        parser.error('explicit pinned native browser entry helper pair required')
    if not args.browser_entry_helper.is_absolute() or not re.fullmatch(
            '[a-f0-9]{64}', args.browser_entry_helper_sha256):
        parser.error('browser entry helper path must be absolute and pin must be a full lowercase SHA-256')
    product_source_enabled = validate_product_source_options(args, parser)
    if args.security_key_probe and any((getattr(args,'announcement_probes',False),getattr(args,'suppress_announcement',False),
            args.migration,args.fabricctl,args.security_probes,args.session_expiry_helper,
            args.session_expiry_admission,args.ui_responses,args.role_matrix,args.poll_seconds,args.pressure,
            args.expired_cursor_probe,args.shell_lifecycle,args.recovery,args.inject_rejection_defect,
            args.inject_api_cache_defect,args.inject_role_defect,args.inject_nonresident_enrollment,
            args.inject_security_defect,args.inject_ui_response_defect)):
        parser.error('security key admission requires its own finite cell')
    import browser_announcements
    browser_announcements.validate_mode(args,parser)
    if args.inject_ui_response_defect and not args.ui_responses:parser.error('UI response defect requires its own cell')
    if args.ui_responses and (args.migration or args.session_expiry_helper or args.security_probes or args.role_matrix or args.poll_seconds or args.pressure or args.expired_cursor_probe or args.shell_lifecycle or args.recovery):parser.error('UI responses use a separate finite cell')
    if args.migration and (not args.fabricctl or args.ui_responses or args.session_expiry_helper or args.security_probes or args.role_matrix or args.poll_seconds or args.pressure or args.expired_cursor_probe or args.shell_lifecycle or args.recovery or any((args.inject_rejection_defect,args.inject_api_cache_defect,args.inject_role_defect,args.inject_nonresident_enrollment,args.inject_security_defect))):
        parser.error('migration requires exact fabricctl and its own ordinary-clock cell')
    if args.fabricctl and not args.migration:
        parser.error('fabricctl is reserved for the isolated migration cell')
    if args.inject_security_defect and not args.security_probes:
        parser.error('security defect requires its isolated security cell')
    if args.security_probes and (args.session_expiry_helper or args.role_matrix or args.poll_seconds or args.pressure or args.expired_cursor_probe or args.shell_lifecycle or args.recovery):
        parser.error('security probes use a separate finite cell')
    if args.role_matrix and (args.poll_seconds or args.pressure or args.expired_cursor_probe or args.shell_lifecycle or args.recovery):
        parser.error('role matrix is a separate finite cell; polling/pressure/cursor/shell/recovery retain their own cells')
    expiry=args.session_expiry_helper is not None
    if expiry and (args.role_matrix or args.poll_seconds or args.pressure or args.expired_cursor_probe or args.shell_lifecycle or args.recovery
            or any((args.inject_rejection_defect,args.inject_api_cache_defect,args.inject_role_defect,args.inject_nonresident_enrollment))):
        parser.error('session expiry is a separate finite cell')
    if expiry and (not args.session_expiry_admission or not args.package or not re.fullmatch('[a-f0-9]{40}',args.candidate_commit or '')):
        parser.error('session expiry requires exact native admission, candidate commit and package')
    fixture_started = time.monotonic()
    limits = require_limits()
    # The combined browser/server fixture uses a stricter cap than build jobs.
    subprocess.run(['systemctl','--user','set-property','--runtime',limits.name,
                    'MemoryHigh=3500000000','MemoryMax=4000000000','MemorySwapMax=0',
                    'CPUQuota=200%','TasksMax=512'], check=True, capture_output=True)
    memory_max = int((limits/'memory.max').read_text().strip())
    if not 3999995904 <= memory_max <= 4000000000 or (limits/'memory.swap.max').read_text().strip()!='0':
        raise SystemExit('combined fixture 4 GB cap was not enforced')
    out = args.out.resolve()
    build = args.build.resolve()
    if not out.is_relative_to(DATA / 'results') or out.exists() or not build.is_relative_to(DATA / 'results'):
        raise SystemExit('fresh owned data-drive result directories required')
    for binary in (args.server, args.spindle, args.browser_entry_helper,
                   *([args.fabricctl] if args.migration else []),
                   *([args.console_source_helper] if product_source_enabled else [])):
        if not binary.resolve().is_relative_to(DATA) or not binary.is_file():
            raise SystemExit('project binaries must be existing data-drive artifacts')
    if hashlib.sha256(args.browser_entry_helper.read_bytes()).hexdigest() != args.browser_entry_helper_sha256:
        raise SystemExit('browser entry helper differs from caller pin')
    out.mkdir()
    work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / 'live-console-browser'
    work.mkdir(mode=0o700)
    checks = []
    receipt = {'command': sys.argv, 'exit': 1, 'checks': checks, 'build': str(build),
               'browser':args.browser,'browser_version_expected': VERSION if args.browser=='chrome' else '157.0.1', 'fixture_storage': str(work), 'combined_memory_cap_bytes':4000000000,'fixture_storage_cap_bytes':512*1024*1024,
               'enforced_limits':{key:(limits/key).read_text().strip() for key in ('memory.max','memory.high','memory.swap.max','cpu.max','pids.max')},
               'scope': 'finite virtual CTAP2 desktop/narrow browser integration; not hardware/mobile/installed-app qualification',
               'tls_scope': 'exact fixture leaf SPKI trusted by this isolated browser only; Spindle trusts fixture CA',
               'console_product_source': None,
               'injected_api_cache_defect': args.inject_api_cache_defect,
               'injected_role_defect': args.inject_role_defect,
               'injected_nonresident_enrollment': args.inject_nonresident_enrollment,
               'injected_rejection_defect':args.inject_rejection_defect,
               'source_sha256': {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                                 for p in (Path(__file__), ROLE_FIXTURE, ROOT / 'tools/ui/fixture_cadence.py', ROOT / 'tools/ui/browser_producer.py', ROOT / 'tools/packaging/stage_console.py')},
               'binaries': {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in
                   (args.server, args.spindle, args.browser_entry_helper,
                    *([args.fabricctl] if args.migration else []),
                    *([args.console_source_helper] if product_source_enabled else []))},
               'browser_download': json.loads((TOOLS / 'download.json').read_text()) if args.browser=='chrome' else json.loads((DATA/'tools/ui/geckodriver.json').read_text())}
    processes = []
    logs = []
    session = None
    viewport_websocket_url = None
    viewport_context_handle = None
    viewport_restore_required = False
    viewport_helper_attempt = 0
    driver_port = free_port()
    endpoint = f'http://127.0.0.1:{driver_port}'
    server_port = free_port()
    origin = f'https://localhost:{server_port}'
    context = None
    tls_opener = None
    clock_targets = {}
    state_descriptors = []
    announcements = None
    deadline = fixture_started + (1200 if args.expired_cursor_probe else 240)
    group_parent, owned_groups = begin_browser_domain(limits, deadline), []
    samples = []
    stop_samples = threading.Event()
    storage_violation = threading.Event()

    def sample():
        while not stop_samples.wait(.5):
            group = limits
            footprint=0
            for path in work.rglob('*'):
                try:
                    if path.is_file():footprint+=path.stat().st_size
                except FileNotFoundError:pass
            entry = {'monotonic_ns': time.monotonic_ns(), 'processes': {}, 'fixture_bytes': footprint}
            if entry['fixture_bytes'] > 512 * 1024 * 1024:
                entry['storage_bound_exceeded'] = True
                storage_violation.set()
            for label, proc in processes:
                try:
                    fields = dict(line.split(':', 1) for line in Path(f'/proc/{proc.pid}/status').read_text().splitlines() if ':' in line)
                    entry['processes'][label] = {key: fields.get(key, '').strip() for key in ('VmRSS', 'VmHWM', 'Threads')}
                except FileNotFoundError:
                    pass
            if group and group.exists():
                entry['cgroup'] = {key: (group / key).read_text().strip() for key in ('memory.current', 'memory.peak', 'memory.events', 'memory.swap.current', 'pids.current', 'pids.peak', 'pids.events', 'cpu.stat')}
            samples.append(entry)

    def wait(predicate, label, seconds=25):
        until = min(deadline, time.monotonic() + seconds)
        while time.monotonic() < until:
            if storage_violation.is_set():raise RuntimeError("owned fixture exceeds 512 MiB storage ceiling")
            if predicate():
                return
            time.sleep(.1)
        raise AssertionError('timeout: ' + label)

    def healthy(process):
        if process.poll() is not None:
            raise RuntimeError('server exited; inspect server.log')
        try:
            with urllib.request.urlopen(origin + '/v1/health', context=context, timeout=1) as response:
                return response.status == 200
        except (OSError, urllib.error.URLError):
            return False

    def check(condition, label):
        if not condition:
            raise AssertionError(label)
        checks.append(label)

    def call(method, path, body=None):
        dispatch_timeout(deadline,storage_violation,35)
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(endpoint + path, data=data, method=method,
                                         headers={'Content-Type': 'application/json'})
        with io_deadline(deadline,storage_violation):
            try:
                with urllib.request.urlopen(request, timeout=dispatch_timeout(deadline,storage_violation,35)) as response:
                    value=json.loads(read_response(response,None,deadline,storage_violation,35)).get('value')
                    dispatch_timeout(deadline,storage_violation,35)
                    return value
            except urllib.error.HTTPError as error:
                raise RuntimeError(read_response(error,None,deadline,storage_violation,35).decode()) from error

    def wd(path, body=None, method='POST'):
        return call(method, f'/session/{session}' + path, body)

    def js(script, *values):
        return wd('/execute/sync', {'script': script, 'args': values})

    def run_viewport(mode, context_handle):
        nonlocal viewport_helper_attempt
        if args.browser != 'firefox' or mode not in ('narrow', 'restore'):
            raise ValueError('BiDi viewport adapter is Firefox-only and mode-bounded')
        if not isinstance(viewport_websocket_url, str) or not context_handle:
            raise ValueError('Firefox BiDi endpoint or classic window context is missing')
        viewport_helper_attempt += 1
        label = 'browser-bidi-viewport-' + str(viewport_helper_attempt) + '-' + mode
        command = [str(args.browser_entry_helper), 'browser-bidi-viewport',
                   '--ws-url', viewport_websocket_url, '--context', context_handle,
                   '--mode', mode]
        helper_log = (out / (label + '.log')).open('w+b')
        logs.append(helper_log)
        helper_environment = {'PATH': '/usr/bin:/bin'}
        helper = browser_group_launch(group_parent, owned_groups, label, command,
            helper_log, helper_environment, entry_helper=args.browser_entry_helper,
            entry_helper_sha256=args.browser_entry_helper_sha256)
        processes.append((label, helper))
        binding = owned_groups[-1]
        helper_timeout = min(5, max(0, deadline - time.monotonic()))
        try:
            exit_code = helper.wait(timeout=helper_timeout)
        except subprocess.TimeoutExpired as error:
            stop_browser_groups(group_parent, [binding])
            raise ValueError('BiDi viewport helper exceeded browser fixture deadline') from error
        stop_browser_groups(group_parent, [binding])
        helper_log.flush()
        helper_size = helper_log.tell()
        helper_log.seek(0)
        output = helper_log.read(64 * 1024 + 1)
        if helper_size > 64 * 1024 or len(output) > 64 * 1024:
            raise ValueError('BiDi viewport helper report exceeds 64 KiB read bound')
        if (binding['group'].get('kernel_empty') is not True
                or binding['group'].get('children_reaped') is not True):
            raise ValueError('BiDi viewport helper group did not stop cleanly')
        if exit_code != 0:
            raise ValueError('BiDi viewport helper exited unsuccessfully')
        try:
            acknowledgement = json.loads(output)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ValueError('BiDi viewport helper report is not valid JSON') from error
        expected = {'command': 'browsingContext.setViewport', 'context': context_handle,
                    'mode': mode, 'result': {}, 'received_messages': 2,
                    'actual_viewport_verified': False}
        if (not isinstance(acknowledgement, dict)
                or set(acknowledgement) != set(expected)
                or acknowledgement != expected
                or type(acknowledgement.get('received_messages')) is not int
                or type(acknowledgement.get('actual_viewport_verified')) is not bool):
            raise ValueError('BiDi viewport helper acknowledgement did not match request')
        return acknowledgement

    def verify_viewport_geometry(actual, baseline):
        check(isinstance(actual,dict) and actual.get('width')==390
              and actual.get('height')==844 and actual.get('dpr')==baseline.get('dpr'),
              'Firefox BiDi viewport has actual 390 by 844 CSS pixels at unchanged DPR')

    def async_js(script, *values):
        return wd('/execute/async', {'script': script, 'args': values})

    def click(text):
        cadence.pace()
        button=js('const button=Array.from(document.querySelectorAll("button")).find(e=>e.textContent.trim()===arguments[0]); if(!button)throw Error("button absent:"+arguments[0]);return button', text)
        wd('/element/'+button['element-6066-11e4-a52e-4f735466cecf']+'/click',{})

    def field(label, value):
        js('const label=Array.from(document.querySelectorAll("label")).find(e=>e.textContent.trim().startsWith(arguments[0]));if(!label)throw Error("label absent:"+arguments[0]);const input=label.querySelector("input,textarea,select");input.value=arguments[1];input.dispatchEvent(new Event(input.tagName==="SELECT"?"change":"input",{bubbles:true}))', label, value)

    def nav(text):
        js('Array.from(document.querySelectorAll(".sidebar .nav-item")).find(e=>e.textContent.includes(arguments[0])).click()', text)

    cadence = FixtureCadence()
    receipt['benign_explicit_action_interval_ms'] = 550

    def pace_api():
        dispatch_timeout(deadline,storage_violation,35)
        if session and js('return !!window.fixturePace'):
            async_js('const done=arguments[arguments.length-1];window.fixturePace().then(()=>done(true))')
        cadence.pace()
        dispatch_timeout(deadline,storage_violation,35)

    def capture_api_metadata():
        import math
        receipt['actual_api_headers']=js('return window.fixtureApiHeaders')
        rows=receipt['actual_api_headers']
        check(isinstance(rows,list) and len(rows)<=512 and not js('return window.fixtureApiHeadersTruncated'),'browser API ledger is a complete bounded list')
        responses=[]
        cancellations=[]
        finite=lambda value: type(value) in (int,float) and math.isfinite(value)
        for item in rows:
            check(isinstance(item,dict),'browser API ledger entries are objects')
            if item.get('state')=='response':
                check(type(item.get('status')) is int,'browser API response has an integer HTTP status')
                responses.append(item)
                continue
            started=item.get('started_ms')
            finished=item.get('finished_ms')
            observed=item.get('signal_abort_observed_ms')
            witnessed_abort=(item.get('state')=='rejected' and item.get('path')=='/v1/console/query' and item.get('method')=='POST' and item.get('error_stage')=='fetch' and item.get('error_name')=='AbortError' and item.get('status') is None and item.get('signal_aborted_at_rejection') is True and finite(started) and finite(observed) and finite(finished) and started<=observed<=finished)
            check(witnessed_abort,'only a witnessed protected-query AbortSignal cancellation has no HTTP response')
            cancellations.append(item)
        receipt['actual_api_cancellations']=cancellations
        check(bool(responses),'browser API ledger contains an actual HTTP response')
        check(all(item.get('cache_control')=='no-store' and item.get('csp') and item.get('coop')=='same-origin' and item.get('corp')=='same-origin' for item in responses),'actual HTTP responses carry no-store CSP COOP and CORP')

    def verify_binary_identity():
        receipt['native_input_hashes_after']={name:hashlib.sha256(Path(name).read_bytes()).hexdigest() for name in receipt['binaries']}
        receipt['native_snapshot_hashes_after']={role:hashlib.sha256(getattr(args,role).read_bytes()).hexdigest() for role in receipt['immutable_binary_snapshots']}
        check(receipt['native_input_hashes_after']==receipt['binaries'],'native input artifact identities remain unchanged throughout run')
        check(all(receipt['native_snapshot_hashes_after'][role]==identity['sha256'] for role,identity in receipt['immutable_binary_snapshots'].items()),'all fixture launches retain immutable candidate binary bytes')

    def expect(answer, status, message, label):
        code, body = answer
        receipt.setdefault('semantic_rejections', []).append({'label':label,'status':code,'error':body.get('error') if isinstance(body,dict) else None})
        check(semantic_rejection(answer,status,message), label)

    def api(path, method='GET', body=None, version=True, csrf=True):
        query_csrf=None
        if csrf and method!='GET' and path.startswith('/v1/console/') and '/auth/' not in path:
            session_status,session_view=api('/v1/console/session')
            if session_status==200:query_csrf=session_view.get('csrf')
        if not js('return !!window.fixturePace'):cadence.pace()
        # Decode returned text in Python, never JS JSON.parse on telemetry.
        answer = async_js('''const [path,method,body,version,csrf]=arguments;const done=arguments[arguments.length-1];
          const headers={};if(csrf)headers['x-fabric-csrf']=csrf;if(version)headers['x-fabric-client-version']='1';if(body!==null)headers['content-type']='application/json';
          fetch(path,{method,headers,body:body===null?undefined:body,cache:'no-store',credentials:'same-origin'})
          .then(async response=>done({status:response.status,text:await response.text()})).catch(error=>done({error:String(error)}));''',
          path, method, None if body is None else json.dumps(body), version, query_csrf)
        if 'error' in answer:
            raise RuntimeError(answer['error'])
        body=json.loads(answer['text']) if answer['text'] else None
        if answer['status']>=400:receipt.setdefault('api_rejections',[]).append({'path':path.split('?')[0],'status':answer['status'],'error':body.get('error') if isinstance(body,dict) else None})
        if args.inject_rejection_defect and path=='/v1/console/auth/register/start' and body=={'error':'access denied'}:
            receipt['rejection_injection_actual_witness']={'status':answer['status'],'error':body['error']}
            if args.inject_rejection_defect=='error':body={'error':'different rejection mechanism'}
            else:answer['status']=429
        return answer['status'],body

    def bearer_api(token,path,method='GET',body=None,*,cookie=None,csrf=None,role=None,paced=True):
        dispatch_timeout(deadline,storage_violation,15)
        if paced:pace_api()
        dispatch_timeout(deadline,storage_violation,15)
        request=urllib.request.Request(origin+path,method=method,data=None if body is None else json.dumps(body).encode(),headers={
            **({'Authorization':'Bearer '+token} if token else {}),**({'Cookie':cookie} if cookie else {}),
            **({'x-fabric-csrf':csrf} if csrf else {}),'Origin':origin,'x-fabric-client-version':'1','Content-Type':'application/json'})
        entry={'role':role,'method':method,'path':path,'started_ns':time.monotonic_ns(),'status':None}
        calls=receipt.setdefault('explicit_tls_requests',[])
        if len(calls)>=1024:raise RuntimeError('explicit request ledger exceeds its bound')
        calls.append(entry)
        try:
            with io_deadline(deadline,storage_violation):
                try:
                    with tls_opener.open(request,timeout=dispatch_timeout(deadline,storage_violation,15)) as response:
                        code,text=response.status,read_response(response,8*1024*1024,deadline,storage_violation,15)
                except urllib.error.HTTPError as error:
                    code,text=error.code,read_response(error,8*1024*1024,deadline,storage_violation,15)
            entry['status']=code
            try:response=json.loads(text) if text else None
            except json.JSONDecodeError:response=None
            field='status' if path in ('/v1/batches','/v1/config') else 'error'
            entry['refusal']=response.get(field) if code>=400 and isinstance(response,dict) else None
            result=code,response
            dispatch_timeout(deadline,storage_violation,15)
        finally:
            entry['finished_ns']=time.monotonic_ns()
            cadence.last=cadence.clock()
        if args.inject_role_defect and token and path=='/v1/console/audit' and result[0]==403:
            receipt['role_injection_actual_witness']={'status':result[0],'error':result[1].get('error')}
            return 200,{'events':[]}
        return result

    def cdp(command, params):
        return wd('/goog/cdp/execute', {'cmd': command, 'params': params})

    def add_authenticator():
        options={'protocol':'ctap2','transport':'internal','hasResidentKey':True,'hasUserVerification':True,'isUserVerified':True}
        if args.browser=='firefox':
            # Gecko's CTAP 2.0 test token omits the credential ID after filtering
            # an original two-key allowlist to one. Its bridge restores the ID
            # only for an originally single-key list. CTAP 2.1 retains that ID.
            options['protocol']='ctap2_1'
            options['transport']='usb'
            options['isUserConsenting']=True
            # geckodriver #2239 renames fields that Gecko expects in camelCase.
            # Use Gecko's actual virtual authenticator implementation from the
            # privileged fixture context; content still runs real WebAuthn.
            wd('/moz/context',{'context':'chrome'})
            try:
                return js('return ChromeUtils.importESModule("chrome://remote/content/marionette/webauthn.sys.mjs").webauthn.addVirtualAuthenticator(arguments[0])',options)
            finally:
                wd('/moz/context',{'context':'content'})
        options['automaticPresenceSimulation']=True
        return cdp('WebAuthn.addVirtualAuthenticator',{'options':options})['authenticatorId']

    def remove_authenticator(identifier):
        if args.browser=='firefox':
            wd('/moz/context',{'context':'chrome'})
            try:
                return js('ChromeUtils.importESModule("chrome://remote/content/marionette/webauthn.sys.mjs").webauthn.removeVirtualAuthenticator(arguments[0]);return null',identifier)
            finally:
                wd('/moz/context',{'context':'content'})
        return cdp('WebAuthn.removeVirtualAuthenticator',{'authenticatorId':identifier})

    def resident_credentials(identifier, label):
        # Inventory belongs to the browser fixture, not credProps from the page.
        # Gecko returns only public booleans/RP here; never persist private keys.
        if args.browser == 'firefox':
            wd('/moz/context', {'context':'chrome'})
            try:
                inventory = js('''return ChromeUtils.importESModule("chrome://remote/content/marionette/webauthn.sys.mjs").webauthn
                  .getCredentials(arguments[0]).map(c=>({resident:c.isResidentCredential,rp:c.rpId}));''', identifier)
            finally:
                wd('/moz/context', {'context':'content'})
        else:
            inventory = [{'resident':c.get('isResidentCredential'), 'rp':c.get('rpId')}
                         for c in cdp('WebAuthn.getCredentials', {'authenticatorId':identifier})['credentials']]
        receipt.setdefault('resident_credential_witnesses', []).append(
            {'phase':label, 'count':len(inventory), 'credentials':inventory})
        check(bool(inventory) and all(c['resident'] is True and c['rp']=='localhost' for c in inventory),
              label + ' stores actual resident credentials for the fixture RP')

    def launch(label, command, controlled=False):
        nonlocal announcements
        environment=os.environ.copy()
        for key in ('XDG_DATA_HOME','XDG_CONFIG_HOME','XDG_CACHE_HOME'):
            directory=work/key.lower();directory.mkdir(exist_ok=True);environment[key]=str(directory)
        if controlled:
            from session_clock import SessionClock
            def spawn(argv,env,fd,helper,log):
                return browser_group_launch(group_parent,owned_groups,label,argv,log,env,
                    entry_helper=args.browser_entry_helper,
                    entry_helper_sha256=args.browser_entry_helper_sha256,clock=(helper,fd))
            clock=SessionClock(args.server,args.session_expiry_helper,out/f'{label}.log',deadline,
                server_argv=command,server_launch=spawn)
            clock_targets[label]=clock;proc=clock.process
        else:
            if label=='webdriver' and args.announcement_probes:
                announcements=browser_announcements.Runtime(work,out,args,deadline)
                command=announcements.command(command)
            log = (out / f'{label}.log').open('w');logs.append(log)
            proc = browser_group_launch(group_parent, owned_groups, label, command, log, environment,
                entry_helper=args.browser_entry_helper,
                entry_helper_sha256=args.browser_entry_helper_sha256)
            if label=='webdriver' and announcements is not None:announcements.process=proc
        processes.append((label, proc))
        return proc

    sampler = threading.Thread(target=sample, daemon=True)
    sampler.start()
    try:
        artifact_dir=work/'binaries';artifact_dir.mkdir(mode=0o700)
        snapshots={}
        for role in ('server','spindle', *(['fabricctl'] if args.migration else []),
                     *(['console_source_helper'] if product_source_enabled else []),
                     'browser_entry_helper'):
            source=getattr(args,role)
            destination=artifact_dir/role
            shutil.copyfile(source,destination)
            destination.chmod(0o700)
            identity=hashlib.sha256(destination.read_bytes()).hexdigest()
            if identity!=receipt['binaries'][str(source)]:
                raise ValueError('native binary changed before fixture snapshot')
            snapshots[role]={'source':str(source),'sha256':identity,'bytes':destination.stat().st_size}
            setattr(args,role,destination)
        receipt['immutable_binary_snapshots']=snapshots
        receipt['browser_entry_helper'] = {
            'caller_path': str(args.browser_entry_helper),
            'caller_sha256': args.browser_entry_helper_sha256,
            'snapshot_path': str(artifact_dir / 'browser_entry_helper'),
            'snapshot_sha256': snapshots['browser_entry_helper']['sha256']}
        args.browser_entry_helper = artifact_dir / 'browser_entry_helper'
        if product_source_enabled:
            args.console_source_helper = artifact_dir / 'console_source_helper'
        if expiry:
            from session_clock import digest
            helper=args.session_expiry_helper.resolve();admission=args.session_expiry_admission.resolve();package=args.package.resolve()
            check(all(path.is_relative_to(DATA) and path.is_file() for path in (helper,admission,package)), 'controlled-time inputs reside on data drive')
            native=json.loads(admission.read_text())
            check(native.get('state')=='completed' and native.get('exit')==0 and native.get('retained_helper')=={'path':str(helper),'sha256':digest(helper)}, 'exact retained helper has successful native admission')
            receipt['native_clock_source_binding']=native_clock_sources(native['hashes'],ROOT)
            args.session_expiry_helper=helper
            receipt['session_expiry_inputs']={'candidate_commit':args.candidate_commit,'package_sha256':digest(package),
                'package_path':str(package),'helper_sha256':digest(helper),'native_admission':str(admission),'native_admission_sha256':digest(admission)}
        check(semantic_rejection((403,{'error':'access denied'}),403,'access denied') and not semantic_rejection((429,{'error':'access denied'}),403,'access denied') and not semantic_rejection((403,{'error':'wrong error'}),403,'access denied'),'semantic rejection grader rejects wrong status and wrong error negative controls')
        if product_source_enabled:
            console_source_evidence = {}
            receipt['console_product_source'] = {
                'source_root': str(args.product_source_root.resolve()),
                'source_commit': args.product_source_commit,
                'build_root': str(build),
                'receipt_sha256': args.console_receipt_sha256,
                'helper': snapshots['console_source_helper']['source'],
                'helper_sha256': args.console_source_helper_sha256,
                'harness_source_sha256': receipt['source_sha256'],
                'claim': 'caller-pinned product source and build receipt identity join; no acceptance credit'}
            try:
                def verify_console_source(snapshot, source_root, source_commit, build_root, receipt_pin):
                    argv = [str(snapshot), 'console-source', '--source-root', str(source_root),
                            '--source-commit', source_commit, '--build-root', str(build_root),
                            '--receipt-sha256', receipt_pin]
                    helper_log_path = out / 'console-source-helper.log'
                    helper_log = helper_log_path.open('w+b')
                    logs.append(helper_log)
                    helper_environment = {'PATH': '/usr/bin:/bin', 'FABRIC_STORAGE_ROOT': str(DATA),
                                          'FABRIC_SCRATCH_ROOT': str(Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve())}
                    helper = browser_group_launch(group_parent, owned_groups,
                        'console-source-verifier', argv, helper_log, helper_environment,
                        entry_helper=args.browser_entry_helper,
                        entry_helper_sha256=args.browser_entry_helper_sha256)
                    processes.append(('console-source-verifier', helper))
                    binding = owned_groups[-1]
                    helper_timeout = min(90, max(0, deadline - time.monotonic()))
                    try:
                        exit_code = helper.wait(timeout=helper_timeout)
                    except subprocess.TimeoutExpired as error:
                        stop_browser_groups(group_parent, [binding])
                        raise ValueError('console source helper exceeded browser fixture deadline') from error
                    stop_browser_groups(group_parent, [binding])
                    helper_log.flush()
                    helper_size = helper_log.tell()
                    helper_log.seek(0)
                    output = helper_log.read(64 * 1024 + 1)
                    console_source_evidence.update({'helper_log': str(helper_log_path),
                        'helper_log_read_sha256': hashlib.sha256(output).hexdigest(),
                        'helper_log_bytes': helper_size,
                        'helper_process_group_stopped': binding['group'].get('kernel_empty') is True
                            and binding['group'].get('children_reaped') is True})
                    if helper_size > 64 * 1024 or len(output) > 64 * 1024:
                        raise ValueError('console source helper report exceeds 64 KiB read bound')
                    if not console_source_evidence['helper_process_group_stopped']:
                        raise ValueError('console source helper group did not stop cleanly')
                    if exit_code != 0:
                        return None, exit_code, len(output), 0
                    try:
                        report = json.loads(output)
                    except (UnicodeDecodeError, json.JSONDecodeError) as error:
                        raise ValueError('console source helper returned invalid JSON') from error
                    return report, exit_code, len(output), 0

                stage(build, work / 'console',
                      product_source=(args.product_source_root.resolve(), args.product_source_commit,
                                      args.console_receipt_sha256, artifact_dir / 'console_source_helper',
                                      args.console_source_helper_sha256),
                      helper_runner=verify_console_source,
                      evidence=console_source_evidence)
            finally:
                receipt['console_product_source']['verification'] = console_source_evidence
        else:
            stage(build, work / 'console')
        if args.packaged_console:
            packaged=args.packaged_console.resolve()
            if not packaged.is_relative_to(DATA) or not packaged.is_dir():
                raise ValueError('extracted console must be an existing data-drive directory')
            staged=work/'console'
            if {path.name for path in packaged.iterdir()}!={path.name for path in staged.iterdir()}:
                raise ValueError('extracted package asset inventory differs from frozen build')
            identities={}
            for expected in staged.iterdir():
                source=packaged/expected.name
                if source.is_symlink() or not source.is_file() or source.stat().st_size>8*1024*1024:
                    raise ValueError('unsafe extracted package asset')
                body=source.read_bytes()
                if expected.name in ('console-headers.json','asset-manifest.json'):
                    if json.loads(body)!=json.loads(expected.read_bytes()):
                        raise ValueError('extracted package metadata differs from frozen build')
                elif body!=expected.read_bytes():
                    raise ValueError('extracted package asset differs from frozen build')
                identities[source.name]={'bytes':len(body),'sha256':hashlib.sha256(body).hexdigest()}
                shutil.copyfile(source,expected)
            receipt['extracted_packaged_console']={'path':str(packaged),'assets':identities}
            check(True,'actual extracted package console assets match frozen build before serving')
        if args.announcement_probes:browser_announcements.admit_package(args,receipt)
        if args.inject_api_cache_defect:
            worker=work/'console/service-worker.js'
            worker.write_text(worker.read_text()+'''
self.addEventListener("fetch",event=>{
  if(new URL(event.request.url).pathname.startsWith("/v1/")&&event.request.method==="GET")
    event.respondWith(fetch(event.request).then(async response=>{
      await(await caches.open("fabric-shell-injected-defect")).put(event.request,response.clone());return response;
    }));
});
''')
            manifest_path=work/'console/asset-manifest.json'
            manifest=json.loads(manifest_path.read_text())
            body=worker.read_bytes()
            manifest['service-worker.js']={'bytes':len(body),'sha256':hashlib.sha256(body).hexdigest()}
            manifest_path.write_text(json.dumps(manifest)+'\n')
        (work / 'san.ext').write_text('subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n')
        commands = [
            ['req','-x509','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-nodes','-keyout','ca.key','-out','ca.pem','-days','1','-subj','/CN=Fabric browser fixture CA','-addext','basicConstraints=critical,CA:TRUE','-addext','keyUsage=critical,keyCertSign,cRLSign'],
            ['req','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-nodes','-keyout','server.key','-out','server.csr','-subj','/CN=localhost'],
            ['x509','-req','-in','server.csr','-CA','ca.pem','-CAkey','ca.key','-CAcreateserial','-out','server.pem','-days','1','-extfile','san.ext']]
        for command in commands:
            subprocess.run(['openssl', *command], cwd=work, capture_output=True, check=True, timeout=10)
        public = subprocess.run(['openssl','x509','-in',str(work/'server.pem'),'-pubkey','-noout'], capture_output=True, check=True).stdout
        der = subprocess.run(['openssl','pkey','-pubin','-outform','DER'],input=public,capture_output=True,check=True).stdout
        spki = base64.b64encode(hashlib.sha256(der).digest()).decode()
        receipt['fixture_leaf_spki_sha256'] = spki
        admin = work / 'admin-token'
        admin.write_text(secrets.token_hex(32) + '\n')
        admin.chmod(0o600)
        config = work / 'server.conf'
        config.write_text(f'listen=127.0.0.1:{server_port}\ntls_cert={work}/server.pem\ntls_key={work}/server.key\nstate_dir={work}/state\nadmin_token_file={admin}\nconsole_dir={work}/console\naccess_origin={origin}\naccess_rp_id=localhost\njournal_bytes=8388608\njournal_file_bytes=1048576\nretention_s=86400\nretention_bytes=33554432\nseal_workers=1\nself_spindle_url={origin}\nself_spindle_ca={work}/ca.pem\nself_spindle_executable={args.spindle.resolve()}\n')
        migration_prepared = None
        if args.migration:
            import browser_migration
            args._migration_origin = origin
            def stop_migration(prefix):
                selected = [b for b in owned_groups if b['group']['label'].startswith(prefix)]
                check(bool(selected),'migration owned processes observed')
                stop_browser_groups(group_parent,selected)
                check(all(b['group'].get('kernel_empty') and b['group'].get('children_reaped') for b in selected),
                    'migration kernel-empty stopped descendants')
            receipt['source_sha256'].update({str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest()
                for p in (ROOT/'tools/ui/browser_migration.py',ROOT/'tools/ui/migration_contract.py')})
            migration_prepared = browser_migration.prepare(args,work,out,origin,config,admin,launch,stop_migration,wait,pace_api,receipt)
        def start_owner(label):
            nonlocal session,endpoint,context,tls_opener
            fixture=work/label if label else work
            if label:fixture.mkdir(mode=0o700)
            fixture_config=fixture/'server.conf'
            if label:fixture_config.write_text(config.read_text().replace(f'state_dir={work}/state',f'state_dir={fixture}/state'))
            state_record=None
            if expiry:
                from session_clock import kernel_time
                created=kernel_time(1);state=fixture/'state';state.mkdir(mode=0o700)
                held=browser_hold_directory(state)
                state_record=dict(path=str(state),device=held['dev'],inode=held['ino'],descriptor_held=True,
                    created_monotonic_ns=created,initial_entries=[p.name for p in state.iterdir()],copy_source=None,
                    observed_monotonic_ns=kernel_time(1))
                state_descriptors.append(held)
            driver_port=free_port();endpoint=f'http://127.0.0.1:{driver_port}'
            started=time.monotonic_ns()
            server = launch(label+'server', [str(args.server.resolve()), 'serve', str(fixture_config)], controlled=expiry)
            context = ssl.create_default_context(cafile=str(work / 'ca.pem'))
            tls_opener=urllib.request.build_opener(urllib.request.HTTPSHandler(context=context),NoRedirect())
            wait(lambda:healthy(server), 'production TLS server')
            build_receipt=json.loads((build/'build.json').read_text())
            served_assets=[]
            for name,identity in build_receipt['assets'].items():
                if args.inject_api_cache_defect and name=='service-worker.js':
                    identity=json.loads((work/'console/asset-manifest.json').read_text())[name]
                    receipt['injected_worker_identity']=identity
                with urllib.request.urlopen(origin+'/console/'+name,context=context,timeout=15) as response:
                    content=response.read(8*1024*1024+1)
                    mime=response.headers.get_content_type()
                    expected_mime={'html':'text/html','js':'text/javascript','css':'text/css','wasm':'application/wasm','webmanifest':'application/manifest+json','png':'image/png','svg':'image/svg+xml'}[name.rsplit('.',1)[1]]
                    # HTTP permits application/javascript as the legacy registered JS MIME.
                    correct_mime=mime==expected_mime or expected_mime=='text/javascript' and mime=='application/javascript'
                    check(len(content)==identity['bytes'] and hashlib.sha256(content).hexdigest()==identity['sha256'] and correct_mime,'actual TLS shell asset hash size and MIME: '+name)
                    check(all(response.headers.get(header)==value for header,value in build_receipt['headers'].items()),'actual shell headers match frozen build: '+name)
                    served_assets.append({'name':name,'bytes':len(content),'mime':mime})
            receipt['served_assets']=served_assets
            with urllib.request.urlopen(origin+'/console/manifest.webmanifest',context=context,timeout=5) as response:pwa_manifest=json.load(response)
            check(pwa_manifest['scope']=='/console/' and pwa_manifest['start_url']=='/console/','actual manifest limits shell scope and start URL')
            bootstrap_file = fixture / 'state/access/access-bootstrap.secret'
            wait(lambda: bootstrap_file.is_file(), 'one-time bootstrap')
            bootstrap = bootstrap_file.read_text().strip()
            if args.browser=='firefox':
                profile=fixture/'profile';profile.mkdir()
                certutil=args.certutil.resolve()
                for command in ([str(certutil),'-N','--empty-password','-d','sql:'+str(profile)],
                                [str(certutil),'-A','-d','sql:'+str(profile),'-n','Owned Fabric fixture CA','-t','C,,','-i',str(work/'ca.pem')]):
                    subprocess.run(command,check=True,capture_output=True,timeout=10)
                receipt['tls_scope']='fixture CA trusted only in owned Firefox NSS profile; acceptInsecureCerts false; Spindle trusts fixture CA'
                receipt['virtual_authenticator_driver_workaround']='geckodriver2239 camelCase mismatch; privileged fixture invokes Gecko WebAuthn module, then restores content context'
                driver=launch(label+'webdriver',[str(DATA/'tools/ui/geckodriver'),'--host','127.0.0.1','--port',str(driver_port),'--allow-system-access'])
            else:
                driver = launch(label+'webdriver', [str(TOOLS / 'chromedriver-linux64/chromedriver'), '--port=' + str(driver_port), '--allowed-ips=127.0.0.1'])
            def driver_ready():
                if driver.poll() is not None:
                    raise RuntimeError('driver exited; inspect webdriver.log')
                try:
                    return call('GET', '/status').get('ready') is True
                except (OSError, RuntimeError):
                    return False
            wait(driver_ready, 'Chrome WebDriver')
            chrome_capabilities={'browserName':'chrome','goog:chromeOptions': {
                'binary': str(TOOLS / 'chrome-linux64/chrome'),
                'args': ['--headless=new','--disable-dev-shm-usage','--user-data-dir=' + str(fixture/'profile'),
                         '--ignore-certificate-errors-spki-list=' + spki, '--window-size=1440,1100', '--no-first-run','--no-default-browser-check']}}
            if args.shell_lifecycle:
                chrome_capabilities['goog:chromeOptions']['args'].append('--remote-debugging-pipe')
            firefox_capabilities={'browserName':'firefox','acceptInsecureCerts':False,'moz:firefoxOptions':{
                'binary':'/usr/bin/firefox','args':['-headless','-profile',str(fixture/'profile')],
                # Firefox 157 dispatches USB before the software manager. A virtual
                # authenticator belongs to the software manager, so select that
                # fixture transport explicitly; the server still requires UV.
                'prefs':{'security.enterprise_roots.enabled':False,
                         'security.webauth.webauthn_enable_softtoken':True,
                         'security.webauth.webauthn_enable_usbtoken':False}}}
            firefox_capabilities['webSocketUrl'] = True
            if args.announcement_probes:
                firefox_capabilities['moz:firefoxOptions']['args'].remove('-headless')
                firefox_capabilities['moz:firefoxOptions']['prefs']['accessibility.force_disabled']=0
            info = call('POST','/session',{'capabilities':{'alwaysMatch':firefox_capabilities if args.browser=='firefox' else chrome_capabilities}})
            session = info['sessionId']
            if args.browser == 'firefox':
                viewport_websocket_url = info['capabilities'].get('webSocketUrl')
                if not isinstance(viewport_websocket_url, str):
                    raise ValueError('Firefox did not return the requested BiDi WebSocket URL')
            receipt['browser_version_actual'] = info['capabilities']['browserVersion']
            check(receipt['browser_version_actual'] == receipt['browser_version_expected'], 'actual browser matches reviewed version')
            if args.browser=='firefox':
                wd('/moz/context',{'context':'chrome'})
                try:
                    receipt['virtual_authenticator_transport_prefs']=js('return {software:Services.prefs.getBoolPref("security.webauth.webauthn_enable_softtoken",false),usb:Services.prefs.getBoolPref("security.webauth.webauthn_enable_usbtoken",true)}')
                finally:
                    wd('/moz/context',{'context':'content'})
                check(receipt['virtual_authenticator_transport_prefs']=={'software':True,'usb':False},
                      'virtual authenticator dispatch selects software transport instead of physical USB')
            wd('/timeouts', {'script': 35000, 'pageLoad': 30000, 'implicit': 0})
            if args.browser=='chrome':cdp('WebAuthn.enable', {})
            authenticator = add_authenticator()
            wd('/window/rect',{'width':1440,'height':1100})
            receipt['virtual_authenticator'] = {
                'protocol':'ctap2_1' if args.browser=='firefox' else 'ctap2',
                'transport':'usb' if args.browser=='firefox' else 'internal',
                'user_verified':True,'resident_key_capability':True}
            wd('/url', {'url': origin + '/console/'})
            wait(lambda: js('return !!document.querySelector("[data-testid=connect-server]")'), 'WASM mounted')
            check(js('return !!document.querySelector("[data-testid=locked-screen]")'), 'initial view is locked')
            receipt['cold_shell_timing']=js('return {navigation:performance.getEntriesByType("navigation")[0]?.toJSON(),assets:performance.getEntriesByType("resource").map(e=>({path:new URL(e.name).pathname,duration_ms:e.duration,transfer_bytes:e.transferSize,encoded_bytes:e.encodedBodySize}))}')
            async_js('const done=arguments[arguments.length-1];navigator.serviceWorker.ready.then(()=>done(true)).catch(error=>done({error:String(error)}))')
            # Gecko WebDriver refresh bypasses worker control. Ordinary navigation
            # exercises the installed public-shell worker in both engines.
            js('location.assign(location.href)')
            wait(lambda: js('return !!document.querySelector("[data-testid=connect-server]") && !!navigator.serviceWorker.controller'), 'public worker controls reloaded shell')
            check(async_js('const done=arguments[arguments.length-1];navigator.serviceWorker.getRegistration().then(r=>done(new URL(r.scope).pathname==="/console/" && new URL(r.active.scriptURL).pathname==="/console/service-worker.js"))'),'actual worker registration and script remain inside console scope')
            receipt['warm_shell_timing']=js('return {navigation:performance.getEntriesByType("navigation")[0]?.toJSON(),assets:performance.getEntriesByType("resource").map(e=>({path:new URL(e.name).pathname,duration_ms:e.duration,transfer_bytes:e.transferSize,encoded_bytes:e.encodedBodySize}))}')
            js('''window.fixtureApiHeaders=[];window.fixtureApiHeadersTruncated=false;window.fixturePaceQueue=Promise.resolve();window.fixtureLastDispatch=-Infinity;
              const interval=arguments[0],original=window.fetch;
              window.fixturePace=(dispatch)=>{const slot=window.fixturePaceQueue.then(async()=>{let delay;while((delay=interval-(performance.now()-window.fixtureLastDispatch))>0)await new Promise(resolve=>setTimeout(resolve,delay));const started=performance.now();window.fixtureLastDispatch=started;return {response:dispatch?.(started)}});window.fixturePaceQueue=slot.then(()=>{},()=>{});return slot};
              window.fetch=async(...args)=>{const url=typeof args[0]==="string"?args[0]:args[0].url;const path=new URL(url,location.href).pathname;
                if(!path.startsWith('/v1/'))return original(...args);const paced=!window.fixtureUnpacedBurst;
                const dispatch=async(started_ms)=>{const entry={path,method:args[1]?.method||args[0]?.method||'GET',started_ms,paced,status:null,state:'pending'};
                if(window.fixtureApiHeaders.length>=512)window.fixtureApiHeadersTruncated=true;else window.fixtureApiHeaders.push(entry);
                const signal=args[1]?.signal!==undefined?args[1].signal:args[0]?.signal||null;
                const abortedAtDispatch=!!signal?.aborted;entry.signal_aborted_at_dispatch=abortedAtDispatch;
                const observeAbort=()=>{entry.signal_abort_observed_ms=performance.now()};
                if(abortedAtDispatch)entry.signal_abort_observed_ms=performance.now();else if(signal)signal.addEventListener('abort',observeAbort,{once:true});
                try{let response;try{response=await original(...args)}catch(error){entry.state='rejected';entry.error_stage='fetch';entry.error_name=String(error?.name||'Error').slice(0,80);entry.error_message=String(error?.message||error).slice(0,256);entry.signal_aborted_at_rejection=!!signal?.aborted;throw error}
                  try{Object.assign(entry,{status:response.status,retry_after:response.headers.get('retry-after'),cache_control:response.headers.get('cache-control'),csp:response.headers.get('content-security-policy'),coop:response.headers.get('cross-origin-opener-policy'),corp:response.headers.get('cross-origin-resource-policy')});entry.state='response';return response}
                  catch(error){entry.state='rejected';entry.error_stage='metadata';entry.error_name=String(error?.name||'Error').slice(0,80);entry.error_message=String(error?.message||error).slice(0,256);entry.signal_aborted_at_rejection=!!signal?.aborted;throw error}}
                finally{if(signal)signal.removeEventListener('abort',observeAbort);entry.finished_ms=performance.now()}};
                return paced?(await window.fixturePace(dispatch)).response:dispatch(performance.now())}''',cadence.interval*1000)
            transport_controls=async_js('''const done=arguments[arguments.length-1];(async()=>{
              const endpoint=new URL('/v1/console/query',location.href).href;
              const matches=(row,preaborted)=>!!row&&row.path==='/v1/console/query'&&row.method==='POST'&&row.state==='rejected'&&row.error_stage==='fetch'&&row.error_name==='AbortError'&&row.status===null&&row.signal_aborted_at_rejection===true&&row.signal_aborted_at_dispatch===preaborted&&Number.isFinite(row.started_ms)&&Number.isFinite(row.signal_abort_observed_ms)&&Number.isFinite(row.finished_ms)&&row.started_ms<=row.signal_abort_observed_ms&&row.signal_abort_observed_ms<=row.finished_ms;
              const beforeRequest=window.fixtureApiHeaders.length;const beforeController=new AbortController();beforeController.abort();let beforeError=null;
              try{await window.fetch(new Request(endpoint,{method:'POST',headers:{'content-type':'application/json','x-fabric-client-version':'1'},body:'{}',signal:beforeController.signal}))}catch(error){beforeError=error}
              const beforeRow=window.fixtureApiHeaders[beforeRequest];const beforeOk=window.fixtureApiHeaders.length===beforeRequest+1&&beforeError?.name==='AbortError'&&matches(beforeRow,true);
              const inFlightIndex=window.fixtureApiHeaders.length;const inFlightController=new AbortController();const originalPace=window.fixturePace;let inFlightError=null;
              window.fixturePace=dispatch=>originalPace(started=>{const pending=dispatch(started);inFlightController.abort();return pending});
              try{await window.fetch(endpoint,{method:'POST',headers:{'content-type':'application/json','x-fabric-client-version':'1'},body:'{}',signal:inFlightController.signal})}catch(error){inFlightError=error}finally{window.fixturePace=originalPace}
              const inFlightRow=window.fixtureApiHeaders[inFlightIndex];const inFlightOk=window.fixtureApiHeaders.length===inFlightIndex+1&&inFlightError?.name==='AbortError'&&matches(inFlightRow,false);
              return {preaborted:{accepted:beforeOk,row:beforeRow},inflight:{accepted:inFlightOk,row:inFlightRow}};
            })().then(done).catch(error=>done({error:String(error)}));''')
            receipt['transport_cancellation_controls']=transport_controls
            if not isinstance(transport_controls,dict) or not transport_controls.get('preaborted',{}).get('accepted') or not transport_controls.get('inflight',{}).get('accepted'):
                raise AssertionError('real browser did not produce both exact AbortSignal cancellation witnesses')
            if args.security_probes:
                js('''const original=window.fetch;window.fixtureCompletedRegistration=null;
                  window.fetch=(...args)=>{const p=new URL(typeof args[0]==='string'?args[0]:args[0].url,location.href).pathname;
                  if(p==='/v1/console/auth/register/finish' && !window.fixtureCompletedRegistration)
                    window.fixtureCompletedRegistration=JSON.parse(args[1].body);
                  return original(...args)}''')
            click('Connect to server')
            wait(lambda: js('return document.body.textContent.includes("Sign in with a passkey") && !document.body.textContent.includes("Working…")'), 'live sign-in locked view')
            js('''const weakenResident=arguments[0];const original=window.fabricPasskey;window.fabricPasskey=async function(register,text){
              const parsed=JSON.parse(text);const options=parsed.publicKey;window.fixtureCeremonyDiagnostic={register,attestation:options.attestation,authenticatorSelection:{...options.authenticatorSelection}};
              if(register && weakenResident){options.authenticatorSelection.residentKey='discouraged';options.authenticatorSelection.requireResidentKey=false;text=JSON.stringify(parsed);}
              try{const result=await original(register,text);window.fixtureCeremonyDiagnostic.complete=true;return result;}
              catch(error){window.fixtureCeremonyDiagnostic.errorName=error.name;window.fixtureCeremonyDiagnostic.errorMessage=error.message;throw error;}}''', args.inject_nonresident_enrollment)
            js('Array.from(document.querySelectorAll("details")).find(e=>e.querySelector("button")?.textContent.trim()==="Create owner passkey").open=true')
            expect(api('/v1/console/auth/register/start','POST',{'bootstrap_secret':'','display_name':'First visitor'}), 403, 'access denied', 'first visitor cannot claim owner without protected bootstrap')
            field('Display name', 'Browser fixture owner')
            field('One-time setup secret', bootstrap)
            bootstrap_replay=bootstrap
            bootstrap = None
            click('Create owner passkey')
            wait(lambda: js('return document.body.textContent.includes("Browser fixture owner · owner")'), 'actual WebAuthn owner registration',seconds=65 if args.browser=='firefox' else 25)
            check(not bootstrap_file.exists(), 'first owner consumed protected one-time bootstrap')
            selection=js('return window.fixtureCeremonyDiagnostic.authenticatorSelection')
            receipt['registration_selection']=selection
            check(selection.get('residentKey')=='required' and selection.get('requireResidentKey') is True
                  and selection.get('userVerification')=='required',
                  'server enrollment requires resident credentials and user verification')
            resident_credentials(authenticator, 'owner enrollment')
            expect(api('/v1/console/auth/register/start','POST',{'bootstrap_secret':bootstrap_replay,'display_name':'Replayed owner'}), 403, 'access denied', 'consumed one-time bootstrap cannot start another owner')
            bootstrap_replay=None
            cookies = wd('/cookie', method='GET')
            cookie = next(item for item in cookies if item['name'].startswith('__Host-'))
            check(cookie.get('secure') and cookie.get('httpOnly') and cookie.get('sameSite') == 'Strict' and cookie.get('path') == '/', 'session cookie Secure HttpOnly SameSite Strict host path')
            code, view = api('/v1/console/session')
            check(code == 200 and view['api_version'] == 1, 'real server session compatibility')
            principal_id = view['principal_id']
            return server,cookie,view,principal_id,bootstrap_file,authenticator,dict(
                started_monotonic_ns=started,state_before=state_record,browser_pid=info['capabilities'].get('moz:processID'))

        server,cookie,view,principal_id,bootstrap_file,authenticator,first_fixture=start_owner('AB' if expiry else '')
        if args.migration:
            browser_migration.finish(args,work,out,config,admin,migration_prepared,api,bearer_api,launch,stop_migration,wait)
            capture_api_metadata();verify_binary_identity();receipt['exit']=0
            return
        if expiry:
            from browser_expiry import run_expiry
            run_expiry(args,receipt,clock_targets,owned_groups,group_parent,state_descriptors,first_fixture,
                start_owner,wd,js,async_js,click,wait,check,api,bearer_api,pace_api,
                deadline,cadence,cookie,view,fixture_started)
            verify_binary_identity();receipt['exit']=0
            return
        expect(api('/v1/console/logout','POST',csrf=False), 403, 'access denied', 'cookie mutation without CSRF rejected')
        check(api('/v1/console/session')[0] == 200, 'rejected CSRF mutation preserves session')
        request=urllib.request.Request(origin+'/v1/console/logout',data=b'',method='POST',headers={
            'Cookie':cookie['name']+'='+cookie['value'],'x-fabric-client-version':'1',
            'x-fabric-csrf':view['csrf'],'Origin':'https://untrusted.invalid'})
        try:
            urllib.request.urlopen(request,context=context,timeout=10)
            raise AssertionError('cross-origin cookie mutation unexpectedly accepted')
        except urllib.error.HTTPError as error:
            expect((error.code,json.load(error)),403,'access denied','cookie mutation with foreign Origin rejected')
        check(api('/v1/console/session')[0] == 200, 'rejected Origin mutation preserves session')
        if args.announcement_probes:
            for name in ('browser_announcements.py','announcement_probe.py','announcement_oracle.py','announcement_contract.py'):
                path=ROOT/'tools/ui'/name
                receipt['source_sha256'][str(path.relative_to(ROOT))]=hashlib.sha256(path.read_bytes()).hexdigest()
            browser_announcements.run(announcements,js=js,wd=wd,click=click,field=field,nav=nav,wait=wait,
                principal=principal_id,origin=origin,browser_pid=first_fixture['browser_pid'],receipt=receipt)
            check(dict(line.split() for line in (limits/'pids.events').read_text().splitlines())['max']=='0',
                'actual announcement aggregate has no refused tasks')
            capture_api_metadata();verify_binary_identity();receipt['exit']=0
            return
        if args.security_probes or args.security_key_probe:
            import browser_security
            def export_fixture_credentials():
                if args.browser == 'firefox':
                    wd('/moz/context', {'context':'chrome'})
                    try:
                        return js('return ChromeUtils.importESModule("chrome://remote/content/marionette/webauthn.sys.mjs").webauthn.getCredentials(arguments[0])', authenticator)
                    finally:
                        wd('/moz/context', {'context':'content'})
                return cdp('WebAuthn.getCredentials', {'authenticatorId':authenticator})['credentials']
            if args.security_key_probe:
                path=ROOT/'tools/ui/browser_security.py'
                receipt['source_sha256'][str(path.relative_to(ROOT))]=hashlib.sha256(path.read_bytes()).hexdigest()
                receipt['security_key_probe']=browser_security.probe_key(api=api,bearer=bearer_api,async_js=async_js,
                    export_credentials=export_fixture_credentials,origin=origin,principal=principal_id,
                    work=work,deadline=deadline)
                capture_api_metadata();verify_binary_identity();receipt['exit']=0
                return
            registration=js('const r=window.fixtureCompletedRegistration;window.fixtureCompletedRegistration=null;return r')
            receipt['security_probes']=[]
            browser_security.run(api=api,bearer=bearer_api,js=js,async_js=async_js,wd=wd,
                export_credentials=export_fixture_credentials,origin=origin,principal=principal_id,
                work=work,deadline=deadline,registration=registration,inject=args.inject_security_defect,
                observations=receipt['security_probes'])
            check(browser_security.grade(receipt['security_probes']),'all signed browser security properties')
            registration=None
            capture_api_metadata();verify_binary_identity();receipt['exit']=0
            return
        if args.pressure:
            held=[]
            try:
                for _ in range(2):
                    stream=context.wrap_socket(socket.create_connection(('127.0.0.1',server_port),timeout=5),server_hostname='localhost')
                    stream.settimeout(20)
                    head=(f'POST /v1/console/query HTTP/1.1\r\nHost: localhost:{server_port}\r\n'
                          f'Cookie: {cookie["name"]}={cookie["value"]}\r\nx-fabric-client-version: 1\r\n'
                          f'Origin: {origin}\r\nx-fabric-csrf: {view["csrf"]}\r\n'
                          'Content-Type: application/json\r\nContent-Length: 65536\r\nConnection: close\r\n\r\n{')
                    stream.sendall(head.encode());held.append(stream)
                wait(lambda:api('/v1/console/query','POST',{})[0]==503,'finite protected query pool saturation',seconds=4)
                expect(bearer_api('0'*64,'/v1/console/session'), 401, 'sign in required', 'invalid credential rejected before occupied protected query pool')
                started=time.monotonic()
                response=held[0].recv(8192)
                elapsed=time.monotonic()-started
                receipt['protected_body_timeout']={'elapsed_s':elapsed,'response_status_line':response.split(b'\r\n',1)[0].decode(errors='replace')}
                check(b'408' in response.split(b'\r\n',1)[0],'actual partial authenticated body reaches finite server deadline')
                check(elapsed<=18,'native authenticated body wait stays within bounded deadline allowance')
            finally:
                for stream in held:stream.close()
            wait(lambda:api('/v1/console/session')[0]==200,'protected admission recovers after timeout/cancellation',seconds=4)
        code, _ = api('/v1/console/session', version=False)
        expect((code,_),426,'console update required','missing client version rejected')
        unsupported=async_js("const done=arguments[arguments.length-1];fetch('/v1/console/query',{method:'POST',headers:{'x-fabric-client-version':'999','content-type':'application/json'},body:'{}'}).then(async r=>done({status:r.status,text:await r.text()}));")
        expect((unsupported['status'],json.loads(unsupported['text'])),426,'console update required','unsupported client cannot submit incompatible query')
        nav('Settings')
        click('Refresh inventory')
        wait(lambda: js('return document.body.textContent.includes("fabric-server-self")'), 'actual companion inventory')
        check(js('return document.body.textContent.includes("enrollment_id")'), 'inventory exposes immutable enrollment IDs')
        click('Refresh status')
        wait(lambda: js('return document.body.textContent.includes("retention_bytes")'), 'effective measured storage policy')
        overload=async_js('''const done=arguments[arguments.length-1];window.fixtureUnpacedBurst=true;Promise.all(Array.from({length:16},()=>fetch('/v1/console/session',{headers:{'x-fabric-client-version':'1'},cache:'no-store',credentials:'same-origin'}).then(async r=>({status:r.status,retry_after:r.headers.get('retry-after'),cache_control:r.headers.get('cache-control'),body:await r.text()})))).finally(()=>{window.fixtureUnpacedBurst=false}).then(done);''')
        # Retain only public rejection metadata, never the successful sessions.
        receipt['peer_overload']=[{k:r[k] for k in ('status','retry_after','cache_control')} for r in overload]
        refused=[r for r in overload if r['status']==429]
        check(bool(refused) and all(r['retry_after']=='1' and r['cache_control']=='no-store'
              and json.loads(r['body'])=={'error':'request admission is full'} for r in refused),
              'unpaced protected peer burst is explicitly limited without cacheable responses')
        wait(lambda:api('/v1/console/session')[0]==200,'protected peer admission recovers after refill')
        click('List passkeys')
        wait(lambda: js('return document.querySelectorAll(".live-json").length > 0 && !document.body.textContent.includes("Working…")'), 'passkey listing')
        # excludeCredentials intentionally rejects enrolling the same authenticator
        # twice. Model a second physical authenticator for the additional passkey.
        remove_authenticator(authenticator)
        authenticator = add_authenticator()
        click('Add another passkey')
        wait(lambda: api('/v1/console/passkeys')[0] == 200 and len(api('/v1/console/passkeys')[1]) == 2, 'additional WebAuthn passkey')
        code, passkeys = api('/v1/console/passkeys')
        check(code == 200 and len(passkeys) == 2, 'two real virtual-authenticator passkeys enrolled')
        resident_credentials(authenticator, 'additional key enrollment')
        # Enroll and run an actual second Spindle for an untrusted-text fixture.
        fixture_log=work/'untrusted.log'
        fixture_log.write_text('<img src=x onerror="window.fabricXssMarker=1"><script>window.fabricXssMarker=1</script> browser-xss-fixture\n'+
            ''.join(f'browser-page-fixture ordinal={i:03d} trace_id={TRACE_ID}\n' for i in range(250)))
        field('Absolute log paths, one per line',str(fixture_log))
        field('Metric interval (1–3600 s)','1')
        js('Array.from(document.querySelectorAll("details")).find(e=>e.textContent.includes("Enroll a new Spindle")).open=true')
        field('New Spindle name','browser-edge')
        click('Enroll with configuration above')
        wait(lambda:js('return document.body.textContent.includes("One-time credential")'),'Spindle enrollment')
        node_token=js('return Array.from(document.querySelectorAll("code")).map(e=>e.textContent.trim()).find(value=>/^[a-f0-9]{64}$/.test(value))')
        token_file=work/'edge-token';token_file.write_text(node_token+'\n');token_file.chmod(0o600);node_token=None
        click('Clear displayed credential')
        node_config=work/'edge.conf'
        trace_port=free_port()
        node_config.write_text(f'spool_dir={work}/edge-spool\nlog={fixture_log}\nmetric_interval_s=1\nspool_bytes=33554432\nserver_url={origin}\nserver_ca={work}/ca.pem\ntoken_file={token_file}\nmax_output_bytes_per_s=65536\ntraces_listen=127.0.0.1:{trace_port}\n')
        edge=launch('edge',[str(args.spindle.resolve()),'run',str(node_config)])
        wait(lambda:(out/'edge.log').is_file() and 'status=ack' in (out/'edge.log').read_text(),'native edge exact delivery ACK')
        trace_origin=time.time_ns()-1_000_000_000
        trace_body,trace_rows=traces(trace_origin)
        request=urllib.request.Request(f'http://127.0.0.1:{trace_port}/v1/traces',data=trace_body,headers={'Content-Type':'application/x-protobuf'})
        with urllib.request.urlopen(request,timeout=15) as response:
            check(response.status==200,'independent OTLP trace producer received committed-Spool success')
        receipt['producer_expected_spans']=trace_rows
        receipt['producer_schema']='https://github.com/open-telemetry/opentelemetry-proto/blob/v1.9.0/opentelemetry/proto/trace/v1/trace.proto'
        canary_b='browser-canary-B-private'
        b_log=work/'canary-b-private.log';b_log.write_text(canary_b+'\n')
        field('Absolute log paths, one per line',str(b_log))
        field('Metric interval (1–3600 s)','7')
        field('New Spindle name','browser-canary-b')
        click('Enroll with configuration above')
        wait(lambda:js('return document.body.textContent.includes("One-time credential")'),'second canary immutable enrollment')
        b_token=js('return Array.from(document.querySelectorAll("code")).map(e=>e.textContent.trim()).find(value=>/^[a-f0-9]{64}$/.test(value))')
        b_token_file=work/'canary-b-token';b_token_file.write_text(b_token+'\n');b_token_file.chmod(0o600);b_token=None
        click('Clear displayed credential')
        b_config=work/'canary-b.conf'
        b_config.write_text(f'spool_dir={work}/canary-b-spool\nlog={b_log}\nmetric_interval_s=7\nspool_bytes=4194304\nserver_url={origin}\nserver_ca={work}/ca.pem\ntoken_file={b_token_file}\nmax_output_bytes_per_s=65536\n')
        launch('canary-b',[str(args.spindle.resolve()),'run',str(b_config)])
        wait(lambda:(out/'canary-b.log').is_file() and 'status=ack' in (out/'canary-b.log').read_text(),'independent second canary source delivers through actual ACK')
        canary_query={'kind':'logs','node':'browser-canary-b','from_ns':trace_origin-60_000_000_000,'to_ns':time.time_ns()+60_000_000_000,'contains':canary_b,'limit':200,'page':None}
        wait(lambda:api('/v1/console/query','POST',canary_query)[0]==200 and len(api('/v1/console/query','POST',canary_query)[1]['rows'])==1,'owner positive witness for second canary history')
        b_code,b_answer=api('/v1/console/query','POST',canary_query)
        check(b_code==200 and b_answer['rows'][0]['body']==canary_b,'second source matches distinct producer canary constant')
        receipt['second_canary_observed_ns']=b_answer['rows'][0]['observed_ns']
        receipt['second_canary_freshness_ns']=b_answer['freshness']['browser-canary-b']
        field('Existing Spindle name','browser-edge')
        click('pause')
        wait(lambda:not js('return document.body.textContent.includes("Working…")'),'Spindle pause')
        code,current=api('/v1/console/nodes')
        check(next(n for n in current['nodes'] if n['name']=='browser-edge')['status']=='paused','actual Spindle pause applied to control')
        click('resume')
        wait(lambda:not js('return document.body.textContent.includes("Working…")'),'Spindle resume')
        code,current=api('/v1/console/nodes')
        check(next(n for n in current['nodes'] if n['name']=='browser-edge')['status']=='active','actual Spindle resume applied to control')
        nav('Explore')
        signal_view=js('const select=Array.from(document.querySelectorAll("label")).find(e=>e.textContent.trim().startsWith("Signal")).querySelector("select");return {selected:select.value,label:Array.from(document.querySelectorAll("label")).map(e=>e.textContent.trim()).find(text=>text.startsWith("Body substring")||text.startsWith("Exact metric name")||text.startsWith("Exact trace ID"))}')
        receipt['initial_signal_view']=signal_view
        check(signal_view['label'].startswith({'logs':'Body substring','metrics':'Exact metric name','spans':'Exact trace ID'}.get(signal_view['selected'],'invalid selection')),'initial granted signal selection agrees with actual query filter label')
        field('Signal','logs')
        field('Source name (blank = all sources)','browser-edge')
        field('Body substring (case-sensitive)','browser-xss-fixture')
        click('Run query')
        wait(lambda:js('return document.querySelector(".results")?.textContent.includes("browser-xss-fixture")'),'actual untrusted source query')
        check(js('return document.querySelectorAll(".results img,.results script").length===0 && !window.fabricXssMarker'),'stored untrusted telemetry rendered as inert text')
        field('Body substring (case-sensitive)','browser-page-fixture')
        click('Last 15 minutes')
        click('Run query')
        wait(lambda:js('return document.querySelectorAll(".results tbody tr").length===200'),'bounded first actual log page')
        first_page=js('return Array.from(document.querySelectorAll(".results tbody tr pre")).map(e=>e.textContent)')
        check(all(f'ordinal={i:03d}' in row for i,row in enumerate(first_page)), 'first page matches independent producer ordinals')
        if args.ui_responses:
            import browser_responses
            query={'kind':'logs','node':'browser-edge','from_ns':trace_origin-60_000_000_000,'to_ns':time.time_ns()+60_000_000_000,'contains':'browser-page-fixture','limit':200,'page':None}
            status,baseline=api('/v1/console/query','POST',query)
            check(status==200 and len(baseline['rows'])==200,'actual package response before diagnostic UI corpus')
            receipt['ui_response_observations']=[]
            receipt['ui_response_witness']=browser_responses.run(js=js,click=click,wait=wait,field=field,nav=nav,
                baseline=baseline,observations=receipt['ui_response_observations'],inject=args.inject_ui_response_defect)
            check(browser_responses.grade(receipt['ui_response_observations']),'all diagnostic UI response properties')
            capture_api_metadata();verify_binary_identity();receipt['exit']=0
            return
        click('Next snapshot page')
        wait(lambda:js('return document.querySelectorAll(".results tbody tr").length===50'),'bounded continuation actual log page')
        second_page=js('return Array.from(document.querySelectorAll(".results tbody tr pre")).map(e=>e.textContent)')
        check(all(f'ordinal={i+200:03d}' in row for i,row in enumerate(second_page)), 'continuation has exact remaining producer ordinals')
        field('Signal','metrics')
        field('Exact metric name','system.cpu.time')
        click('Run query')
        wait(lambda:js('return document.querySelectorAll(".live-metric-plot").length>0'),'actual native metric series chart')
        check(js('return document.querySelectorAll(".live-metric-plot").length<=8'),'metric charts remain bounded to eight distinct series')
        nav('Trace detail')
        field('Signal','spans')
        field('Exact trace ID (optional)',TRACE_ID)
        click('Run query')
        wait(lambda:js('return document.querySelectorAll(".live-span").length===3'),'actual recorded trace waterfall')
        waterfall=js('return Array.from(document.querySelectorAll(".live-span")).map(e=>e.textContent)')
        check(all(any(r['name'] in text and f"[{r['start_ns']}, {r['end_ns']}]" in text for text in waterfall) for r in trace_rows), 'waterfall preserves independently produced adjacent integer timestamps')
        check(any('browser-orphan' in text and 'parent unavailable in this page' in text for text in waterfall),'missing parent remains explicit in actual trace page')
        geometry=js('return Array.from(document.querySelectorAll(".live-span")).map(e=>{const track=e.querySelector(".live-span-track").getBoundingClientRect(),bar=e.querySelector(".live-span-bar").getBoundingClientRect();return {name:e.querySelector("b").textContent,left_percent:100*(bar.left-track.left)/track.width,width_percent:100*bar.width/track.width}})')
        receipt['trace_geometry']=geometry
        first_start=min(row['start_ns'] for row in trace_rows);span_range=max(row['end_ns'] for row in trace_rows)-first_start
        check(len(geometry)==3 and all(any(item['name']==row['name'] and abs(item['left_percent']-100*(row['start_ns']-first_start)/span_range)<.03 and abs(item['width_percent']-100*(row['end_ns']-row['start_ns'])/span_range)<.03 for item in geometry) for row in trace_rows),'actual waterfall geometry matches independent integer offsets and durations')
        field('Exact span name (optional)','browser-child');click('Run query')
        wait(lambda:js('return document.querySelectorAll(".live-span").length===1 && document.querySelectorAll(".results tbody tr").length===1'),'backend-supported exact span-name UI returns only independent child span')
        named_spans=[json.loads(value) for value in js('return Array.from(document.querySelectorAll(".results tbody .live-json")).map(e=>e.textContent)')]
        check(len(named_spans)==1 and named_spans[0]['name']=='browser-child' and named_spans[0]['start_ns']==trace_rows[1]['start_ns'] and named_spans[0]['end_ns']==trace_rows[1]['end_ns'],'exact span-name filter preserves independent integer producer constants')
        related_button=wd('/element',{'using':'css selector','value':'[data-testid=live-related-logs]'})['element-6066-11e4-a52e-4f735466cecf']
        wd('/element/'+related_button+'/click',{})
        wait(lambda:js('return document.querySelectorAll(".results tbody tr").length===200 && document.querySelector(".breadcrumb")?.textContent.includes("Explore")'),'actual related-log action opens producer-correlated source rows')
        related_rows=[json.loads(value) for value in js('return Array.from(document.querySelectorAll(".results tbody .live-json")).map(e=>e.textContent)')]
        check(len(related_rows)==200 and all(TRACE_ID in row['body'] and row['node']=='browser-edge' for row in related_rows),'related-log navigation matches independent trace/log correlation constant')
        nav('Overview')
        click('Refresh sources')
        wait(lambda:js('return Array.from(document.querySelectorAll(".source-row b")).some(e=>e.textContent==="fabric-server-self")'),'overview shows readable actual companion inventory')
        click('Load process observations')
        wait(lambda:js('return Array.from(document.querySelectorAll(".stat h3")).some(e=>e.textContent.includes("PID"))'),'overview shows measured process samples from real companion')
        check(js('return document.body.textContent.includes("RSS") && document.body.textContent.includes("ticks/s") && document.body.textContent.includes("collector observed")'),'overview labels measured memory CPU and observation clocks')
        nav('Pipeline')
        click('Refresh status')
        wait(lambda:js('return document.body.textContent.includes("retention_bytes")'),'pipeline effective measured policy')
        check(js('return document.querySelectorAll(".pipeline-stages li").length===5 && document.body.textContent.includes("Server ACK follows durable commit")'),'pipeline states actual custody boundary through readable stages')
        nav('Live Tail')
        field('Signal','logs')
        field('Source name (blank = all sources)','browser-edge')
        field('Body substring (case-sensitive)','browser-xss-fixture')
        click('Resume 5-second polling')
        wait(lambda:js('return document.querySelectorAll(".tail-line").length>0'),'bounded actual tail snapshot',seconds=12)
        click('Pause 5-second polling')
        paused_tail=js('return document.querySelector(".tail-terminal").textContent')
        check(js('return document.body.textContent.includes("Resume 5-second polling")'),'actual tail pause control')
        click('Resume 5-second polling')
        check(js('return document.body.textContent.includes("Pause 5-second polling")'),'actual tail resume control')
        if args.browser=='chrome':
            js('window.fixtureTailFetch=window.fetch;window.fixtureTailQueryCount=0;window.fetch=(...args)=>{const url=typeof args[0]==="string"?args[0]:args[0].url;if(new URL(url,location.href).pathname==="/v1/console/query")window.fixtureTailQueryCount++;return window.fixtureTailFetch(...args)}')
            wait(lambda:js('return window.fixtureTailQueryCount>0 && !document.body.textContent.includes("Working…")'),'visible tail has positive polling witness',seconds=7)
            tail_handle=wd('/window',method='GET')
            previous_handles=set(wd('/window/handles',method='GET'))
            check(js('window.fixtureHiddenTab=window.open("about:blank","fixture-hidden-tail");return !!window.fixtureHiddenTab'),'owned background-polling tab created')
            hidden_handle=next(handle for handle in wd('/window/handles',method='GET') if handle not in previous_handles)
            wd('/window',{'handle':hidden_handle})
            wait(lambda:js('return window.opener.document.visibilityState==="hidden"'),'actual tail document becomes hidden')
            before_hidden=js('return window.opener.fixtureTailQueryCount')
            hidden_start=time.monotonic()
            while time.monotonic()-hidden_start<12:
                check(js('return window.opener.fixtureTailQueryCount')==before_hidden,'hidden tail sends no new protected query')
                time.sleep(1)
            receipt['hidden_tail_polling']={'duration_s':time.monotonic()-hidden_start,'queries_before':before_hidden,'queries_after':js('return window.opener.fixtureTailQueryCount')}
            wd('/window',method='DELETE');wd('/window',{'handle':tail_handle})
            wait(lambda:js('return window.fixtureTailQueryCount>arguments[0]',before_hidden),'tail polling resumes only after visible document returns',seconds=7)
            js('window.fetch=window.fixtureTailFetch;delete window.fixtureTailFetch')
        if args.poll_seconds:
            field('Body substring (case-sensitive)','browser-poll-fixture')
            polling=[];start_poll=time.monotonic();next_write=start_poll;ordinal=0
            while time.monotonic()-start_poll<args.poll_seconds:
                if time.monotonic()>deadline or storage_violation.is_set():raise RuntimeError('bounded polling fixture deadline/storage limit')
                if time.monotonic()>=next_write:
                    with fixture_log.open('a') as source:source.write(f'browser-poll-fixture ordinal={ordinal:03d}\n')
                    ordinal+=1;next_write+=5
                observation=js('return {rows:document.querySelectorAll(".tail-line").length,bytes:new TextEncoder().encode(document.querySelector(".tail-terminal").textContent).length,status:Array.from(document.querySelectorAll("[role=status]")).map(e=>e.textContent)}')
                observation['elapsed_s']=time.monotonic()-start_poll
                if observation['rows']>200 or observation['bytes']>262144:raise AssertionError('sustained tail snapshot exceeded display budget')
                polling.append(observation)
                time.sleep(1)
            receipt['bounded_polling']=polling
            check(any(item['rows']>0 for item in polling),'two-minute real tail workload observed independently appended producer rows')
            check(all(item['rows']<=200 and item['bytes']<=262144 for item in polling),'two-minute tail retains explicit row/UTF-8 display bounds')
            click('Pause 5-second polling')
        nav('Explore')
        field('Source name (blank = all sources)', 'fabric-server-self')
        field('Body substring (case-sensitive)', '')
        click('Run query')
        wait(lambda: js('return document.body.textContent.includes("Evidence and exact applied request") && !document.body.textContent.includes("Working…")'), 'actual retained-history response')
        check(js('return document.body.textContent.includes("retained_from_ns") && document.body.textContent.includes("snapshot")'), 'query evidence remains visible')
        wait(lambda:js('return document.querySelectorAll(".results td.mono").length>0'), 'real companion observations retained')
        exact_titles=js('return Array.from(document.querySelectorAll(".results td.mono")).map(e=>e.title)')
        exact_rows=[json.loads(value) for value in js('return Array.from(document.querySelectorAll(".results tbody .live-json")).map(e=>e.textContent)')]
        check(bool(exact_titles) and len(exact_titles)==len(exact_rows) and exact_titles==[str(row['observed_ns']) for row in exact_rows],'human-readable timestamp titles match exact typed response integers')
        check(js('return !!document.querySelector("details.exact-range") && !document.querySelector("details.exact-range").open && document.querySelectorAll(".results details").length>0 && Array.from(document.querySelectorAll(".results details")).every(e=>!e.open)'),'exact query bounds evidence and row JSON start collapsed')
        (out/'desktop.png').write_bytes(base64.b64decode(wd('/screenshot', method='GET')))
        contrast_script='''const foreground=getComputedStyle(document.querySelector(".results h2")).color;
          const background=getComputedStyle(document.querySelector(".results")).backgroundColor;
          const luminance=value=>{const c=value.match(/[0-9.]+/g).slice(0,3).map(Number).map(x=>{x/=255;return x<=.04045?x/12.92:((x+.055)/1.055)**2.4});return .2126*c[0]+.7152*c[1]+.0722*c[2]};
          const a=luminance(foreground),b=luminance(background);return {foreground,background,ratio:(Math.max(a,b)+.05)/(Math.min(a,b)+.05)};'''
        theme_button=js('return document.querySelector(".theme-button").textContent.trim()')
        first_contrast=js(contrast_script)
        click(theme_button)
        wait(lambda:js('return document.querySelector(".theme-button").textContent.trim()')!=theme_button,'color theme toggled')
        async_js('const done=arguments[arguments.length-1];Promise.all(document.getAnimations().filter(a=>a.effect.getTiming().iterations!==Infinity).map(a=>a.finished.catch(()=>{}))).then(()=>done(true))')
        second_contrast=js(contrast_script)
        receipt['selected_text_contrast']=[first_contrast,second_contrast]
        check(all(item['ratio']>=4.5 for item in receipt['selected_text_contrast']),'selected measured heading text contrast at least 4.5 in both themes')
        click(js('return document.querySelector(".theme-button").textContent.trim()'))
        body_element=wd('/element',{'using':'css selector','value':'body'})['element-6066-11e4-a52e-4f735466cecf']
        js('document.body.setAttribute("tabindex","-1");document.body.focus()')
        wd('/element/'+body_element+'/value',{'text':'\ue004','value':['\ue004']})
        js('document.body.removeAttribute("tabindex")')
        check(js('return document.activeElement.classList.contains("skip-link")'),'keyboard Tab reaches visible skip link')
        check(js('return document.activeElement.getBoundingClientRect().top>=0'),'focused skip link visible')
        field('Body substring (case-sensitive)','x'*4097)
        check(js('return !!document.querySelector("[role=status][aria-live=polite]") && document.querySelector("[role=alert]")?.textContent.includes("Filter exceeds 4096 UTF-8 bytes")'),'actual invalid filter error and request status have announcement roles')
        field('Body substring (case-sensitive)','');click('Run query')
        wait(lambda:js('return document.querySelectorAll(".results tbody tr").length>0'),'valid query restores readable table after announced validation error')
        if args.browser == 'firefox':
            viewport_context_handle = wd('/window', method='GET')
            viewport_baseline = js('return {width:innerWidth,height:innerHeight,dpr:devicePixelRatio}')
            viewport_restore_required = True
            wd('/window/rect', {'width':390,'height':844})
            run_viewport('narrow', viewport_context_handle)
            viewport_actual = js('return {width:innerWidth,height:innerHeight,dpr:devicePixelRatio}')
            verify_viewport_geometry(viewport_actual, viewport_baseline)
        else:
            wd('/window/rect', {'width':390,'height':844})
        check(js('return document.documentElement.scrollWidth <= window.innerWidth + 2'), 'narrow viewport has no document overflow')
        (out/'narrow.png').write_bytes(base64.b64decode(wd('/screenshot', method='GET')))
        js('document.documentElement.style.zoom="200%"')
        receipt['zoom_geometry']=js('return {viewport:innerWidth,documentWidth:document.documentElement.scrollWidth,overflow:Array.from(document.querySelectorAll("body *")).map(e=>({tag:e.tagName,cls:e.className?.baseVal??e.className,text:e.tagName==="BUTTON"?e.textContent:null,left:e.getBoundingClientRect().left,right:e.getBoundingClientRect().right})).filter(e=>e.right>innerWidth+2).slice(0,40)}')
        check(js('return document.documentElement.scrollWidth <= window.innerWidth + 2'),'200 percent content zoom retains document reflow')
        # Hosted CI exposed an unconstrained standalone pagination action.
        # Force a larger user font on that actual action, outside query-form.
        js("window.fixtureNextAction=Array.from(document.querySelectorAll('button')).find(e=>e.textContent.trim()==='Next snapshot page');if(!window.fixtureNextAction)throw Error('standalone pagination action missing');window.fixtureNextStyle=window.fixtureNextAction.style.cssText;window.fixtureNextAction.style.fontSize='18px';")
        receipt['pagination_reflow_candidate']=js("const e=window.fixtureNextAction;return {font:getComputedStyle(e).fontSize,viewport:innerWidth,documentWidth:document.documentElement.scrollWidth,buttonLeft:e.getBoundingClientRect().left,buttonRight:e.getBoundingClientRect().right,maxInlineSize:getComputedStyle(e).maxInlineSize,whiteSpace:getComputedStyle(e).whiteSpace}")
        check(js("return getComputedStyle(window.fixtureNextAction).fontSize==='18px' && !window.fixtureNextAction.closest('.query-form')"),'actual standalone pagination action uses independent 18px font witness')
        check(js('return document.documentElement.scrollWidth <= innerWidth + 2 && window.fixtureNextAction.getBoundingClientRect().right<=innerWidth+2'),'standalone pagination fits at 200 percent zoom and 18px font')
        try:
            js("window.fixtureNextAction.style.maxInlineSize='none';window.fixtureNextAction.style.whiteSpace='nowrap';window.fixtureNextAction.style.overflowWrap='normal';")
            receipt['pagination_reflow_negative']=js("const e=window.fixtureNextAction;return {viewport:innerWidth,documentWidth:document.documentElement.scrollWidth,buttonRight:e.getBoundingClientRect().right,maxInlineSize:getComputedStyle(e).maxInlineSize,whiteSpace:getComputedStyle(e).whiteSpace}")
            check(js('return document.documentElement.scrollWidth > innerWidth + 2 && window.fixtureNextAction.getBoundingClientRect().right>innerWidth+2'),'pagination reflow grader rejects removed width cap and forced nowrap')
        finally:
            js("window.fixtureNextAction.style.cssText=window.fixtureNextStyle;delete window.fixtureNextAction;delete window.fixtureNextStyle;")
        check(js('return document.documentElement.scrollWidth <= innerWidth + 2'),'pagination negative control restores coherent responsive layout')
        js('document.documentElement.style.zoom=""')
        if args.browser == 'firefox':
            run_viewport('restore', viewport_context_handle)
            wd('/window/rect', {'width':1440,'height':1100})
            desktop_geometry = js('return {width:innerWidth,height:innerHeight}')
            check(isinstance(desktop_geometry,dict) and desktop_geometry.get('width')==1440,
                  'desktop classic restore retains 1440 CSS pixels of inner width')
            viewport_restore_required = False
        else:
            wd('/window/rect', {'width':1440,'height':1100})
        code, _ = api('/v1/console/query', 'POST', {'kind':'rate','name':'test','from_ns':1,'to_ns':2})
        expect((code,_),400,'unbounded rate queries are unavailable in the console','rate endpoint is unavailable in the console')
        cache_keys = async_js('const done=arguments[arguments.length-1];caches.keys().then(async keys=>done((await Promise.all(keys.map(async key=>(await(await caches.open(key)).keys()).map(r=>new URL(r.url).pathname)))).flat()))')
        check(all(path.startswith('/console/') for path in cache_keys), 'worker caches only public shell paths')
        check(js('return localStorage.length===0 && sessionStorage.length===0'), 'no credentials or telemetry in Web Storage')
        js('window.fixtureSupersededFetch=window.fetch;window.fixtureSupersededReady=false;window.fixtureSupersededAbort=false;window.fixtureSupersededDelivered=false;window.fetch=async(...args)=>{const url=typeof args[0]==="string"?args[0]:args[0].url;const query=new URL(url,location.href).pathname==="/v1/console/query";if(query){const signal=args[0]?.signal||args[1]?.signal;if(signal)signal.addEventListener("abort",()=>window.fixtureSupersededAbort=true,{once:true});}const response=await window.fixtureSupersededFetch(...args);if(query){window.fixtureSupersededReady=true;await new Promise(resolve=>setTimeout(resolve,1200));window.fixtureSupersededDelivered=true;}return response}')
        click('Run query')
        wait(lambda:js('return window.fixtureSupersededReady'),'actual response is held before superseding input')
        field('Body substring (case-sensitive)','superseded-cancel-fixture')
        wait(lambda:js('return window.fixtureSupersededAbort'),'superseded query aborts actual transport signal')
        check(js('return Array.from(document.querySelectorAll("button")).find(e=>e.textContent.trim()==="Run query").disabled'),'superseded request retains pending slot until transport completion')
        wait(lambda:js('return window.fixtureSupersededDelivered && !document.body.textContent.includes("Working…")'),'superseded transport reaches terminal completion')
        check(js('return !document.querySelector(".results tbody") && !document.querySelector(".results > details") && document.querySelector(".results")?.textContent.includes("No observations have been fetched")'),'superseded reply cannot restore discarded query results')
        js('window.fetch=window.fixtureSupersededFetch;delete window.fixtureSupersededFetch')
        field('Body substring (case-sensitive)','');click('Run query')
        wait(lambda:js('return document.querySelectorAll(".results tbody tr").length>0'),'new explicit query succeeds after cancelled transport completes')
        js('''window.fixtureOriginalFetch=window.fetch;window.fixtureDelayedReady=false;window.fixtureDelayedDelivered=false;
          window.fetch=async(...args)=>{const response=await window.fixtureOriginalFetch(...args);
          const url=typeof args[0]==="string"?args[0]:args[0].url;
          if(new URL(url,location.href).pathname==="/v1/console/query"){
            window.fixtureDelayedReady=true;await new Promise(resolve=>setTimeout(resolve,1200));window.fixtureDelayedDelivered=true;}return response;}''')
        click('Run query')
        wait(lambda:js('return window.fixtureDelayedReady'),'real authenticated response is held before disconnect')
        js('window.dispatchEvent(new Event("offline"))')
        check(js('return !document.querySelector(".results") && document.body.textContent.includes("Sign in with a passkey") && !document.body.textContent.includes("Browser fixture owner · owner")'), 'offline clears protected in-memory views')
        wait(lambda:js('return window.fixtureDelayedDelivered && !document.body.textContent.includes("Working…")'),'delayed authenticated response completed after lock')
        check(js('return !document.querySelector(".results") && document.body.textContent.includes("Sign in to view your telemetry.")'),'delayed response cannot restore protected view after disconnect')
        js('window.fetch=window.fixtureOriginalFetch;delete window.fixtureOriginalFetch')
        click('Check session')
        wait(lambda: js('return document.body.textContent.includes("Browser fixture owner · owner")'), 'online session explicitly revalidated')
        nav('Settings')
        code,inventory=api('/v1/console/nodes')
        names={node['name'] for node in inventory['nodes']}
        check(code==200 and {'browser-edge','browser-canary-b','fabric-server-self'}<=names,'owner inventory lists every enrolled source')
        spindle_token=token_file.read_text().strip()
        field('Reader name','Browser fixture reader')
        field('Credential lifetime (seconds)','1800' if args.expired_cursor_probe else '600')
        click('Issue reader token')
        wait(lambda:js('return document.body.textContent.includes("credential_id") && document.querySelector("code")!==null'), 'real reader token issuance')
        credential=js('return Array.from(document.querySelectorAll("code")).map(e=>e.textContent.trim()).find(value=>/^[a-f0-9]{64}$/.test(value))')
        issuance=next(v for v in (json.loads(text) for text in js('return Array.from(document.querySelectorAll(".live-json")).map(e=>e.textContent)') if text.strip().startswith('{')) if isinstance(v,dict) and 'credential_id' in v)
        check(bool(credential),'issued reader secret displayed once')
        click('Clear displayed credential')
        check(js('return !Array.from(document.querySelectorAll("code")).some(e=>/^[a-f0-9]{64}$/.test(e.textContent.trim()))'),'one-time credential clear control removes displayed secret')
        # Role matrix over trusted native TLS without browser cookies (ADR-0029): a
        # reader reads everything and reaches nothing else; neither anonymous callers
        # nor a Spindle token are console credentials.
        code,reader_nodes=bearer_api(credential,'/v1/console/nodes')
        check(code==200 and {n['name'] for n in reader_nodes['nodes']}==names,'reader inventory lists every enrolled source')
        if args.role_matrix:
            previous_cookie=next(item for item in wd('/cookie',method='GET') if item['name'].startswith('__Host-'))
            previous_session=hashlib.sha256(previous_cookie['value'].encode()).hexdigest();previous_cookie=None
            click('Reverify with passkey')
            wait(lambda:not js('return document.body.textContent.includes("Working…")'),'real owner freshness before complete route matrix')
            owner_cookie=next(item for item in wd('/cookie',method='GET') if item['name'].startswith('__Host-'))
            current_session=hashlib.sha256(owner_cookie['value'].encode()).hexdigest()
            check(current_session!=previous_session,'real passkey reverification replaces owner cookie before matrix')
            code,owner_view=api('/v1/console/session');check(code==200,'fresh owner session for matrix')
            matrix=receipt['route_matrix']={'fixture_sha256':hashlib.sha256(ROLE_FIXTURE.read_bytes()).hexdigest(),'owner_session':current_session,'observations':[]}
            def matrix_request(role,method,path,body):
                return bearer_api({'anonymous':None,'spindle':spindle_token,'reader':credential,'owner':None}[role],path,method,body,
                                  cookie=owner_cookie['name']+'='+owner_cookie['value'] if role=='owner' else None,
                                  csrf=owner_view['csrf'] if role=='owner' and method!='GET' else None,role=role)
            run_role_matrix(matrix_request,check,matrix['observations'])
            matrix['dispatches']=[item for item in receipt['explicit_tls_requests'] if item['role'] is not None]
            fields=('role','method','path','status','refusal')
            check([tuple(r[k] for k in fields) for r in matrix['dispatches']]==[tuple(r[k] for k in fields) for r in matrix['observations']], 'actual TLS dispatch ledger agrees with all matrix cells')
            check(all(b['started_ns']-a['started_ns']>=550_000_000 for a,b in zip(matrix['dispatches'],matrix['dispatches'][1:])),'every matrix dispatch retains shared 550ms cadence')
            matrix['auth_dispatch_count']=sum('/auth/' in item['path'] for item in matrix['dispatches'])
            matrix['browser_setup_auth_dispatches']=js('return window.fixtureApiHeaders.filter(item=>item.path.includes("/auth/"))')
            check(matrix['auth_dispatch_count']==16 and len(matrix['browser_setup_auth_dispatches'])==7,'literal matrix16 and real browser setup7 auth dispatches recorded')
            check(all(b['started_ms']-a['started_ms']>=550 for a,b in zip(matrix['browser_setup_auth_dispatches'],matrix['browser_setup_auth_dispatches'][1:])),'automatic WebAuthn dispatches retain shared cadence')
            owner_cookie=None;owner_view=None
            click('Check session');wait(lambda:js('return document.body.textContent.includes("Sign in to view your telemetry.")'),'matrix owner logout locks actual browser')
            capture_api_metadata();verify_binary_identity();receipt['exit']=0
            return
        owner_routes=(('GET','/v1/console/session',None),('GET','/v1/console/audit',None),('GET','/v1/console/passkeys',None),
                      ('GET','/v1/console/sessions',None),('GET','/v1/console/readers',None),('POST','/v1/console/readers',{'name':'escalation','ttl_s':60}),
                      ('POST','/v1/console/nodes',{'name':'escalation'}),('POST','/v1/console/nodes/browser-edge/pause',None),
                      ('PUT','/v1/console/nodes/browser-edge/config',{'logs':[],'metric_interval_s':15}),('POST','/v1/console/passkeys/start',None),
                      ('POST','/v1/console/logout',None))
        query_probe={'kind':'logs','from_ns':1,'to_ns':2,'limit':1}
        for method,path,body in owner_routes:
            expect(bearer_api(credential,path,method,body),403,'role not permitted',f'reader token refused on owner route {method} {path}')
        for token,who in ((None,'anonymous caller'),(spindle_token,'Spindle token')):
            for method,path,body in (owner_routes[0],owner_routes[5],('GET','/v1/console/nodes',None),('POST','/v1/console/query',query_probe)):
                expect(bearer_api(token,path,method,body),401,'sign in required',f'{who} refused on console route {method} {path}')
        check(bearer_api(credential,'/v1/config')[0]==401 and bearer_api(credential,'/v1/batches','POST',{})[0]==401 and api('/v1/config')[0]==401,'reader token and owner session are not Spindle credentials')
        read={'kind':'logs','node':'browser-edge','from_ns':trace_origin-60_000_000_000,'to_ns':time.time_ns()+1_000_000_000,'contains':'browser-page-fixture','limit':200,'page':None}
        code,reader_page=bearer_api(credential,'/v1/console/query','POST',read)
        check(code==200 and len(reader_page['rows'])==200 and all(row['node']=='browser-edge' for row in reader_page['rows']),'reader bearer query returns the independently produced source rows')
        check(all(f'ordinal={i:03d}' in row['body'] for i,row in enumerate(reader_page['rows'])),'reader query matches producer ordinal constants')
        expect(bearer_api(credential,'/v1/console/query','POST',dict(read,contains='x'*4097)),400,'query filter exceeds budget','oversize query filter receives explicit bounded-input rejection')
        positive_code,positive_after_pressure=bearer_api(credential,'/v1/console/query','POST',read)
        check(positive_code==200 and positive_after_pressure['rows']==reader_page['rows'],'query payload pressure preserves previously committed producer observations')
        code,reader_status=bearer_api(credential,'/v1/console/status')
        check(code==200 and 'committed_group' in reader_status and reader_status['storage']['retention_bytes']>0,'reader status reports the committed group and effective storage policy')
        check(reader_page['freshness'].get('browser-edge')!=receipt['second_canary_freshness_ns'],'independently collected canary fixtures have distinct freshness observations')
        # Freshness is the source's newest retained log, not its newest row, and a Spindle
        # also collects its own diagnostics every 15 s. The window must therefore cover
        # every log retained when the query runs, not end where `read` was created.
        all_logs_query=dict(read,contains='',to_ns=time.time_ns()+60_000_000_000)
        all_logs_code,all_logs=bearer_api(credential,'/v1/console/query','POST',all_logs_query)
        all_log_rows=list(all_logs['rows'])
        if all_logs['next_page']:
            next_code,next_logs=bearer_api(credential,'/v1/console/query','POST',dict(all_logs_query,page=all_logs['next_page']))
            check(next_code==200 and next_logs['next_page'] is None,'bounded complete canary log witness spans at most two pages')
            all_log_rows.extend(next_logs['rows'])
        check(all_logs_code==200 and all_logs['freshness'].get('browser-edge')==max(row['observed_ns'] for row in all_log_rows),'log query freshness reflects log observations without later metric timestamp')
        canary_code,canary_page=bearer_api(credential,'/v1/console/query','POST',dict(canary_query,contains='',to_ns=time.time_ns()+60_000_000_000))
        canary_rows={row['body']:row for row in canary_page['rows']}
        check(canary_code==200 and canary_page['next_page'] is None and canary_rows.get(canary_b,{}).get('observed_ns')==receipt['second_canary_observed_ns'] and canary_page['freshness']['browser-canary-b']==max(row['observed_ns'] for row in canary_page['rows']),'reader reads the second independently produced source with its own log freshness')
        metric_query={'kind':'metrics','node':'browser-edge','from_ns':read['from_ns'],'to_ns':read['to_ns'],'name':'system.cpu.time','limit':200,'page':None}
        metric_code,metric_page=bearer_api(credential,'/v1/console/query','POST',metric_query)
        check(metric_code==200 and len(metric_page['rows'])>0,'reader reads metric observations as well as logs')
        check(bearer_api(credential,'/v1/console/query','POST',dict(read,node='fabric-server-self',contains=''))[0]==200,'reader reads the companion source')
        page=reader_page['next_page']
        check(bool(page),'reader continuation handle issued')
        expect(bearer_api(credential,'/v1/console/query','POST',dict(read,page=page+'x')), 410, 'page expired; run the query again', 'modified opaque page handle rejected')
        expect(bearer_api(credential,'/v1/console/query','POST',dict(read,page=page,contains='changed')), 403, 'access denied', 'page handle bound to original query')
        expect(api('/v1/console/query','POST',dict(read,page=page)), 403, 'access denied', 'reader page handle cannot be reused by the owner session')
        if args.expired_cursor_probe:
            cursor_wait_start=time.monotonic()
            while time.monotonic()-cursor_wait_start<902:
                if time.monotonic()>deadline or storage_violation.is_set():raise RuntimeError('expired cursor fixture bound exceeded')
                time.sleep(1)
            expect(bearer_api(credential,'/v1/console/query','POST',dict(read,page=page)),410,'page expired; run the query again','actual expired opaque cursor rejected before history execution')
            check(bearer_api(credential,'/v1/console/nodes')[0]==200,'cursor expiry rejection has adjacent live credential witness')
            receipt['cursor_expiry_wait_s']=time.monotonic()-cursor_wait_start
            click('Reverify with passkey')
            wait(lambda:not js('return document.body.textContent.includes("Working…")'),'fresh passkey step-up after expiry wait')
        # Rotation is issue-then-revoke: a second reader coexists until the first is revoked.
        field('Reader name','Browser fixture second reader')
        field('Credential lifetime (seconds)','1800' if args.expired_cursor_probe else '600')
        click('Issue reader token')
        wait(lambda:js('return Array.from(document.querySelectorAll("code")).some(e=>/^[a-f0-9]{64}$/.test(e.textContent.trim()))'),'second reader token issuance')
        reader_token=js('return Array.from(document.querySelectorAll("code")).map(e=>e.textContent.trim()).find(value=>/^[a-f0-9]{64}$/.test(value))')
        click('Clear displayed credential')
        check(bearer_api(credential,'/v1/console/nodes')[0]==200 and bearer_api(reader_token,'/v1/console/nodes')[0]==200,'independent reader tokens coexist')
        field('Exact passkey, session or reader ID',issuance['credential_id'])
        click('revoke readers record')
        wd('/alert/accept',{})
        wait(lambda:bearer_api(credential,'/v1/console/nodes')[0]==401,'explicit revocation immediately retires one reader token')
        expect(bearer_api(credential,'/v1/console/nodes'), 401, 'sign in required', 'explicit revoked reader token no longer authorizes')
        check(bearer_api(reader_token,'/v1/console/nodes')[0]==200,'revoking one reader token leaves the other valid')
        credential=None
        check(api('/v1/console/session')[0] == 200, 'reader issuance and revocation preserve the owner session')
        first_handle=wd('/window',method='GET')
        second_handle=wd('/window/new',{'type':'tab'})['handle']
        wd('/window',{'handle':second_handle})
        wd('/url',{'url':origin+'/console/'})
        wait(lambda:js('return !!document.querySelector("[data-testid=connect-server]")'),'second tab shell')
        click('Connect to server')
        wait(lambda:js('return document.body.textContent.includes("Browser fixture owner · owner")'),'second tab validated same session')
        nav('Explore')
        field('Signal','logs')
        field('Source name (blank = all sources)','browser-edge')
        field('Body substring (case-sensitive)','browser-page-fixture')
        # The deliberate 902-second expiry wait ages these original rows beyond
        # a fresh tab's default 15-minute preset. Preserve the independently
        # selected producer window for this positive logout witness.
        js('document.querySelector("details.exact-range").open=true')
        field('Start (Unix ns)',str(read['from_ns']));field('End, exclusive (Unix ns)',str(read['to_ns']))
        click('Run query')
        wait(lambda:js('return document.querySelectorAll(".results tbody tr").length===200'),'second tab has positive protected-history witness before logout')
        js('window.fixtureLogoutFetch=window.fetch;window.fixtureLogoutResponseReady=false;window.fixtureLogoutDelivered=false;window.fixtureLogoutLockedAt=null;window.addEventListener("fabric-session-logout",()=>window.fixtureLogoutLockedAt=performance.now(),{once:true});window.fetch=async(...args)=>{const response=await window.fixtureLogoutFetch(...args);const url=typeof args[0]==="string"?args[0]:args[0].url;if(new URL(url,location.href).pathname==="/v1/console/query"){window.fixtureLogoutResponseReady=true;await new Promise(resolve=>setTimeout(resolve,5000));window.fixtureLogoutDeliveredAt=performance.now();window.fixtureLogoutDelivered=true;}return response}')
        click('Run query')
        wait(lambda:js('return window.fixtureLogoutResponseReady'),'real authenticated response is held before cross-tab logout')
        wd('/window',{'handle':first_handle})
        nav('Explore');field('Signal','logs');field('Source name (blank = all sources)','browser-edge');field('Body substring (case-sensitive)','browser-page-fixture')
        js('document.querySelector("details.exact-range").open=true')
        field('Start (Unix ns)',str(read['from_ns']));field('End, exclusive (Unix ns)',str(read['to_ns']))
        js('window.fixtureBusyLogoutFetch=window.fetch;window.fixtureBusyLogoutReady=false;window.fixtureBusyLogoutAbort=false;window.fixtureBusyLogoutDelivered=false;window.fixtureBusyLogoutComplete=false;window.fixtureBusyLogoutOrder=[];window.fetch=async(...args)=>{const url=typeof args[0]==="string"?args[0]:args[0].url;const path=new URL(url,location.href).pathname;window.fixtureBusyLogoutOrder.push(path+":start");if(path==="/v1/console/query"){const signal=args[0]?.signal||args[1]?.signal;if(signal)signal.addEventListener("abort",()=>window.fixtureBusyLogoutAbort=true,{once:true});}const response=await window.fixtureBusyLogoutFetch(...args);if(path==="/v1/console/query"){window.fixtureBusyLogoutReady=true;await new Promise(resolve=>setTimeout(resolve,1200));window.fixtureBusyLogoutDelivered=true;}window.fixtureBusyLogoutOrder.push(path+":delivered");if(path==="/v1/console/logout")window.fixtureBusyLogoutComplete=true;return response}')
        click('Run query');wait(lambda:js('return window.fixtureBusyLogoutReady'),'first tab holds actual reply before busy signout')
        logout_cookie=next(item for item in wd('/cookie',method='GET') if item['name'].startswith('__Host-'))
        click('Sign out')
        check(js('return !document.querySelector(".results") && document.body.textContent.includes("Sign in with a passkey") && !document.body.textContent.includes("Browser fixture owner · owner")'),'signout clears protected display immediately during pending read')
        wait(lambda:js('return window.fixtureBusyLogoutAbort'),'signout aborts pending real read transport')
        wait(lambda:js('return window.fixtureBusyLogoutDelivered && window.fixtureBusyLogoutComplete && !document.body.textContent.includes("Working…")'),'queued server logout completes after pending read transport')
        logout_order=js('return window.fixtureBusyLogoutOrder')
        receipt['busy_logout_fetch_order']=logout_order
        check(logout_order.count('/v1/console/logout:start')==1 and logout_order.index('/v1/console/query:delivered')<logout_order.index('/v1/console/logout:start'),'busy signout queues one CSRF logout without overlapping protected transports')
        js('window.fetch=window.fixtureBusyLogoutFetch;delete window.fixtureBusyLogoutFetch')
        wait(lambda: js('return document.body.textContent.includes("Signed out. In-memory telemetry cleared.")'), 'actual server logout')
        wd('/window',{'handle':second_handle})
        wait(lambda:js('return document.body.textContent.includes("Sign in to view your telemetry.")'),'cross-tab logout cleared protected views')
        check(js('return document.body.textContent.includes("another tab logged out")'),'broadcast carries logout to same-origin tab')
        wait(lambda:js('return window.fixtureLogoutDelivered && !document.body.textContent.includes("Working…")'),'held real reply completes after actual cross-tab logout')
        check(js('return window.fixtureLogoutLockedAt!==null && window.fixtureLogoutLockedAt<window.fixtureLogoutDeliveredAt && !document.querySelector(".results") && !document.body.textContent.includes("Browser fixture owner · owner")'),'late authenticated reply cannot restore protected display after actual logout')
        wd('/window',method='DELETE')
        wd('/window',{'handle':first_handle})
        expect(api('/v1/console/session'), 401, 'sign in required', 'logged-out cookie no longer grants session')
        check(bearer_api(reader_token,'/v1/console/nodes')[0]==200,'reader token remains independent of owner logout')
        recovery_credential=reader_token
        reader_token=None
        request=urllib.request.Request(origin+'/v1/console/session',headers={
            'Cookie':logout_cookie['name']+'='+logout_cookie['value'],'x-fabric-client-version':'1'})
        try:
            urllib.request.urlopen(request,context=context,timeout=10)
            raise AssertionError('copied stale session cookie unexpectedly accepted')
        except urllib.error.HTTPError as error:
            expect((error.code,json.load(error)),401,'sign in required','replayed stale session cookie rejected by server')
        logout_cookie=None
        field('Principal ID', principal_id)
        click('Use passkey')
        wait(lambda: js('return document.body.textContent.includes("Browser fixture owner · owner")'), 'actual WebAuthn assertion login')
        nav('Settings')
        current_cookie=next(item for item in wd('/cookie',method='GET') if item['name'].startswith('__Host-'))
        current_session_id=hashlib.sha256(current_cookie['value'].encode()).hexdigest()
        current_cookie=None
        code,sessions=api('/v1/console/sessions')
        check(code==200 and any(item['id']==current_session_id for item in sessions),'current session has immutable revocation target')
        field('Exact passkey, session or reader ID',current_session_id)
        click('revoke sessions record');wd('/alert/accept',{})
        wait(lambda:api('/v1/console/session')[0]==401,'self session revocation retires current cookie')
        expect(api('/v1/console/session'),401,'sign in required','revoked session cannot authorize')
        click('Check session')
        wait(lambda:js('return document.body.textContent.includes("Sign in to view your telemetry.")'),'revoked session locks UI')
        field('Principal ID',principal_id);click('Use passkey')
        wait(lambda:js('return document.body.textContent.includes("Browser fixture owner · owner")'),'fresh login after self-session revocation')
        click('Sign out')
        wait(lambda:js('return document.body.textContent.includes("Sign in to view your telemetry.")'),'owner sign out leaves the locked shell')
        check(js('return !document.querySelector(".results")'),'sign out leaves no previous telemetry display')
        field('Principal ID',principal_id);click('Use passkey')
        wait(lambda:js('return document.body.textContent.includes("Browser fixture owner · owner")'),'owner login after sign out')
        check(js('return !document.querySelector(".results")'),'relogin contains no previous telemetry display')
        check(edge.poll() is None,'native producer remains running after bounded expiry and polling')
        capture_api_metadata()
        if args.browser=='chrome':
            cdp('Network.enable',{})
            cdp('Network.emulateNetworkConditions',{'offline':True,'latency':0,'downloadThroughput':0,'uploadThroughput':0})
            wd('/url',{'url':'about:blank'})
            wd('/url',{'url':origin+'/console/'})
            wait(lambda:js('return !!document.querySelector("[data-testid=locked-screen]")'),'real offline cached public shell reopened')
            check(js('return !document.querySelector(".results") && !document.body.textContent.includes("browser-page-fixture")'),'reopened offline shell contains no previous sensitive rows')
            check(async_js('const done=arguments[arguments.length-1];fetch("/v1/console/session",{headers:{"x-fabric-client-version":"1"}}).then(()=>done(false)).catch(()=>done(true))'),'offline API request fails rather than replaying cached identity')
            cdp('Network.emulateNetworkConditions',{'offline':False,'latency':0,'downloadThroughput':-1,'uploadThroughput':-1})
        check(js('return !document.body.textContent.includes("Partial fixture")'), 'live workspace contains no synthetic evidence badges')
        if args.recovery:
            stop_browser_groups(group_parent, [b for b in owned_groups if b['group']['process'] is server])
            recovery=subprocess.run([str(args.server.resolve()),'recover-access',str(config),principal_id],capture_output=True,text=True,timeout=20)
            (out/'owner-recovery.log').write_text(recovery.stdout+recovery.stderr)
            check(recovery.returncode==0,'actual offline owner recovery command succeeds after server stop')
            server=launch('server-recovered',[str(args.server.resolve()),'serve',str(config)])
            wait(lambda:healthy(server),'production server reopened after owner recovery')
            expect(bearer_api(recovery_credential,'/v1/console/nodes'),401,'sign in required','offline recovery invalidates previously issued reader token')
            recovery_credential=None
            wd('/url',{'url':'about:blank'});wd('/url',{'url':origin+'/console/'})
            wait(lambda:js('return !!document.querySelector("[data-testid=connect-server]")'),'recovery public shell')
            click('Connect to server')
            wait(lambda:js('return document.body.textContent.includes("Sign in with a passkey") && !document.body.textContent.includes("Working…") && document.querySelector("[role=alert]")?.textContent.includes("HTTP 401: sign in required")'),'old identity session rejected after recovery epoch change')
            remove_authenticator(authenticator);authenticator=add_authenticator()
            replacement_secret=bootstrap_file.read_text().strip()
            js('Array.from(document.querySelectorAll("details")).find(e=>e.querySelector("button")?.textContent.trim()==="Create owner passkey").open=true')
            field('Display name','Browser fixture owner')
            field('One-time setup secret',replacement_secret);replacement_secret=None
            click('Create owner passkey')
            wait(lambda:js('return document.body.textContent.includes("Browser fixture owner · owner")'),'real WebAuthn owner enrollment after offline recovery')
            resident_credentials(authenticator, 'recovered owner enrollment')
            code,recovered_session=api('/v1/console/session')
            check(code==200 and recovered_session['principal_id']==principal_id,'offline owner recovery preserves immutable owner principal')
            check(not bootstrap_file.exists(),'replacement recovery secret consumed once')
        if args.shell_lifecycle:
            check(args.browser=='chrome','shell lifecycle fixture uses Chrome')
            worker=work/'console/service-worker.js'
            original_worker=worker.read_text()
            original_cache=original_worker.split('const CACHE = ')[1].split(';')[0].strip('"')
            def restart_server(worker_text):
                nonlocal server
                stop_browser_groups(group_parent, [b for b in owned_groups if b['group']['process'] is server])
                worker.write_text(worker_text)
                manifest_path=work/'console/asset-manifest.json'
                asset_manifest=json.loads(manifest_path.read_text())
                content=worker.read_bytes()
                asset_manifest['service-worker.js']={'bytes':len(content),'sha256':hashlib.sha256(content).hexdigest()}
                manifest_path.write_text(json.dumps(asset_manifest)+'\n')
                server=launch('server-lifecycle-'+str(len(processes)),[str(args.server.resolve()),'serve',str(config)])
                wait(lambda:healthy(server),'restarted production TLS server')
            def worker_state():
                return async_js('const done=arguments[arguments.length-1];navigator.serviceWorker.getRegistration().then(async r=>done({active:r?.active?.state,waiting:r?.waiting?.state,caches:await caches.keys()}))')
            def request_update():
                return async_js('const done=arguments[arguments.length-1];navigator.serviceWorker.getRegistration().then(r=>r.update()).then(()=>done(true)).catch(error=>done({error:String(error)}))')
            def close_controlled_tab():
                old_handle=wd('/window',method='GET')
                blank_handle=wd('/window/new',{'type':'tab'})['handle']
                wd('/window',{'handle':old_handle});wd('/window',method='DELETE')
                wd('/window',{'handle':blank_handle})
                async_js('const done=arguments[arguments.length-1];setTimeout(()=>done(true),500)')
                wd('/url',{'url':origin+'/console/'})
            failed_cache=original_cache+'-interrupted'
            failed_worker=original_worker.replace(original_cache,failed_cache).replace('const ASSETS = [','const ASSETS = ["/console/fixture-missing-shell.js",')
            restart_server(failed_worker)
            failed_update=request_update()
            wait(lambda:failed_cache not in worker_state()['caches'],'interrupted candidate cache removed')
            check(not worker_state()['waiting'] and original_cache in worker_state()['caches'],'interrupted update retains coherent active public shell')
            receipt['interrupted_shell_update']=failed_update
            candidate_cache=original_cache+'-candidate'
            restart_server(original_worker.replace(original_cache,candidate_cache))
            request_update()
            wait(lambda:worker_state()['waiting']=='installed','new shell waits while current tab remains open')
            check(original_cache in worker_state()['caches'] and candidate_cache in worker_state()['caches'],'open tab retains old shell alongside coherent waiting candidate')
            close_controlled_tab()
            wait(lambda:js('return !!navigator.serviceWorker.controller') and worker_state()['caches']==[candidate_cache],'candidate activates after old controlled tab closes')
            check(js('return !!document.querySelector("[data-testid=locked-screen]")'),'updated shell reopens locked after server restart')
            restart_server(original_worker)
            request_update()
            wait(lambda:worker_state()['waiting']=='installed','rollback shell waits without forced takeover')
            close_controlled_tab()
            wait(lambda:js('return !!navigator.serviceWorker.controller') and worker_state()['caches']==[original_cache],'rollback returns one coherent original shell')
            receipt['shell_lifecycle_scope']='real worker install/activation/rollback using same application assets and staged cache identities; no application schema migration claim'
            manifest_id=origin+'/console/'
            try:
                cdp('PWA.install',{'manifestId':manifest_id,'installUrlOrBundleUrl':manifest_id})
                cdp('PWA.changeAppUserSettings',{'manifestId':manifest_id,'displayMode':'standalone'})
                ordinary_handle=wd('/window',method='GET')
                launched=cdp('PWA.launch',{'manifestId':manifest_id})
                receipt['installed_pwa']={'target_id':launched['targetId'],'user_display_preference':'standalone','os_state':cdp('PWA.getOsAppState',{'manifestId':manifest_id})}
                wd('/window',{'handle':launched['targetId']})
                wait(lambda:js('return !!document.querySelector("[data-testid=locked-screen]")'),'actual installed PWA renders locked public shell')
                check(js('return matchMedia("(display-mode: standalone)").matches'),'installed PWA uses standalone display mode')
                check(js('return !document.querySelector(".results") && !document.body.textContent.includes("browser-page-fixture")'),'installed PWA opens without prior sensitive display')
                wd('/window',{'handle':ordinary_handle})
                cdp('Target.closeTarget',{'targetId':launched['targetId']})
                cdp('PWA.uninstall',{'manifestId':manifest_id})
                check(bool(launched['targetId']),'isolated desktop PWA installs launches and uninstalls')
            except BaseException:
                try:cdp('PWA.uninstall',{'manifestId':manifest_id})
                except RuntimeError:pass
                raise
        verify_binary_identity()
        receipt['exit'] = 0
    except BaseException as error:
        receipt['error'] = str(error)
        receipt['fixture_failed'] = True
        if session:
            try:
                receipt['actual_api_headers']=js('return window.fixtureApiHeaders||[]')
                receipt['failure_browser_windows']=wd('/window/handles',method='GET')
                receipt['failure_worker_state']=async_js('const done=arguments[arguments.length-1];if(!navigator.serviceWorker){done(null);return;}navigator.serviceWorker.getRegistration().then(async r=>done({active:r?.active?.state,waiting:r?.waiting?.state,caches:await caches.keys()})).catch(error=>done({error:String(error)}))')
                receipt['ceremony_diagnostic']=js('return window.fixtureCeremonyDiagnostic||null')
                if args.browser=='firefox':
                    wd('/moz/context',{'context':'chrome'})
                    try:
                        receipt['firefox_authenticator_diagnostic']=js('''const auth=ChromeUtils.importESModule("chrome://remote/content/marionette/webauthn.sys.mjs").webauthn;
                          return {exists:auth.hasVirtualAuthenticator(arguments[0]),credential_count:auth.getCredentials(arguments[0]).length,
                          ctap2:Services.prefs.getBoolPref("security.webauthn.ctap2",false),
                          usb:Services.prefs.getBoolPref("security.webauth.webauthn_enable_usbtoken",false),
                          prompt:document.querySelector("#notification-popup")?.textContent||null};''',authenticator)
                    finally:
                        wd('/moz/context',{'context':'content'})
                js('document.querySelectorAll("code").forEach(e=>{if(/^[a-f0-9]{64}$/.test(e.textContent.trim()))e.textContent="[fixture secret redacted]"})')
                (out/'failure.png').write_bytes(base64.b64decode(wd('/screenshot', method='GET')))
                (out/'failure-text.txt').write_text(js('return document.body.innerText'))
            except BaseException:
                pass
        raise
    finally:
        cleanup_error = None
        if session and viewport_restore_required:
            try:
                run_viewport('restore', viewport_context_handle)
                wd('/window/rect', {'width':1440,'height':1100})
                desktop_geometry = js('return {width:innerWidth,height:innerHeight}')
                if not isinstance(desktop_geometry,dict) or desktop_geometry.get('width') != 1440:
                    raise ValueError('desktop classic restore did not reach 1440 CSS pixels')
                viewport_restore_required = False
                receipt['failure_viewport_restore'] = {'inner_width':desktop_geometry['width']}
            except BaseException as error:
                cleanup_error = error
                receipt['viewport_restore_error'] = str(error)
        if session:
            try:
                call('DELETE', '/session/' + session)
            except BaseException:
                pass
        if announcements is not None:
            try:announcements.close()
            except BaseException as error:cleanup_error=cleanup_error or error
        try:
            browser_cleanup_provisionals(group_parent)
            stop_browser_groups(group_parent, owned_groups)
            browser_reap_domain(group_parent, owned_groups)
            end_browser_domain(group_parent, owned_groups)
        except BaseException as error:
            cleanup_error = cleanup_error or error
        finally:
            for label,clock in clock_targets.items():
                try:
                    if not getattr(clock,'closed',False):clock.close()
                except BaseException as error:
                    cleanup_error=cleanup_error or error
                finally:
                    receipt.setdefault('session_clock_ledgers',{})[label]={'rows':clock.rows,'frames':[frame.hex() for frame in clock.frames],'error':clock.error}
            stop_samples.set()
            sampler.join(timeout=2)
            if sampler.is_alive(): cleanup_error = cleanup_error or RuntimeError('browser sampler stop')
            (out/'resource-samples.json').write_text(json.dumps(samples, indent=2) + '\n')
            for log in logs: log.close()
            for held in state_descriptors:os.close(held['fd'])
            receipt['own_children_empty'] = group_parent.get('own_children_empty', False)
            receipt['reader_namespaces'] = group_parent.get('namespaces')
            receipt['process_exits'] = {label: proc.returncode for label, proc in processes}
            receipt['group_stops'] = [{key: binding['group'].get(key) for key in
                ('pid', 'start_ticks', 'entry_helper_sha256', 'entry_elapsed_ns',
                 'kernel_empty', 'children_reaped', 'late_reaped', 'stop_error')} for binding in owned_groups]
            receipt['provisional_children'] = [{key: record.get(key) for key in
                ('state', 'cleanup_error', 'children_reaped')} for record in
                group_parent.get('provisional_children', [])]
            if cleanup_error is None and receipt.get('exit') == 0:
                shutil.rmtree(work)
                receipt['cleanup'] = 'owned fixtures removed; process groups stopped and bound children reaped'
            elif cleanup_error is None:
                receipt['cleanup'] = 'failure evidence retained in owned fixture storage; process groups stopped and bound children reaped'
            else:
                receipt['cleanup_error'] = str(cleanup_error)
                receipt['exit'] = 1
            (out/'browser.json').write_text(json.dumps(receipt, indent=2) + '\n')
        if cleanup_error is not None: raise cleanup_error
        receipt['remaining_uncertainty'] = 'virtual authenticator only; no physical/synced passkeys, OS trust installation, mobile OS, general capacity or release qualification'
        (out/'browser.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps({'exit':receipt['exit'],'checks':len(checks),'out':str(out)}))


if __name__ == '__main__':
    main()
