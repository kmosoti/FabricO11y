"""Owned raw guest-clock requests over the existing SSH management path."""
import hashlib
import json
import os
from pathlib import Path
import select
import shlex
import subprocess
import threading
import time

MAX_SAMPLES = 8192
MAX_REPLY = 4096
WORKER = r'''
import json,sys,time,subprocess
from pathlib import Path
unit=sys.argv[1]
group=Path('/proc/self/cgroup').read_text().strip().split(':',2)[2]
assert group.endswith('/'+unit)
root=Path('/sys/fs/cgroup'+group)
limits={name:(root/name).read_text().strip() for name in ('memory.max','memory.swap.max','cpu.max','pids.max')}
assert limits=={'memory.max':'33554432','memory.swap.max':'0','cpu.max':'25000 100000','pids.max':'8'},limits
runtime=subprocess.check_output(['systemctl','show',unit,'--value','--property=RuntimeMaxUSec'],text=True).strip()
assert runtime=='10min',runtime
boot=Path('/proc/sys/kernel/random/boot_id').read_text().strip()
def emit(value):print(json.dumps(value,separators=(',',':')),flush=True)
emit({'event':'ready','boot_id':boot,'limits':limits,'runtime':runtime,'sequence':0})
started=time.monotonic();samples=0
while True:
 line=sys.stdin.buffer.readline(129)
 if not line:raise SystemExit(3)
 assert len(line)<=128 and line.endswith(b'\n')
 request=json.loads(line)
 assert set(request)=={'action','sequence'} and type(request['sequence']) is int and request['sequence']==samples+1
 if request['action']=='stop':
  emit({'event':'stopped','sequence':samples+1,'samples':samples});break
 assert request['action']=='sample' and samples<8192 and time.monotonic()-started<600
 before=time.monotonic_ns();wall=time.time_ns();after=time.monotonic_ns()
 samples+=1
 emit({'event':'sample','sequence':samples,'boot_id':boot,'monotonic_before_ns':before,'unix_ns':wall,'monotonic_after_ns':after})
'''


class ClockChannel:
    def __init__(self, guest):
        if guest.role not in ('edge', 'server'):
            raise ValueError('unregistered clock guest role')
        self.guest, self.unit = guest, 'cross-clock-' + guest.role + '.service'
        self.process, self.buffer = None, b''
        self.samples, self.previous = 0, None
        self.lock = threading.Lock()
        self.closed, self.active = False, False
        self.record = dict(unit=self.unit, samples=0, exit=None, stop_confirmed=False,
                           worker_sha256=hashlib.sha256(WORKER.encode()).hexdigest())
        self.path = guest.results / (guest.role + '-clock-channel.json')

    def save(self):
        self.record['samples'] = self.samples
        self.path.write_text(json.dumps(self.record, indent=2) + '\n')

    def reply(self, timeout=5):
        deadline = time.monotonic() + timeout
        while b'\n' not in self.buffer:
            if len(self.buffer) > MAX_REPLY:
                raise ValueError('clock reply exceeds byte bound')
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([self.process.stdout], [], [], remaining)[0]:
                raise TimeoutError('clock reply deadline')
            block = os.read(self.process.stdout.fileno(), MAX_REPLY + 1)
            if not block:
                raise EOFError('clock channel disconnected or partial reply')
            self.buffer += block
        raw, self.buffer = self.buffer.split(b'\n', 1)
        if len(raw) + 1 > MAX_REPLY or self.buffer:
            raise ValueError('clock reply exceeds byte bound or contains unsolicited data')
        value = json.loads(raw)
        if not isinstance(value, dict):
            raise ValueError('clock reply is not an object')
        return value

    def send(self, action):
        self.process.stdin.write((json.dumps(dict(action=action, sequence=self.samples+1))+'\n').encode())
        self.process.stdin.flush()

    def start(self):
        payload = ['sudo', '-n', 'systemd-run', '--quiet', '--pipe', '--wait', '--collect',
                   '--unit=' + self.unit, '--property=MemoryMax=32M', '--property=MemorySwapMax=0',
                   '--property=CPUQuota=25%', '--property=CPUQuotaPeriodSec=100ms',
                   '--property=TasksMax=8', '--property=RuntimeMaxSec=600',
                   '/usr/bin/python3', '-u', '-c', WORKER, self.unit]
        command = [*self.guest.ssh, shlex.join(payload)]
        self.record['command'] = command
        self.save()
        fd = os.open(self.guest.results / (self.guest.role+'-clock-stderr.txt'),
                     os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        try:
            with os.fdopen(fd, 'wb') as errors:
                self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors)
            ready = self.reply(15)
            if (set(ready) != {'event','boot_id','limits','runtime','sequence'}
                    or ready['event'] != 'ready' or type(ready['sequence']) is not int or ready['sequence'] != 0
                    or ready['boot_id'] != self.guest.boot_id or ready['runtime'] != '10min'
                    or ready['limits'] != {'memory.max':'33554432','memory.swap.max':'0',
                                           'cpu.max':'25000 100000','pids.max':'8'}):
                raise ValueError('clock startup identity or enforced limits mismatch')
            self.record['startup'] = ready
            self.active = True
            self.save()
        except BaseException as error:
            self.record['startup_error'] = type(error).__name__ + ': ' + str(error)
            self.close(abort=True)
            raise

    def correlation(self):
        with self.lock:
            if not self.active or self.closed or self.samples >= MAX_SAMPLES:
                raise ValueError('clock channel inactive or sample bound reached')
            before = time.monotonic_ns()
            self.send('sample')
            value = self.reply()
            after = time.monotonic_ns()
            fields = {'event','sequence','boot_id','monotonic_before_ns','unix_ns','monotonic_after_ns'}
            if (set(value) != fields or value['event'] != 'sample'
                    or type(value['sequence']) is not int or value['sequence'] != self.samples+1
                    or value['boot_id'] != self.guest.boot_id
                    or any(type(value[k]) is not int or value[k] < 0 for k in
                           ('monotonic_before_ns','unix_ns','monotonic_after_ns'))
                    or value['monotonic_after_ns'] < value['monotonic_before_ns']
                    or (self.previous is not None and value['monotonic_before_ns'] < self.previous)):
                raise ValueError('clock sample identity, sequence or order changed')
            self.samples += 1
            self.previous = value['monotonic_after_ns']
            return {k:v for k,v in value.items() if k not in ('event','sequence')} | dict(
                host_request_before_ns=before, host_request_after_ns=after, ssh_roundtrip_ns=after-before)

    def close(self, abort=False):
        with self.lock:
            if self.closed:
                return self.record['stop_confirmed'] and self.record['exit'] == 0 and not self.record.get('error')
            self.closed, self.active = True, False
            try:
                if abort:
                    raise RuntimeError('clock channel aborted before admission')
                self.send('stop')
                stopped = self.reply()
                if stopped != dict(event='stopped', sequence=self.samples+1, samples=self.samples):
                    raise ValueError('clock stop receipt mismatch')
                self.record['stop_reply'] = stopped
                self.record['exit'] = self.process.wait(timeout=10)
                if self.record['exit'] != 0:
                    raise RuntimeError('original clock command failed')
            except BaseException as error:
                self.record['error'] = type(error).__name__ + ': ' + str(error)
            finally:
                if self.process is not None:
                    if self.process.poll() is None:
                        self.process.terminate()
                        try:
                            self.process.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            self.process.kill()
                            self.process.wait(timeout=5)
                    self.record['exit'] = self.process.returncode
                    self.process.stdin.close()
                    self.process.stdout.close()
                try:
                    if self.record.get('error'):
                        stop = self.guest.script('systemctl stop '+self.unit+'\n', timeout=15, check=False)
                        self.record['fallback_stop'] = dict(exit=stop.returncode, stdout=stop.stdout, stderr=stop.stderr)
                    result = self.guest.script('systemctl show '+self.unit+' -p LoadState -p ActiveState\n', timeout=15, check=False)
                    self.record['unit_check'] = dict(exit=result.returncode, stdout=result.stdout, stderr=result.stderr)
                    self.record['stop_confirmed'] = (result.returncode in (0, 1) and
                        set(result.stdout.splitlines()) == {'LoadState=not-found','ActiveState=inactive'})
                except BaseException as error:
                    self.record['unit_check_error'] = type(error).__name__ + ': ' + str(error)
                self.save()
            return self.record['stop_confirmed'] and self.record['exit'] == 0 and not self.record.get('error')
