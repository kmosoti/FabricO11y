"""Bounded initial-thread syscall observer for the separately frozen helper."""
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import select
import signal
import stat
import subprocess
import time

POPEN = subprocess.Popen
MAX_TRACE = 32 * 2**20
BOOTSTRAP_ENV = "/usr/bin/env"
BOOTSTRAP_ENV_SHA256 = "505168d7e1cc5ad8e7dc93d2b5c3867987bf605ff323b679c847441ca1569eeb"
MINIMAL_ENV = {"PATH": "/usr/bin:/bin", "LANG": "C", "LC_ALL": "C"}
SYSCALLS = 'execve,execveat,open,openat,write,writev,fchmod,fsync,fdatasync,link,linkat,unlink,unlinkat,clone,clone3,fork,vfork,fstat,newfstatat,statx,close,close_range,dup,dup2,dup3,fcntl'
QUOTED = re.compile(r'"(?:[^"\\]|\\.)*"')


def require(value, reason):
    if not value:
        raise ValueError('native-trace-' + reason)


def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def strings(arguments):
    return [ast.literal_eval(value) for value in QUOTED.findall(arguments)]


def trace_calls(raw):
    require(type(raw) is bytes and len(raw) <= MAX_TRACE, 'bound')
    calls, offset = [], 0
    for line in raw.decode('ascii').splitlines(keepends=True):
        text = line.strip()
        match = re.fullmatch(r'([a-z0-9_]+)\((.*)\)\s+=\s+(-?(?:0x[0-9a-f]+|\d+))(.*)', text)
        if match:
            calls.append(dict(name=match[1], args=match[2], result=int(match[3], 0), tail=match[4],
                              offset=offset, line=text))
        else:
            require(not text or text.startswith(('--- ', '+++ ')), 'incomplete-or-unknown-line')
        offset += len(line.encode('ascii'))
    return calls


def publication(raw, begin_path, begin_raw, inode):
    """Facts from one ordered trace, plus independently observed final bytes/inode."""
    begin_path = Path(begin_path)
    value = json.loads(begin_raw)
    pid = value['process']['pid']
    stage = str(begin_path.parent / f'.begin-witness-{pid}.stage')
    calls = trace_calls(raw)
    execution = [call for call in calls if call['name'] in ('execve', 'execveat')]
    require(len(execution) == 2 and all(call['result'] == 0 and call['name'] == 'execve' for call in execution)
            and '(DELAYED)' not in execution[0]['tail'] and '(DELAYED)' in execution[1]['tail'], 'exec')
    shim, target = [ast.literal_eval('[' + call['args'] + ']') for call in execution]
    environment = [key + '=' + value for key, value in MINIMAL_ENV.items()]
    require(len(shim) == len(target) == 3 and re.fullmatch(r'/proc/self/fd/[0-9]+', shim[0])
            and shim[1][0] == shim[0] and shim[1][1:5] == ['-i', *environment]
            and shim[1][5:] == target[1] and target[0] == target[1][0]
            and re.fullmatch(r'/proc/self/fd/[0-9]+', target[0])
            and shim[2] == target[2] == environment, 'bootstrap-exec')
    require(not any(call['name'] in ('fork', 'vfork') for call in calls), 'fork-escape')
    opened = [call for call in calls if call['name'] in ('open', 'openat') and stage in strings(call['args'])]
    require(len(opened) == 1 and opened[0]['result'] >= 0
            and all(flag in opened[0]['args'] for flag in ('O_RDWR', 'O_CREAT', 'O_EXCL', 'O_NOFOLLOW'))
            and re.search(r', 0600$', opened[0]['args']), 'exclusive-stage')
    descriptor = opened[0]['result']
    prefix = f'{descriptor}<{stage}>'
    def raw_write(call):
        if call['name'] not in ('write', 'writev'): return None
        match = re.fullmatch(r'(0x[0-9a-f]+|\d+), (0x[0-9a-f]+|\d+), (0x[0-9a-f]+|\d+)', call['args'])
        require(match is not None, 'redacted-write-format')
        return tuple(int(part, 0) for part in match.groups())
    def singular(name, predicate):
        selected = [call for call in calls if call['name'] in name and predicate(call)]
        require(len(selected) == 1 and selected[0]['result'] >= 0, 'operation-' + '-'.join(name))
        return selected[0]
    closing = [call for call in calls if call['name'] == 'close' and call['offset'] > opened[0]['offset']
               and re.match(r'^' + str(descriptor) + r'(?:<|$)', call['args'])]
    lifetime_end = closing[0]['offset'] if closing else len(raw)
    writes = [call for call in calls if call['name'] in ('write', 'writev')
              and opened[0]['offset'] < call['offset'] < lifetime_end and raw_write(call)[0] == descriptor]
    require(len(writes) == 1 and writes[0]['name'] == 'write', 'sole-stage-write')
    write = writes[0]
    require(write['result'] == raw_write(write)[2] == len(begin_raw), 'complete-write-count')
    require(not any(opened[0]['offset'] < call['offset'] < lifetime_end
                    and (call['name'] in ('dup', 'dup2', 'dup3', 'close_range')
                         or (call['name'] in ('open', 'openat') and call['result'] == descriptor)
                         or (call['name'] == 'fcntl' and call['args'].startswith(prefix) and 'F_DUPFD' in call['args']))
                    for call in calls), 'stage-fd-reuse')
    mode = singular(('fchmod',), lambda call: call['args'].startswith(prefix + ', '))
    require(mode['result'] == 0 and mode['args'].endswith(', 0400'), 'mode')
    synced = singular(('fsync',), lambda call: call['args'] == prefix)
    linked = singular(('link', 'linkat'), lambda call: strings(call['args']) == [stage, str(begin_path)])
    require(linked['result'] == 0 and (linked['name'] != 'linkat' or linked['args'].endswith(', 0')), 'no-replace-link')
    unlinked = singular(('unlink', 'unlinkat'), lambda call: strings(call['args']) == [stage])
    require(unlinked['result'] == 0, 'stage-unlink')
    directory = singular(('fsync',), lambda call: re.fullmatch(r'\d+<' + re.escape(str(begin_path.parent)) + '>', call['args']) is not None)
    require(directory['result'] == 0, 'directory-sync')
    steps = [opened[0], write, mode, synced, linked, unlinked, directory]
    require(all(a['offset'] < b['offset'] for a, b in zip(steps, steps[1:])), 'publication-order')
    clones = [call for call in calls if call['name'] in ('clone', 'clone3')]
    require(clones and all(call['offset'] > directory['offset'] for call in clones), 'before-worker')
    metadata = [call for call in calls if call['name'] in ('fstat', 'newfstatat', 'statx')
                and call['result'] == 0 and unlinked['offset'] < call['offset'] < clones[0]['offset']
                and (stage in call['args'] or str(begin_path) in strings(call['args']))
                and re.search(r'st(?:x)?_ino=' + str(inode) + r'(?:,|})', call['args'])
                and re.search(r'st(?:x)?_nlink=1(?:,|})', call['args'])
                and 'S_IFREG|0400' in call['args']]
    require(metadata, 'final-inode-metadata')
    require(lifetime_end > metadata[0]['offset'], 'stage-held-through-publication')
    return dict(atomic=True, exclusive=True, before_first_offer=True, regular=True,
                nlink=1, mode=0o400, unchanged_sha256=hashlib.sha256(begin_raw).hexdigest(),
                source_mapping='offers only in workers spawned after synchronous publication',
                operation_offsets={call['name'] + ':' + str(index): call['offset'] for index, call in enumerate(steps)},
                first_clone_offset=clones[0]['offset'], metadata_offset=metadata[0]['offset'])


class NativeTrace:
    @classmethod
    def start(cls, command, binary_fd, out, *, strace, strace_sha256, deadline,
              stdout=None, stderr=None, injection=None):
        self = cls()
        self.out, self.deadline = Path(out), deadline
        self.binary_fd, self.target_pidfd = os.dup(binary_fd), None
        self.bootstrap_fd = None
        self.files, self.target_pid, self.returncode = [], None, None
        self.tracer = None
        self.record = dict(state='starting', command=command, bootstrap_delay='first post-startup execve delay_exit=100ms')
        try:
            require(self.out.is_absolute() and not self.out.exists()
                    and self.out.parent.resolve().is_relative_to(Path('/run/media/kmosoti/data/FabricO11y/results')), 'output')
            self.out.mkdir(mode=0o700)
            require(not Path(strace).is_symlink() and sha(strace) == strace_sha256, 'tool-pin')
            original = os.fstat(self.binary_fd)
            self.binary_sha256 = sha(Path('/proc/self/fd') / str(self.binary_fd))
            require(command[0] == f'/proc/self/fd/{binary_fd}', 'held-exec-command')
            require(not any(name in os.environ for name in ('LD_PRELOAD', 'LD_AUDIT', 'LD_LIBRARY_PATH')), 'environment')
            self.trace = self.out / 'syscalls.raw'
            for name, stream in (('stdout', stdout), ('stderr', stderr)):
                if stream is None:
                    stream = (self.out / name).open('xb'); self.files.append(stream)
                setattr(self, name, stream)
            argv = [str(strace), '-q', '-T', '-v', '-x', '-yy', '-s', '131072', '-o', str(self.trace),
                    '-e', 'trace=' + SYSCALLS, '-e', 'raw=write,writev', '-e', 'inject=execve:delay_exit=100ms:when=1']
            if injection is not None:
                require(re.fullmatch(r'(write|fsync|link|linkat|unlink|unlinkat):error=EIO:when=[1-9][0-9]*', injection), 'injection')
                argv += ['-e', 'inject=' + injection]
            self.bootstrap_fd = os.open(BOOTSTRAP_ENV, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
            bootstrap_info = os.fstat(self.bootstrap_fd)
            require(stat.S_ISREG(bootstrap_info.st_mode)
                    and sha(Path('/proc/self/fd') / str(self.bootstrap_fd)) == BOOTSTRAP_ENV_SHA256, 'bootstrap-tool')
            bootstrap_command = [f'/proc/self/fd/{self.bootstrap_fd}', '-i',
                *[key + '=' + value for key, value in MINIMAL_ENV.items()], *command]
            self.record['bootstrap'] = dict(source=BOOTSTRAP_ENV, sha256=BOOTSTRAP_ENV_SHA256,
                device=bootstrap_info.st_dev, inode=bootstrap_info.st_ino, command=bootstrap_command)
            argv += ['--', *bootstrap_command]
            self.record.update(tracer_command=argv, tool_sha256=strace_sha256, launched_monotonic_ns=time.monotonic_ns())
            environment = dict(MINIMAL_ENV)
            self.record['environment'] = environment
            self.tracer = POPEN(argv, pass_fds=(binary_fd, self.bootstrap_fd), stdout=self.stdout, stderr=self.stderr, env=environment)
            until = min(deadline, time.monotonic() + 5)
            while time.monotonic() < until and self.tracer.poll() is None:
                children = Path(f'/proc/{self.tracer.pid}/task/{self.tracer.pid}/children').read_text().split()
                require(len(children) <= 1, 'tracer-child-domain')
                if children:
                    pid = int(children[0]); proc = Path('/proc') / str(pid)
                    try:
                        first = (proc / 'stat').read_text().rsplit(')', 1)[1].split()
                        loaded = (proc / 'exe').stat()
                        if (loaded.st_dev, loaded.st_ino) == (original.st_dev, original.st_ino):
                            descriptor = os.pidfd_open(pid)
                            self.target_pidfd = descriptor
                            actual_sha = sha(proc / 'exe')
                            last = (proc / 'stat').read_text().rsplit(')', 1)[1].split()
                            require(first[19] == last[19] and actual_sha == self.binary_sha256
                                    and last[0] == 't' and len(list((proc / 'task').iterdir())) == 1, 'bootstrap-binding')
                            self.target_pid, self.target_pidfd = pid, descriptor
                            self.identity = dict(pid=pid, start_ticks=int(last[19]),
                                boot_id=Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
                                exe_device=loaded.st_dev, exe_inode=loaded.st_ino)
                            self.record.update(state='bound', process=self.identity, binary_sha256=self.binary_sha256,
                                bound_monotonic_ns=time.monotonic_ns(), bootstrap_state=last[0], tracer_pid=self.tracer.pid)
                            return self
                    except FileNotFoundError:
                        pass
                time.sleep(.001)
            raise ValueError('native-trace-live-binding-unavailable')
        except BaseException:
            self.abort()
            raise

    @property
    def pid(self):
        return self.target_pid

    def poll(self):
        if self.trace.exists(): require(self.trace.stat().st_size <= MAX_TRACE, 'bound')
        if self.returncode is not None: return self.returncode
        if self.tracer.poll() is None: return None
        raw = self.trace.read_bytes()
        status = re.findall(rb'^\+\+\+ exited with ([0-9]+) \+\+\+\s*$', raw, re.M)
        killed = re.findall(rb'^\+\+\+ killed by (SIG[A-Z0-9]+).*$', raw, re.M)
        require(len(status) + len(killed) == 1, 'target-exit')
        self.returncode = int(status[0]) if status else -int(getattr(signal, killed[0].decode()))
        return self.returncode

    def wait(self, timeout=None):
        until = min(self.deadline, time.monotonic() + timeout if timeout is not None else self.deadline)
        while self.poll() is None:
            if time.monotonic() >= until: raise subprocess.TimeoutExpired(self.record['command'], timeout)
            time.sleep(.005)
        self.tracer.wait(timeout=max(0, until-time.monotonic()))
        return self.returncode

    def send_signal(self, number):
        require(self.target_pidfd is not None, 'unbound-signal')
        signal.pidfd_send_signal(self.target_pidfd, number)

    def kill(self):
        self.send_signal(signal.SIGKILL)

    def finish(self, beginfile):
        try:
            return self._finish(beginfile)
        except BaseException as error:
            self.record['error'] = type(error).__name__ + ': ' + str(error)
            self.abort()
            raise

    def _finish(self, beginfile):
        require(self.wait(timeout=max(0, self.deadline-time.monotonic())) == 0, 'successful-helper')
        require(self.tracer.returncode == 0, 'successful-tracer')
        beginfile = Path(beginfile)
        info = beginfile.lstat()
        require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and stat.S_IMODE(info.st_mode) == 0o400
                and info.st_size <= 131072, 'begin-file')
        raw = beginfile.read_bytes()
        value = json.loads(raw)
        require(value['process'] == self.identity and value['binary_sha256'] == self.binary_sha256, 'begin-identity')
        facts = publication(self.trace.read_bytes(), beginfile, raw, info.st_ino)
        self.record.update(state='stopped', exit=self.returncode, tracer_exit=self.tracer.returncode,
            trace_sha256=sha(self.trace), begin_sha256=hashlib.sha256(raw).hexdigest(), begin_inode=info.st_ino,
            begin_path=str(beginfile),
            publication=facts, stopped_monotonic_ns=time.monotonic_ns())
        self.archive()
        self.release()
        return self.record

    def archive(self):
        if self.out.exists(): (self.out / 'observer-receipt.json').write_text(json.dumps(self.record, indent=2) + '\n')

    def release(self):
        for name in ('binary_fd', 'target_pidfd', 'bootstrap_fd'):
            descriptor = getattr(self, name, None)
            if descriptor is not None: os.close(descriptor); setattr(self, name, None)
        for stream in self.files: stream.close()
        self.files = []

    def abort(self):
        try:
            self.record['outer_campaign_stop_required'] = self.tracer is not None and self.target_pidfd is None
            if self.target_pidfd is not None:
                try: signal.pidfd_send_signal(self.target_pidfd, signal.SIGKILL)
                except ProcessLookupError: pass
            if self.tracer is not None:
                try: self.tracer.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.tracer.kill(); self.tracer.wait(timeout=5)
            self.record.update(state='failed', tracer_exit=None if self.tracer is None else self.tracer.returncode)
        finally:
            try: self.archive()
            finally: self.release()

    @staticmethod
    def validate_stopped(rawtrace_dir, beginfile, expected):
        directory, beginfile = Path(rawtrace_dir), Path(beginfile)
        receipt = directory / 'observer-receipt.json'
        trace = directory / 'syscalls.raw'
        require(sha(receipt) == expected['observer_receipt_sha256'] and sha(trace) == expected['trace_sha256']
                and sha(beginfile) == expected['begin_sha256'], 'offline-expected-pins')
        record = json.loads(receipt.read_text())
        require(record['state'] == 'stopped' and record['exit'] == record['tracer_exit'] == 0
                and record['process'] == expected['process']
                and record['binary_sha256'] == expected['binary_sha256']
                and record['bootstrap']['sha256'] == BOOTSTRAP_ENV_SHA256, 'offline-identity')
        require(beginfile.stat().st_ino == record['begin_inode'], 'offline-inode')
        facts = publication(trace.read_bytes(), Path(record['begin_path']), beginfile.read_bytes(), record['begin_inode'])
        require(facts == record['publication'], 'offline-publication')
        return facts
