"""Literal syscall transcripts; no native trace behavior is inferred."""
import json
import unittest
from pathlib import Path

import simulator_native_trace as trace

BEGIN = b'{"process":{"pid":7}}'
PATH = Path('/owned/begin.json')
STAGE = '/owned/.begin-witness-7.stage'
LINES = [
    'execve("/proc/self/fd/9", ["/proc/self/fd/9"], ["PATH=/usr/bin:/bin", "LANG=C", "LC_ALL=C"]) = 0 (DELAYED) <0.100001>',
    f'openat(AT_FDCWD, "{STAGE}", O_RDWR|O_CREAT|O_EXCL|O_NOFOLLOW|O_CLOEXEC, 0600) = 3<{STAGE}>',
    f'write(0x3, 0x1234, {hex(len(BEGIN))}) = {hex(len(BEGIN))}',
    f'fchmod(3<{STAGE}>, 0400) = 0',
    f'fsync(3<{STAGE}>) = 0',
    f'linkat(AT_FDCWD, "{STAGE}", AT_FDCWD, "/owned/begin.json", 0) = 0',
    f'unlink("{STAGE}") = 0',
    'fsync(4</owned>) = 0',
    f'fstat(3<{STAGE} (deleted)>, {{st_mode=S_IFREG|0400, st_ino=99, st_nlink=1, st_size={len(BEGIN)}}}) = 0',
    'clone3({flags=CLONE_VM|CLONE_THREAD}, 88) = 8',
    'exit_group(0) = ?',
    '+++ exited with 0 +++',
]


SHIM = 'execve("/proc/self/fd/10", ["/proc/self/fd/10", "-i", "PATH=/usr/bin:/bin", "LANG=C", "LC_ALL=C", "/proc/self/fd/9"], ["PATH=/usr/bin:/bin", "LANG=C", "LC_ALL=C"]) = 0'


def fixture(lines=None, shim=SHIM):
    # exit_group normally has no numeric return and is not a publication step.
    return ('\n'.join([shim] + (LINES[:-2] + LINES[-1:] if lines is None else lines)) + '\n').encode()


class NativeTraceLiteralTests(unittest.TestCase):
    def test_complete_literal_sequence(self):
        facts = trace.publication(fixture(), PATH, BEGIN, 99)
        self.assertEqual((facts['atomic'], facts['exclusive'], facts['before_first_offer']), (True, True, True))

    def test_missing_each_publication_step(self):
        for index in range(1, 9):
            with self.subTest(index=index), self.assertRaises(ValueError):
                trace.publication(fixture(LINES[:index] + LINES[index+1:-2] + LINES[-1:]), PATH, BEGIN, 99)

    def test_reordered_sync_link_unlink(self):
        for a, b in ((3, 4), (4, 5), (5, 6), (6, 7)):
            lines = list(LINES[:-2] + LINES[-1:]); lines[a], lines[b] = lines[b], lines[a]
            with self.subTest(a=a, b=b), self.assertRaises(ValueError):
                trace.publication(fixture(lines), PATH, BEGIN, 99)

    def test_short_write_and_substituted_count(self):
        for replacement in (LINES[2].rsplit(' = ', 1)[0] + ' = 0x1', LINES[2].replace(hex(len(BEGIN)) + ')', '0xff)')):
            lines = list(LINES[:-2] + LINES[-1:]); lines[2] = replacement
            with self.subTest(replacement=replacement), self.assertRaises(ValueError):
                trace.publication(fixture(lines), PATH, BEGIN, 99)

    def test_exclusive_nofollow_and_mode(self):
        for index, old, new in ((1, 'O_EXCL|', ''), (1, 'O_NOFOLLOW|', ''), (3, '0400', '0600')):
            lines = list(LINES[:-2] + LINES[-1:]); lines[index] = lines[index].replace(old, new)
            with self.subTest(old=old), self.assertRaises(ValueError):
                trace.publication(fixture(lines), PATH, BEGIN, 99)

    def test_wrong_destination_parent_and_inode(self):
        for index, old, new in ((5, '/owned/begin.json', '/owned/other.json'), (7, '4</owned>', '4</foreign>'), (8, 'st_ino=99', 'st_ino=100'), (8, 'st_nlink=1', 'st_nlink=2')):
            lines = list(LINES[:-2] + LINES[-1:]); lines[index] = lines[index].replace(old, new)
            with self.subTest(old=old), self.assertRaises(ValueError):
                trace.publication(fixture(lines), PATH, BEGIN, 99)

    def test_clone_before_publication_or_no_clone(self):
        for lines in (LINES[:1] + [LINES[9]] + LINES[1:9] + LINES[-1:], LINES[:9] + LINES[-1:]):
            with self.subTest(lines=lines), self.assertRaises(ValueError):
                trace.publication(fixture(lines), PATH, BEGIN, 99)

    def test_failed_syscalls_and_fork_escape(self):
        for index in range(1, 8):
            lines = list(LINES[:-2] + LINES[-1:]); lines[index] = lines[index].split(' = ')[0] + ' = -1 EIO (Input/output error) (INJECTED)'
            with self.subTest(index=index), self.assertRaises(ValueError):
                trace.publication(fixture(lines), PATH, BEGIN, 99)
        with self.assertRaisesRegex(ValueError, 'fork-escape'):
            trace.publication(fixture(LINES[:-2] + ['fork() = 12'] + LINES[-1:]), PATH, BEGIN, 99)

    def test_missing_delayed_exec_or_multiple_exec(self):
        for lines in ([LINES[0].replace(' (DELAYED)', '')] + LINES[1:-2] + LINES[-1:], [LINES[0]] + LINES[:-2] + LINES[-1:]):
            with self.subTest(lines=lines), self.assertRaises(ValueError):
                trace.publication(fixture(lines), PATH, BEGIN, 99)

    def test_incomplete_or_oversized_trace_refuses(self):
        with self.assertRaises(ValueError): trace.trace_calls(b'write(3, "x", 1 <unfinished ...>\n')
        with self.assertRaises(ValueError): trace.trace_calls(b'x' * (trace.MAX_TRACE + 1))

    def test_wrong_bootstrap_shim_or_environment(self):
        for old, new in (("/proc/self/fd/10", "/usr/bin/env"), ("-i", "-"), ("LANG=C", "LANG=foreign"), ("/proc/self/fd/9", "/proc/self/fd/8")):
            with self.subTest(old=old), self.assertRaises(ValueError):
                trace.publication(fixture(shim=SHIM.replace(old, new)), PATH, BEGIN, 99)
