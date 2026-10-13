"""Disposable target-only clock fixture and contained native binding admission."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import platform
import select
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import session_clock_oracle as oracle

HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parent))
from resource_group import STORAGE, require_limits


def digest(path):
    with Path(path).open('rb') as stream:return hashlib.file_digest(stream,'sha256').hexdigest()


class KernelTime(ctypes.Structure):_fields_=[('sec',ctypes.c_long),('nsec',ctypes.c_long)]
def kernel_time():
    value=KernelTime();oracle.require(ctypes.CDLL(None,use_errno=True).syscall(228,0,ctypes.byref(value))==0,'fixture-raw-clock');return value.sec*1000000000+value.nsec


def freeze_images(executable,helper):
    frozen={}
    for name,path in (('executable',executable),('helper',helper)):
        before=path.stat();frozen[name+'_sha256']=digest(path);after=path.stat()
        oracle.require((before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns,before.st_ctime_ns)==(after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns,after.st_ctime_ns),'isolation-'+('target' if name=='executable' else 'helper')+'-frozen')
        frozen[name+'_identity']={'device':before.st_dev,'inode':before.st_ino}
    return frozen


def process_binding(pid, executable, helper, intercepted=True, *, expected=None):
    proc=Path(f'/proc/{pid}');actual=proc/'exe'
    if expected is not None:
        for name,paths,message in (('executable',(actual,executable),'isolation-target-frozen'),('helper',(helper,),'isolation-helper-frozen')):
            for path in paths:
                stat=path.stat()
                oracle.require({'device':stat.st_dev,'inode':stat.st_ino}==expected[name+'_identity'] and digest(path)==expected[name+'_sha256'],message)
    oracle.require(actual.resolve()==executable.resolve() and digest(actual)==digest(executable),'isolation-target')
    rows=[line.split(maxsplit=5) for line in (proc/'maps').read_text().splitlines()]
    mappings={(row[3],row[4],row[5]) for row in rows if len(row)==6 and row[5]==str(helper)};stat=helper.stat()
    if expected is not None and mappings:
        frozen=expected['helper_identity']
        oracle.require(mappings=={(f'{os.major(frozen["device"]):02x}:{os.minor(frozen["device"]):02x}',str(frozen['inode']),str(helper))},'isolation-helper-frozen')
    oracle.require(mappings==({(f'{os.major(stat.st_dev):02x}:{os.minor(stat.st_dev):02x}',str(stat.st_ino),str(helper))} if intercepted else set()),'isolation-interception')
    libc=next(Path(row[5]) for row in rows if len(row)==6 and row[5].endswith('/libc.so.6'))
    return {'pid':pid,'executable_sha256':digest(actual),'helper_sha256':digest(helper) if mappings else None,'libc':str(libc),'libc_sha256':digest(libc),
            'start_ticks':int((proc/'stat').read_text().rsplit(')',1)[1].split()[19]),'cgroup':(proc/'cgroup').read_text(),'helper_maps':sorted(mappings)}


def stop_group(process):
    try:os.killpg(process.pid,signal.SIGTERM)
    except ProcessLookupError:process.wait(timeout=3);return
    try:process.wait(timeout=3)
    except subprocess.TimeoutExpired:pass
    try:os.killpg(process.pid,signal.SIGKILL)
    except ProcessLookupError:pass
    process.wait(timeout=3);deadline=time.monotonic()+3
    while True:
        try:
            while os.waitpid(-process.pid,os.WNOHANG)[0]:pass
        except ChildProcessError:pass
        try:os.killpg(process.pid,0)
        except ProcessLookupError:return
        oracle.require(time.monotonic()<deadline,'fixture-process-group-stop');time.sleep(.01)


class SessionClock:
    def __init__(self, executable, helper, directory, deadline, initial=None, socket_type=socket.SOCK_SEQPACKET, preload=True, invalid_fd=False):
        self.deadline=deadline;self.frames=[];self.rows=[];self.bytes=0;self.error=None;self.buffer=b''
        env=dict(os.environ);oracle.require(not any(name in env for name in ('LD_PRELOAD','LD_AUDIT','FABRIC_CLOCK_FD')),'fixture-environment')
        self.parent,self.child=socket.socketpair(socket.AF_UNIX,socket_type|socket.SOCK_NONBLOCK|socket.SOCK_CLOEXEC)
        self.fd=self.child.fileno();self.socket_identity=os.readlink(f'/proc/self/fd/{self.fd}')
        self.parent.send(initial if initial is not None else self.frame(0));self.stderr=directory.open('xb')
        if preload:env.update(LD_PRELOAD=str(helper),FABRIC_CLOCK_FD=str(1048575 if invalid_fd else self.fd))
        self.argv=[str(executable),'interactive',str(self.fd),str(helper),self.socket_identity]
        try:self.process=subprocess.Popen(self.argv,env=env,pass_fds=(self.fd,),stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=self.stderr,start_new_session=True)
        except BaseException:self.parent.close();self.child.close();self.stderr.close();raise
        self.child.close();self.receiver=threading.Thread(target=self.receive,daemon=True);self.receiver.start()
    def frame(self,offset):
        frame=b'FCLKv1\0\0'+len(self.frames).to_bytes(8,'little')+offset.to_bytes(8,'little')
        oracle.check_frames(self.frames+[frame]);self.frames.append(frame);return frame
    def advance(self,offset):oracle.require(self.parent.send(self.frame(offset))==24,'fixture-control-send')
    def receive(self):
        try:
            while True:
                if not select.select([self.parent],[],[],.1)[0]:continue
                raw=self.parent.recv(257)
                if not raw:return
                self.bytes+=len(raw);oracle.require(len(raw)<=256 and self.bytes<=1048576,'fixture-ledger-bound')
                self.rows.append(json.loads(raw))
        except BaseException as error:
            self.error=type(error).__name__+': '+str(error)
            try:os.killpg(self.process.pid,signal.SIGTERM)
            except ProcessLookupError:pass
    def line(self):
        while b'\n' not in self.buffer:
            oracle.require(self.error is None,'fixture-ledger-error')
            remaining=self.deadline-time.monotonic();oracle.require(remaining>0,'fixture-deadline')
            oracle.require(select.select([self.process.stdout],[],[],remaining)[0],'fixture-deadline')
            raw=os.read(self.process.stdout.fileno(),4096);oracle.require(bool(raw),'fixture-probe-eof')
            self.buffer+=raw;oracle.require(len(self.buffer)<=65536,'fixture-probe-bound')
        raw,self.buffer=self.buffer.split(b'\n',1);return json.loads(raw)
    def command(self,name):self.process.stdin.write((name+'\n').encode());self.process.stdin.flush()
    def close(self):
        try:stop_group(self.process)
        finally:
            self.receiver.join(timeout=1);self.parent.close();self.process.stdin.close();self.process.stdout.close();self.stderr.close()
        oracle.require(not self.receiver.is_alive(),'fixture-receiver-stop')
        try:os.killpg(self.process.pid,0)
        except ProcessLookupError:return
        raise ValueError('fixture-process-group-stop')


def native_admission(executable,helper,output,deadline,report):
    target=SessionClock(executable,helper,output/'positive.stderr',deadline);report['clocks']=[];report['system_time']=[]
    try:
        ready=target.line();report['ready']=ready
        oracle.require(ready=={'kind':'ready','pid':target.process.pid,'env_clear':True,'cloexec':True,'socket_type':5} and os.readlink(f'/proc/{target.process.pid}/fd/{target.fd}')==target.socket_identity and int(Path(f'/proc/{target.process.pid}/fdinfo/{target.fd}').read_text().split('flags:',1)[1].split()[0],8)&os.O_CLOEXEC,'isolation-control')
        report['target']=process_binding(target.process.pid,executable,helper,expected=report['frozen_images']);report['controller']=process_binding(os.getpid(),Path(sys.executable),helper,False);report['host_clocks']=[]
        report['control_fd']={'identity':target.socket_identity,'fdinfo':Path(f'/proc/{target.process.pid}/fdinfo/{target.fd}').read_text()};report['probe_argv']=target.argv
        for sequence,offset in enumerate((0,1,1799,1800,28799,28800)):
            if sequence:target.advance(offset)
            if sequence==2:
                target.command('fork');fork,child,done=(target.line() for _ in range(3));report['fork']=[fork,child,done]
                row=fork['row'];oracle.require(row['pid']==child['pid']==done['fork_pid']!=target.process.pid and done['status']==0,'isolation-fork-pid')
                oracle.require(row['offset_s']==1 and row['sequence']==done['sequence_before']==1 and row['returned_ns']==row['raw_ns'],'isolation-fork-consumption')
                oracle.require(row['syscall_before_ns']<=row['raw_ns']<=row['syscall_after_ns'] and row['syscall_after_ns']-row['syscall_before_ns']<=1000000000,'isolation-host-clock')
                oracle.require(child['kind']=='exec' and child['env_clear'] and not child['helper_mapped'] and not child['control_fd_present'],'isolation-exec-escape')
                oracle.require(child['syscall_before_ns']<=child['clock_ns']<=child['syscall_after_ns'] and child['syscall_after_ns']-child['syscall_before_ns']<=1000000000,'isolation-host-clock')
            target.command('clock')
            for _ in range(3):
                value=target.line();oracle.require(value['kind']=='clock','clock-binding');report['clocks'].append(value['row'])
            system=target.line();oracle.require(system['kind']=='system','clock-binding');report['system_time'].append(system['row'])
            before=kernel_time();clock=time.time_ns();after=kernel_time();oracle.require(before<=clock<=after and after-before<=1000000000 and (not report['host_clocks'] or before>=report['host_clocks'][-1]['syscall_after_ns']),'isolation-host-clock');report['host_clocks'].append({'sequence':sequence,'syscall_before_ns':before,'clock_ns':clock,'syscall_after_ns':after})
        target.command('sleep');sleep=target.line();oracle.require(sleep['kind']=='sleep','sleep-fields');report['sleep']=sleep['row']
        oracle.check_frames(target.frames);oracle.check_probe_clocks(report['clocks'],target.process.pid);oracle.check_sleep(report['sleep'])
        for row,clock in zip(report['system_time'],report['clocks'][::3]):
            oracle.require((row['pid'],row['sequence'],row['offset_s'],row['clock_id'])==(clock['pid'],clock['sequence'],clock['offset_s'],0),'clock-binding')
            oracle.require(row['syscall_before_ns']<=row['raw_ns']<=row['syscall_after_ns'] and row['syscall_after_ns']-row['syscall_before_ns']<=1000000000,'clock-raw-bracket')
            oracle.require(row['returned_ns']==row['raw_ns']+row['offset_s']*1000000000,'clock-realtime-only')
        target.command('quit');oracle.require(target.process.wait(timeout=max(.001,deadline-time.monotonic()))==0,'fixture-probe-exit')
    finally:
        target.close();report['ledger']=target.rows;report['frames']=[frame.hex() for frame in target.frames]
    oracle.require(target.error is None,'fixture-ledger-error')
    for row in report['clocks']+report['system_time']:
        oracle.require({key:value for key,value in row.items() if not key.startswith('syscall_')} in report['ledger'],'clock-ledger-binding')
    report['controls']=[]
    initial=b'FCLKv1\0\0'+bytes(16)
    controls=[('short',initial[:-1],'FCLK-FRAME'),('oversize',initial+b'x','FCLK-FRAME'),('version',b'bad-v001'+bytes(16),'FCLK-FRAME'),
              ('initial-sequence',initial[:8]+(1).to_bytes(8,'little')+bytes(8),'FCLK-SEQUENCE'),('initial-offset',initial[:16]+(1).to_bytes(8,'little'),'FCLK-OFFSET'),
              ('skip',None,'FCLK-SEQUENCE'),('duplicate',None,'FCLK-SEQUENCE'),('decrease',None,'FCLK-OFFSET'),('cap',None,'FCLK-OFFSET'),('overflow',None,'FCLK-OFFSET'),
              ('count',None,'FCLK-COUNT'),('death',None,'FCLK-EOF'),('closed',None,'FCLK-RECV'),('descriptor',None,'FCLK-FD'),('type',None,'FCLK-TYPE'),('unpreloaded',None,'isolation-interception')]
    for name,packet,expected in controls:
        error_path=output/(name+'.stderr');current=SessionClock(executable,helper,error_path,deadline,initial=packet,socket_type=socket.SOCK_STREAM if name=='type' else socket.SOCK_SEQPACKET,preload=name!='unpreloaded',invalid_fd=name=='descriptor')
        try:
            if name=='unpreloaded':
                oracle.require(current.line()=={'kind':'absent','pid':current.process.pid},'fixture-unpreloaded')
                try:process_binding(current.process.pid,executable,helper,expected=report['frozen_images'])
                except ValueError as error:oracle.require(str(error)==expected,'fixture-wrong-refusal')
                else:raise ValueError('fixture-unpreloaded-admitted')
            else:
                if packet is None and name not in ('type','descriptor'):
                    current.line()
                    if name=='death':current.parent.shutdown(socket.SHUT_WR)
                    elif name=='decrease':current.parent.send(initial[:8]+(1).to_bytes(8,'little')+(1).to_bytes(8,'little'));current.parent.send(initial[:8]+(2).to_bytes(8,'little')+bytes(8))
                    else:
                        values=[(i,0) for i in range(1,65)] if name=='count' else [(2 if name=='skip' else 0 if name=='duplicate' else 1,28861 if name=='cap' else 2**64-1 if name=='overflow' else 0)]
                        if name!='closed':
                            for seq,value in values:current.parent.send(initial[:8]+seq.to_bytes(8,'little')+value.to_bytes(8,'little'))
                    current.command('close' if name=='closed' else 'clock')
                oracle.require(current.process.wait(timeout=max(.001,deadline-time.monotonic()))==78,'fixture-control-exit')
        finally:current.close()
        stderr=error_path.read_text();oracle.require(name=='unpreloaded' or stderr.strip()==expected,'fixture-wrong-refusal')
        report['controls'].append({'case':name,'refusal':expected,'exit':current.process.returncode,'stderr':stderr,'ledger':current.rows})


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output',type=Path,required=True);parser.add_argument('--rustc',required=True);parser.add_argument('--cc',default='gcc');args=parser.parse_args()
    require_limits();oracle.require(platform.system()=='Linux' and platform.machine()=='x86_64','fixture-platform')
    output=args.output;oracle.require(output.is_absolute() and output.resolve()==output and output.is_relative_to(STORAGE),'fixture-output');output.mkdir(mode=0o700)
    scratch=Path(os.environ['FABRIC_SCRATCH_ROOT']);oracle.require(scratch.is_dir() and scratch.is_relative_to(STORAGE),'fixture-scratch')
    directory=Path(tempfile.mkdtemp(prefix='session-clock-',dir=scratch));deadline=time.monotonic()+240;report={'scope':'native binding only; actual packaged browser session checks pending','state':'failed','exit':1,'commands':[]}
    def run(command):
        report['commands'].append(command);path=output/('tool-'+str(len(report['commands']))+'.log');timeout=max(.001,deadline-time.monotonic());failure=None
        with path.open('xb') as stream:
            process=subprocess.Popen(command,stdout=stream,stderr=subprocess.STDOUT,start_new_session=True)
            try:
                while process.poll() is None:
                    if time.monotonic()>=deadline:raise subprocess.TimeoutExpired(command,timeout)
                    oracle.require(path.stat().st_size<=1048576,'fixture-tool-output');time.sleep(.01)
            except BaseException as error:failure=error;raise
            finally:
                try:stop_group(process)
                except BaseException as error:
                    report['tool_cleanup_error']=type(error).__name__+': '+str(error)
                    if failure is None:raise
                finally:
                    with path.open('rb') as captured:raw=captured.read(1048576)
                    report.setdefault('tool_outputs',[]).append({'command':command,'exit':process.returncode,'raw_log':str(path),'output':raw.decode(errors='replace')})
                    if isinstance(failure,subprocess.TimeoutExpired):failure.output=raw
        oracle.require(path.stat().st_size<=1048576,'fixture-tool-output');oracle.require(process.returncode==0,'fixture-tool-exit');return raw.decode()
    try:
        compiler=Path(shutil.which(args.cc)).resolve();rustc=Path(shutil.which(args.rustc)).resolve();version=run([str(rustc),'-vV'])
        oracle.require('release: 1.99.0' in version and 'commit-hash: b940084d7' in version,'fixture-toolchain')
        run([str(compiler),'--version']);helper=directory/'session-clock.so';executable=directory/'session-clock-probe'
        run([str(compiler),'-shared','-fPIC','-pthread','-O2','-Wall','-Wextra','-Werror','-o',str(helper),str(HERE/'session_clock.c'),'-ldl'])
        run([str(rustc),'--edition=2021','-C','opt-level=1','-o',str(executable),str(HERE/'session_clock_probe.rs')])
        dynamic=run(['readelf','--dyn-syms','--wide',str(executable)]);oracle.require('clock_gettime@' in dynamic,'fixture-dynamic-clock-import')
        interpreter=run(['readelf','-lW',str(executable)]).split('Requesting program interpreter: ',1)[1].split(']',1)[0]
        report['frozen_images']=freeze_images(executable,helper)
        report['hashes']={str(path):digest(path) for path in (compiler,rustc,helper,executable,Path(interpreter),HERE/'session_clock.py',HERE/'session_clock.c',HERE/'session_clock_probe.rs')}
        oracle.require(all(report['hashes'][str(path)]==report['frozen_images'][name+'_sha256'] for name,path in (('executable',executable),('helper',helper))),'fixture-image-hash')
        oracle.require(sum(path.stat().st_size for path in directory.rglob('*') if path.is_file())<=512*1024**2,'fixture-storage-bound')
        native_admission(executable,helper,output,deadline,report)
        report['build_id_notes']={str(path):run(['readelf','-nW',str(path)]) for path in (helper,executable,Path(interpreter),Path(report['target']['libc']))};report.update(state='completed',exit=0)
    except BaseException as error:report['error']=type(error).__name__+': '+str(error);raise
    finally:
        try:
            if report['state']=='completed':shutil.rmtree(directory)
            elif 'tool_cleanup_error' in report:report['failure_evidence']=str(directory)
            else:
                retained=output/'failed-fixture';shutil.move(str(directory),str(retained));report['failure_evidence']=str(retained)
        except BaseException as error:
            report['cleanup_error']=type(error).__name__+': '+str(error);report['failure_evidence']=str(directory)
            if 'error' not in report:report.update(state='failed',exit=1);raise
        finally:
            report['cleanup']={'owned_fixture':str(directory),'absent':not directory.exists(),'failure_evidence':report.get('failure_evidence')}
            (output/'receipt.json').write_text(json.dumps(report,indent=2)+'\n');print(str(output/'receipt.json'))
    return report['exit']


if __name__=='__main__':raise SystemExit(main())
