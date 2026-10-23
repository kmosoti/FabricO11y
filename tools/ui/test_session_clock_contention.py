"""Native fixture-only lock counterexample; never product/session acceptance."""
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import tempfile
import time
import unittest

DELAY = r'''
#define _GNU_SOURCE
#include <stdatomic.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>
static atomic_int armed;
void test_clock_arm(void) { atomic_store(&armed, 1); }
int clock_gettime(clockid_t id, struct timespec *value) {
    if (id == CLOCK_REALTIME && atomic_exchange(&armed, 0)) {
        long duration = atol(getenv("FABRIC_TEST_HOLD_NS"));
        struct timespec wait = {0, duration};
        if (write(1, "held\n", 5) != 5) _exit(81);
        if (syscall(SYS_nanosleep, &wait, 0)) _exit(82);
    }
    return (int)syscall(SYS_clock_gettime, id, value);
}
'''
PROBE = r'''
#define _GNU_SOURCE
#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>
static unsigned long long raw(void) {
    struct timespec t;
    if (syscall(SYS_clock_gettime, CLOCK_MONOTONIC, &t)) _exit(83);
    return (unsigned long long)t.tv_sec * 1000000000ULL + t.tv_nsec;
}
static void *holder(void *unused) {
    (void)unused; struct timespec t;
    if (clock_gettime(CLOCK_REALTIME, &t)) _exit(84);
    return 0;
}
int main(void) {
    void (*arm)(void) = dlsym(RTLD_DEFAULT, "test_clock_arm");
    if (!arm) return 85;
    arm(); pthread_t thread;
    if (pthread_create(&thread, 0, holder, 0)) return 86;
    char go;
    if (read(0, &go, 1) != 1) return 87;
    unsigned long long before = raw();
    printf("enter %llu\n", before); fflush(stdout);
    struct timespec t;
    if (clock_gettime(CLOCK_MONOTONIC, &t)) return 88;
    unsigned long long after = raw();
    if (pthread_join(thread, 0)) return 89;
    printf("done %llu %llu %llu\n", before, after,
        (unsigned long long)t.tv_sec * 1000000000ULL + t.tv_nsec);
    return 0;
}
'''

class NativeContention(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory(prefix='session-clock-contention-', dir=os.environ['FABRIC_SCRATCH_ROOT'])
        cls.root = Path(cls.directory.name)
        selected = os.environ.get('FABRIC_SESSION_CLOCK_HELPER')
        cls.helper = Path(selected) if selected else cls.root/'current-clock.so'
        if not selected:
            subprocess.run(['/usr/bin/gcc','-shared','-fPIC','-pthread','-O2','-Wall','-Wextra','-Werror','-o',str(cls.helper),str(Path(__file__).with_name('session_clock.c')),'-ldl'],check=True,timeout=30)
        assert cls.helper.is_absolute() and cls.helper.is_file() and not cls.helper.is_symlink()
        for name, body in (('delay', DELAY), ('probe', PROBE)):
            source = cls.root/(name+'.c'); source.write_text(body)
            command = ['/usr/bin/gcc', '-O2', '-Wall', '-Wextra', '-Werror', '-pthread']
            command += ['-shared', '-fPIC'] if name == 'delay' else []
            subprocess.run(command+['-o', str(cls.root/(name+'.so' if name=='delay' else name)), str(source), '-ldl'], check=True, timeout=30)

    @classmethod
    def tearDownClass(cls):
        cls.directory.cleanup()

    def execute(self, hold):
        parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET|socket.SOCK_NONBLOCK|socket.SOCK_CLOEXEC)
        parent.send(b'FCLKv1\0\0'+bytes(16))
        env = dict(os.environ, LD_PRELOAD=str(self.helper)+':'+str(self.root/'delay.so'), FABRIC_CLOCK_FD=str(child.fileno()), FABRIC_TEST_HOLD_NS=str(hold))
        process = subprocess.Popen([str(self.root/'probe')], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, pass_fds=(child.fileno(),), start_new_session=True)
        child.close(); lines=[]; frames=[]
        try:
            self.assertTrue(select.select([process.stdout], [], [], 3)[0], 'holder readiness')
            self.assertEqual(process.stdout.readline(), b'held\n')
            process.stdin.write(b'g'); process.stdin.flush()
            stdout, stderr = process.communicate(timeout=3)
            lines=stdout.decode().splitlines()
            while True:
                try: data=parent.recv(257)
                except BlockingIOError: break
                if not data: break
                self.assertLessEqual(len(data),256);frames.append(json.loads(data))
            return process.returncode, lines, stderr.decode(), frames, time.monotonic_ns()
        finally:
            if process.poll() is None: process.kill();process.wait(timeout=2)
            parent.close()

    def test_short_native_contention_preserves_calls(self):
        status, lines, stderr, frames, _ = self.execute(50_000_000)
        self.assertEqual(status,0,'short lock contention must complete, not terminate: '+stderr)
        self.assertEqual(stderr,'')
        self.assertEqual(len(lines),2)
        _, before, after, value=lines[1].split()
        self.assertLessEqual(int(before),int(value));self.assertLessEqual(int(value),int(after))
        self.assertEqual([r['call_index'] for r in frames],list(range(1,len(frames)+1)))
        self.assertEqual({r['clock_id'] for r in frames},{0,1})
        self.assertTrue(all(r['raw_ns']==r['returned_ns'] and r['offset_s']==0 for r in frames))

    def test_stalled_native_holder_is_refused_with_finite_wait(self):
        status, lines, stderr, _, end=self.execute(250_000_000)
        self.assertEqual(status,78)
        self.assertEqual(stderr.strip(),'FCLK-LOCK')
        self.assertEqual(len(lines),1)
        begin=int(lines[0].split()[1]);elapsed=end-begin
        self.assertGreaterEqual(elapsed,80_000_000,'must exercise the registered monotonic wait, not the old yield-count refusal')
        self.assertLessEqual(elapsed,1_000_000_000,'bounded native refusal')

if __name__=='__main__': unittest.main()
