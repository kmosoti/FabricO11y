from contextlib import contextmanager
import ctypes
import os
from pathlib import Path
import signal
import shutil
import stat
import subprocess
import tempfile
import time
import unittest
import sys
from unittest.mock import patch
import payload
from payload import cpio, identity


def fixture_live(directory):
    pid, start = map(int, (directory / 'child.pid').read_text().split())
    try:
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    except FileNotFoundError:
        return False
    if int(fields[19]) != start:
        raise AssertionError('owned fixture PID identity changed')
    return fields[0] != 'Z'


@contextmanager
def owned_process_fixture():
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    from resource_group import require_limits
    require_limits()
    libc, previous = ctypes.CDLL(None, use_errno=True), ctypes.c_int()
    if libc.prctl(37, ctypes.byref(previous), 0, 0, 0) or libc.prctl(36, 1, 0, 0, 0):
        raise RuntimeError('owned process control requires Linux subreaper')
    try:
        directory = Path(tempfile.mkdtemp(dir=os.environ['FABRIC_SCRATCH_ROOT'], prefix='package-process-control-'))
        script = directory / 'child.py'
        script.write_text('''import os, select, signal, sys, time
from pathlib import Path
directory, mode = Path(sys.argv[1]), sys.argv[2]
stage = Path(sys.argv[3]) if len(sys.argv) > 3 else directory / 'owned-stage'
expires = time.monotonic() + 45
if os.getpgrp() != os.getpid(): os.setsid()
def atomic(name, body):
    temporary = directory / (name + '.pending')
    with temporary.open('x') as stream:
        stream.write(body); stream.flush(); os.fsync(stream.fileno())
    temporary.replace(directory / name)
def identity(name, pid):
    process = Path(f'/proc/{pid}')
    fields = (process / 'stat').read_text().rsplit(')', 1)[1].split()
    if process.stat().st_uid != os.getuid() or int(fields[19]) <= 0:
        raise RuntimeError('unowned fixture identity')
    if pid != os.getpid() and (int(fields[1]) != os.getpid() or int(fields[2]) != os.getpid()):
        raise RuntimeError('fork child is outside owned leader')
    atomic(name, f'{pid} {fields[19]}')
identity('leader.pid', os.getpid())
if mode == 'drain': signal.signal(signal.SIGTERM, signal.SIG_IGN)
gate_read, gate_write = os.pipe()
child = os.fork()
if child == 0:
    os.close(gate_write)
    if not select.select([gate_read], [], [], 5)[0] or os.read(gate_read, 1) != b'1': os._exit(1)
    os.close(gate_read)
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    with open(os.devnull, 'wb') as null:
        os.dup2(null.fileno(), 1); os.dup2(null.fileno(), 2)
    stage.mkdir(exist_ok=True)
    held = (stage / 'held-stage.bin').open('wb')
    held.write(b'literal owned staging'); held.flush()
    atomic('ready', 'ready')
    time.sleep(max(0, expires - time.monotonic()))
    os._exit(0)
os.close(gate_read)
try:
    identity('child.pid', child)
    os.write(gate_write, b'1')
finally:
    os.close(gate_write)
deadline = time.monotonic() + 5
while not (directory / 'ready').exists():
    if time.monotonic() >= deadline: raise RuntimeError('child readiness deadline')
    time.sleep(.01)
print('literal process stdout', flush=True)
print('literal process stderr', file=sys.stderr, flush=True)
atomic('diagnostics-ready', 'ready')
if mode != 'success': time.sleep(max(0, expires - time.monotonic()))
''')
        try:
            yield directory, [sys.executable, '-B', str(script), str(directory)]
        finally:
            cleanup_errors = []
            for name in ('leader.pid', 'child.pid'):
                try:
                    marker = directory / name
                    info = marker.lstat()
                    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1 or info.st_size > 128:
                        raise AssertionError('invalid owned fixture identity marker')
                    pid, start = map(int, marker.read_text().split())
                    if pid <= 1 or start <= 0:
                        raise AssertionError('invalid owned fixture PID/start identity')
                    try:
                        descriptor = os.pidfd_open(pid)
                    except ProcessLookupError:
                        continue
                    try:
                        try:
                            fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
                        except FileNotFoundError:
                            continue
                        if int(fields[19]) != start:
                            raise AssertionError('refuse cleanup of reused fixture PID')
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
                                fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
                            except FileNotFoundError:
                                break
                            if int(fields[19]) != start:
                                raise AssertionError('fixture PID changed before confirmed reap')
                            if time.monotonic() >= deadline:
                                raise AssertionError('owned fixture cleanup deadline')
                            time.sleep(.01)
                    finally:
                        os.close(descriptor)
                except (OSError, ValueError, IndexError, AssertionError) as error:
                    cleanup_errors.append(name + ': ' + str(error))
            if cleanup_errors:
                raise AssertionError('fixture scratch retained at ' + str(directory) + ': ' + '; '.join(cleanup_errors))
            shutil.rmtree(directory)
    finally:
        if libc.prctl(36, previous.value, 0, 0, 0):
            raise RuntimeError('restore test subreaper setting')


def assert_fixture_stopped(test, directory):
    test.assertFalse(fixture_live(directory), 'command returned with live owned descendant')


def entry(name, body=b'', mode=stat.S_IFREG | 0o644):
    name = name.encode() + b'\0'
    fields = [1, mode, 0, 0, 1, 0, len(body), 0, 0, 0, 0, len(name), 0]
    header = b'070701' + ''.join(f'{value:08x}' for value in fields).encode()
    result = header + name
    result += b'\0' * (-len(result) % 4)
    result += body
    return result + b'\0' * (-len(result) % 4)


class PayloadTests(unittest.TestCase):
    def test_extraction_stops_real_descendant_on_success_and_timeout(self):
        # Origin: static cleanup review; literal child ignores TERM and closes pipes.
        for mode in ('success', 'timeout'):
            with self.subTest(mode=mode), owned_process_fixture() as (directory, command):
                if mode == 'success':
                    self.assertEqual(payload.bounded_command(command + [mode]), b'literal process stdout\n')
                else:
                    with self.assertRaises(subprocess.TimeoutExpired) as caught:
                        payload.bounded_command(command + [mode])
                    self.assertEqual(caught.exception.timeout, 30)
                    self.assertIn(b'literal process stderr', caught.exception.stderr)
                assert_fixture_stopped(self, directory)

    def test_extraction_failure_retains_stderr_and_command(self):
        # Origin: #59 C-041 nit4; the bounded reader discarded failure stderr.
        command = [sys.executable, '-c', 'import sys; sys.stderr.write("literal archive diagnostic\\n"); sys.exit(7)']
        with self.assertRaises(ValueError) as caught:
            payload.bounded_command(command)
        self.assertIn('literal archive diagnostic', str(caught.exception))
        self.assertIn(sys.executable, str(caught.exception))

    def test_extraction_stops_at_cap_before_materializing_oversized_output(self):
        with patch.object(payload, 'CAP', 1024):
            self.assertEqual(payload.bounded_command([sys.executable, '-c', 'print("small")']), b'small\n')
            with self.assertRaisesRegex(ValueError, 'bounded inventory cap'):
                payload.bounded_command([sys.executable, '-c', 'import sys; sys.stdout.buffer.write(b"x"*4096)'])

    def test_exact_content_inventory_with_complete_trailer(self):
        raw = entry('./usr/bin/fabric-node', b'ELF') + entry('TRAILER!!!')
        self.assertEqual(cpio(raw), {'usr/bin/fabric-node': identity(b'ELF')})
        for invalid in [raw[:-100], raw.replace(b'070701', b'070702', 1),
                        entry('../usr/bin/bad', b'bad') + entry('TRAILER!!!'),
                        entry('/usr/bin/bad', b'bad') + entry('TRAILER!!!'),
                        entry('usr/bin/link', b'elsewhere', stat.S_IFLNK | 0o777) + entry('TRAILER!!!'),
                        entry('usr/bin/x', b'a') + entry('usr/bin/x', b'b') + entry('TRAILER!!!')]:
            with self.subTest(invalid=invalid[:15]), self.assertRaises(ValueError):
                cpio(invalid)


if __name__ == '__main__':
    unittest.main()
