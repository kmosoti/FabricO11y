"""Raw-clock protocol failures use actual pipes/process exits; no VM credit."""
import json
import os
import pty
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest
from concurrent.futures import ThreadPoolExecutor
from unittest.mock import patch

import clock_channel as C
from guest import Guest
from remote import RemoteGuest

LIMITS = {'memory.max':'33554432','memory.swap.max':'0','cpu.max':'25000 100000','pids.max':'8'}


def program(worker=C.WORKER, boot='boot', limits=None, runtime='10min'):
    # This is an explicitly fake guest kernel view. Real unit enforcement is a
    # fresh disposable smoke obligation, not a result of these pipe controls.
    return '''import sys,subprocess
from pathlib import Path
limits=%r
def read(path,*args,**kwargs):
 if str(path)=='/proc/self/cgroup':return '0::/system.slice/cross-clock-edge.service\\n'
 if str(path)=='/proc/sys/kernel/random/boot_id':return %r
 return limits[path.name]
Path.read_text=read
subprocess.check_output=lambda *a,**k:%r
sys.argv=['worker','cross-clock-edge.service']
exec(%r)
''' % (LIMITS if limits is None else limits, boot, runtime, worker)


class ClockControls(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.guest = types.SimpleNamespace(role='edge', boot_id='boot', results=Path(self.directory.name),
            ssh=['ssh','owned-guest'], script=lambda *a,**k:subprocess.CompletedProcess(a,0,
                'LoadState=not-found\nActiveState=inactive\n',''))

    def start(self, code=None):
        channel = C.ClockChannel(self.guest)
        original = subprocess.Popen
        def local_transport(command, **kwargs):
            self.command = command
            return original([sys.executable,'-I','-u','-c',program() if code is None else code], **kwargs)
        with patch.object(C.subprocess,'Popen',side_effect=local_transport):
            channel.start()
        self.addCleanup(lambda: channel.close(abort=True) if not channel.closed else None)
        return channel

    def test_owned_process_reused_and_sequences_serialized_across_threads(self):
        channel = self.start()
        pid = channel.process.pid
        with ThreadPoolExecutor(max_workers=4) as pool:
            rows = list(pool.map(lambda _:channel.correlation(),range(40)))
        self.assertEqual(channel.process.pid,pid)
        self.assertEqual(channel.samples,40)
        self.assertTrue(all(x['boot_id']=='boot' and x['monotonic_before_ns']<=x['monotonic_after_ns']
                            and x['host_request_before_ns']<=x['host_request_after_ns'] for x in rows))
        self.assertTrue(channel.close())
        self.assertTrue(channel.close())
        self.assertEqual(channel.record['exit'],0)
        self.assertEqual(channel.record['stop_reply']['samples'],40)
        import shlex
        payload=shlex.split(self.command[-1])
        for value in ('--property=MemoryMax=32M','--property=MemorySwapMax=0','--property=CPUQuota=25%',
                      '--property=TasksMax=8','--property=RuntimeMaxSec=600','--wait','--collect','--pty'):
            self.assertIn(value,payload)
        self.assertEqual(payload[payload.index('-c')+1],C.WORKER)
        self.assertNotIn('--pipe',payload)

    def test_real_pty_is_raw_without_echo_or_newline_translation(self):
        master,slave=pty.openpty()
        process=subprocess.Popen([sys.executable,'-I','-u','-c',program()],stdin=slave,
                                 stdout=slave,stderr=subprocess.PIPE)
        os.close(slave)
        try:
            with os.fdopen(master,'rb',buffering=0) as stream:
                channel=C.ClockChannel(self.guest)
                channel.process=types.SimpleNamespace(stdout=stream)
                self.assertEqual(channel.reply()['event'],'ready')
                for sequence in range(1,4):
                    os.write(stream.fileno(),(json.dumps(dict(action='sample',sequence=sequence))+'\n').encode())
                    value=channel.reply()
                    self.assertEqual(value['event'],'sample')  # an echoed request is never a sample
                    self.assertEqual(value['sequence'],sequence)
                os.write(stream.fileno(),b'{"action":"stop","sequence":4}\n')
                self.assertEqual(channel.reply(),dict(event='stopped',sequence=4,samples=3))
                self.assertEqual(process.wait(timeout=5),0)
                self.assertEqual(process.stderr.read(),b'')
        finally:
            if process.poll() is None:process.kill();process.wait(timeout=5)
            process.stderr.close()

    def test_wrong_startup_identity_and_each_enforced_limit_refused(self):
        variants=[program(boot='wrong'),program(runtime='infinity')]
        variants += [program(limits={**LIMITS,key:'max'}) for key in LIMITS]
        for code in variants:
            with self.subTest(code=code[-40:]):
                # Each failed start retains its stderr and original exit.
                for path in Path(self.directory.name).glob('*'):path.unlink()
                with self.assertRaises((EOFError,ValueError)):
                    self.start(code)
                record=json.loads((self.guest.results/'edge-clock-channel.json').read_text())
                self.assertTrue(record['stop_confirmed'])
                self.assertIn('startup_error',record)
                self.assertIn('error',record)

    def test_wrong_sample_sequence_boot_and_order_refused(self):
        variants=[C.WORKER.replace("'sequence':samples,'boot_id':boot", "'sequence':samples+1,'boot_id':boot"),
                  C.WORKER.replace("'sequence':samples,'boot_id':boot", "'sequence':samples,'boot_id':'wrong'"),
                  C.WORKER.replace("'monotonic_after_ns':after", "'monotonic_after_ns':before-1"),
                  C.WORKER.replace("'unix_ns':wall", "'unix_ns':True")]
        for worker in variants:
            with self.subTest(worker=worker[-100:]):
                for path in Path(self.directory.name).glob('*'):path.unlink()
                channel=self.start(program(worker))
                with self.assertRaisesRegex(ValueError,'identity, sequence or order'):
                    channel.correlation()
                self.assertFalse(channel.close())  # rejected sample cannot authorize a shifted stop

    def test_prior_sample_order_is_checked_independently(self):
        channel=self.start()
        first=channel.correlation()
        channel.previous=first['monotonic_after_ns']+10**18
        with self.assertRaisesRegex(ValueError,'order changed'):channel.correlation()
        self.assertFalse(channel.close())

    def test_parent_sample_bound_does_not_send_an_extra_request(self):
        channel=self.start()
        with patch.object(C,'MAX_SAMPLES',2):
            channel.correlation();channel.correlation()
            with self.assertRaisesRegex(ValueError,'sample bound'):channel.correlation()
        self.assertTrue(channel.close())
        self.assertEqual(channel.record['stop_reply']['samples'],2)

    def test_real_worker_requires_explicit_stop_and_rejects_invalid_requests(self):
        cases=[('',3),('{"action":"sample","sequence":2}\n',1),
               ('{"action":"sample","sequence":true}\n',1),('x'*129+'\n',1),
               ('{"action":"other","sequence":1}\n',1)]
        for raw,expected in cases:
            with self.subTest(raw=raw[:60]):
                result=subprocess.run([sys.executable,'-I','-u','-c',program()],input=raw,
                    capture_output=True,text=True,timeout=5)
                self.assertEqual(result.returncode,expected,result.stderr)
        requests=''.join(json.dumps(dict(action='sample',sequence=i))+'\n' for i in range(1,8194))
        result=subprocess.run([sys.executable,'-I','-u','-c',program()],input=requests,
            capture_output=True,text=True,timeout=10)
        self.assertEqual(result.returncode,1)
        self.assertEqual(len(result.stdout.splitlines()),8193)  # ready + 8192, never the excess sample

    def test_reply_partial_disconnect_malformed_oversize_and_deadline(self):
        for raw,closed,expected in [(b'{',True,EOFError),(b'',True,EOFError),
                (b'[]\n',True,ValueError),(b'{bad}\n',True,ValueError),
                (b'x'*4097,True,ValueError),(b'{}\n{}\n',True,ValueError),
                (b'',False,TimeoutError)]:
            with self.subTest(raw=raw[:20],closed=closed):
                channel=C.ClockChannel(self.guest);read,write=os.pipe()
                with os.fdopen(read,'rb') as stream:
                    channel.process=types.SimpleNamespace(stdout=stream)
                    os.write(write,raw)
                    if closed:os.close(write)
                    try:
                        with self.assertRaises(expected):channel.reply(timeout=.02)
                    finally:
                        if not closed:os.close(write)

    def test_original_payload_failure_is_not_transport_success(self):
        channel=self.start(program(C.WORKER+'\nraise SystemExit(7)\n'))
        channel.correlation()
        self.assertFalse(channel.close())
        self.assertEqual(channel.record['exit'],7)
        self.assertTrue(channel.record['stop_confirmed'])

    def test_premature_exit_and_broken_pipe_keep_original_status_and_cleanup_receipt(self):
        worker=C.WORKER.replace("started=time.monotonic();samples=0", "raise SystemExit(124)\nstarted=time.monotonic();samples=0")
        channel=self.start(program(worker))
        self.assertEqual(channel.process.wait(timeout=5),124)
        self.assertFalse(channel.close())
        record=json.loads(channel.path.read_text())
        self.assertEqual(record['exit'],124)
        self.assertTrue(record['stop_confirmed'])
        self.assertEqual(record['samples'],0)
        self.assertIn('BrokenPipeError',record['error'])
        self.assertTrue(record['pipe_close_errors'])
        self.assertFalse(channel.close())

    def test_missing_shutdown_confirmation_refuses_clean_payload(self):
        channel=self.start()
        self.guest.script=lambda *a,**k:subprocess.CompletedProcess(a,0,'LoadState=loaded\nActiveState=active\n','')
        self.assertFalse(channel.close())
        self.assertEqual(channel.record['exit'],0)
        self.assertFalse(channel.record['stop_confirmed'])

    def test_existing_local_and_remote_guest_interface_owns_the_same_channel(self):
        self.assertIs(RemoteGuest.start_clock_channel,Guest.start_clock_channel)
        self.assertIs(RemoteGuest.stop_clock_channel,Guest.stop_clock_channel)
        self.assertIs(RemoteGuest.correlation,Guest.correlation)
        self.assertEqual(C.MAX_SAMPLES,8192)
        self.assertEqual(C.MAX_REPLY,4096)


if __name__=='__main__':unittest.main()
