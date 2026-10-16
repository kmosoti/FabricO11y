"""Private public-text AT-SPI -> Orca -> espeak environment probe, not acceptance."""
import argparse
import base64
import fcntl
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import struct
import subprocess
import sys
import time
import urllib.request

HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parent))
from resource_group import STORAGE as DATA, require_limits
from announcement_oracle import check_announcement,require

TEXT='Fabric public accessibility probe complete'


def append(path,row):
    raw=(json.dumps(row,separators=(',',':'))+'\n').encode()
    require(len(raw)<=4096 and (path.stat().st_size if path.exists() else 0)+len(raw)<=1048576,'announcement-stream-bound')
    with path.open('ab',buffering=0) as stream:stream.write(raw)


def read_rows(path):return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def check_stream(path,out):
    limit=4*1024**2 if path==out/'speech-logs/speech-dispatcher.log' else 1024**2
    require(path.stat().st_size<=limit,'announcement-stream-bound')


def loopback():
    with socket.socket() as listener:
        flags=struct.unpack('16sH14s',fcntl.ioctl(listener,0x8913,struct.pack('16sH14s',b'lo',0,b'')))[1]
        require(flags&1,'announcement-private-loopback')
        listener.bind(('127.0.0.1',0));listener.listen(1)
        with socket.create_connection(listener.getsockname(),timeout=2) as sender:
            receiver,_=listener.accept()
            with receiver:
                sender.sendall(b'AT-loopback-control')
                require(receiver.recv(64)==b'AT-loopback-control','announcement-private-loopback')
    return {'flags':flags,'up':True,'exact_delivery':True,'namespace':os.readlink('/proc/self/ns/net')}


def observe(path):
    import pyatspi
    def event(value):
        try:
            source=value.source
            text=source.queryText().getText(0,-1) if source.queryText() else source.name
            append(path,dict(type=value.type,pid=source.getApplication().get_process_id(),
                role=source.getRoleName(),attributes=source.getAttributes(),text=(text or '')[:1024],monotonic_ns=time.monotonic_ns()))
        except (NotImplementedError,AttributeError):pass
    pyatspi.Registry.registerEventListener(event,'object:text-changed','object:property-change',
        'object:children-changed','object:announcement')
    path.with_suffix('.ready').write_text(str(os.getpid()))
    pyatspi.Registry.start()


def customization(work,out):
    # Real Orca loads this supported private customization before launching speech.
    return f'''import json,os,time
from pathlib import Path
import speechd
original=speechd.SSIPClient.speak
marker=Path({str(work/'capture')!r});output=Path({str(out/'speech.jsonl')!r})
def observed(self,text,*args,**kwargs):
    before=time.monotonic_ns()
    result=original(self,text,*args,**kwargs)
    after=time.monotonic_ns()
    if marker.exists():
        row=dict(pid=os.getpid(),before_ns=before,after_ns=after,text=text,
            module=self.get_output_module(),ssip_code=result[0],message_id=int(result[2][0]))
        raw=(json.dumps(row,separators=(',',':'))+'\\n').encode()
        if len(raw)>4096 or (output.stat().st_size if output.exists() else 0)+len(raw)>1048576:raise RuntimeError('announcement-stream-bound')
        with output.open('ab',buffering=0) as stream:stream.write(raw)
    return result
speechd.SSIPClient.speak=observed
'''


def inside(args):
    work,out=args.work,args.output
    runtime=Path(os.environ['XDG_RUNTIME_DIR'])
    require((runtime.stat().st_dev,runtime.stat().st_ino)==((work/'runtime').stat().st_dev,(work/'runtime').stat().st_ino),'announcement-private-runtime')
    require(os.environ.get('DISPLAY')==(':87' if args.mode=='xvfb' else None),'announcement-private-display')
    require(not os.environ.get('WAYLAND_DISPLAY') and not os.environ.get('AT_SPI_BUS_ADDRESS'),'announcement-private-bus')
    processes=[];logs=[];end=time.monotonic()+150;session=None
    result={'exit':1,'scope':'public environment probe only; no package acceptance','mode':args.mode,
        'namespaces':{name:os.readlink('/proc/self/ns/'+name) for name in ('pid','net','mnt','ipc')},'commands':[]}
    def tick():
        require(time.monotonic()<end,'announcement-deadline')
        require(sum(path.stat().st_size for root in (work,out) for path in root.rglob('*') if path.is_file())<=512*1024**2,'announcement-storage-bound')
        for path in out.rglob('*'):
            if path.is_file() and path.suffix in ('.log','.jsonl','.raw'):check_stream(path,out)
    def launch(label,command):
        log=(out/(label+'.log')).open('xb');logs.append(log)
        process=subprocess.Popen(command,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
        processes.append(process);result['commands'].append({'label':label,'argv':command,'pid':process.pid});return process
    def wait(predicate):
        until=min(end,time.monotonic()+20)
        while time.monotonic()<until:
            tick()
            if predicate():return
            time.sleep(.05)
        raise RuntimeError('announcement-prerequisite-timeout')
    def run(command):
        process=launch('tool-'+str(len(processes)),command)
        wait(lambda:process.poll() is not None);require(process.returncode==0,'announcement-tool-exit')
    endpoint='http://127.0.0.1:48751'
    def call(method,path,body=None):
        tick();request=urllib.request.Request(endpoint+path,method=method,
            data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json'})
        with urllib.request.urlopen(request,timeout=min(10,max(.001,end-time.monotonic()))) as response:
            raw=response.read(65537);require(len(raw)<=65536,'announcement-driver-bound');return json.loads(raw)['value']
    def js(script):return call('POST','/session/'+session+'/execute/sync',{'script':script,'args':[]})
    try:
        result['loopback']=loopback()
        if args.mode=='xvfb':
            run(['/usr/bin/xauth','-f',str(work/'xauthority'),'add',':87','.',secrets.token_hex(16)])
            launch('xvfb',[str(args.xvfb),':87','-screen','0','1280x960x24','-nolisten','tcp','-auth',str(work/'xauthority')])
            wait(lambda:(Path('/tmp/.X11-unix/X87')).exists())
        config=work/'speech';config.mkdir();(config/'modules').mkdir()
        speech_logs=out/'speech-logs';speech_logs.mkdir()
        shutil.copyfile('/etc/speech-dispatcher/modules/espeak-ng.conf',config/'modules/espeak-ng.conf')
        (config/'speechd.conf').write_text(f'LogLevel 2\nLogDir "{speech_logs}"\nAddModule "espeak-ng" "sd_espeak-ng" "espeak-ng.conf"\nDefaultModule espeak-ng\nAudioOutputMethod "alsa"\nAudioALSADevice "fabric_capture"\n')
        (work/'asound.conf').write_text(f'pcm.null {{ type null }}\npcm.fabric_capture {{ type file slave.pcm "null" file "{out}/speech.raw" format "raw" }}\n')
        launch('speech-dispatcher-process',['/usr/bin/speech-dispatcher','-s','-C',str(config),'-S',str(runtime/'speech.sock'),'-P',str(work/'speech.pid')])
        wait(lambda:(runtime/'speech.sock').exists())
        prefs=work/'data/orca';prefs.mkdir(parents=True)
        (prefs/'orca-customizations.py').write_text(customization(work,out))
        for key,value in (('speech-server-factory','speechdispatcherfactory'),('synthesizer','espeak-ng')):
            run(['/usr/bin/gsettings','set','org.gnome.Orca.Speech:/org/gnome/orca/default/speech/',key,value])
        # The private session bus activates its own AT-SPI bus, never the owner's.
        run(['/usr/bin/gsettings','set','org.gnome.desktop.interface','toolkit-accessibility','true'])
        run(['/usr/bin/gdbus','call','--session','--dest','org.a11y.Bus','--object-path','/org/a11y/bus','--method','org.a11y.Bus.GetAddress'])
        launch('atspi',[sys.executable,'-B',str(Path(__file__)),'--observe',str(out/'events.jsonl')])
        wait(lambda:(out/'events.ready').exists())
        launch('orca',['/usr/bin/orca','--speech-system','speechdispatcherfactory','--debug-file',str(out/'orca-debug.log')])
        launch('webdriver',[str(DATA/'tools/ui/geckodriver'),'--host','127.0.0.1','--port','48751'])
        def ready():
            try:return call('GET','/status').get('ready') is True
            except OSError:return False
        wait(ready)
        profile=work/'profile';profile.mkdir()
        info=call('POST','/session',{'capabilities':{'alwaysMatch':{'browserName':'firefox','moz:firefoxOptions':{
            'binary':'/usr/bin/firefox','args':(['-headless'] if args.mode=='headless' else [])+['-profile',str(profile)],
            'prefs':{'accessibility.force_disabled':0,'security.enterprise_roots.enabled':False}}}}})
        session=info['sessionId'];browser_pid=info['capabilities']['moz:processID']
        result['browser_capabilities']=info['capabilities']
        html='<title>Fabric public AT probe</title><h1>Public accessibility probe</h1><button id="focus">Keep focus here</button><p id="status" role="status" aria-live="polite"></p>'
        call('POST','/session/'+session+'/url',{'url':'data:text/html;base64,'+base64.b64encode(html.encode()).decode()})
        prerequisite=js('const e=document.querySelector("#status");document.querySelector("#focus").focus();return {role:e.getAttribute("role"),live:e.getAttribute("aria-live"),focus_elsewhere:document.activeElement.id==="focus"}')
        require(prerequisite=={'role':'status','live':'polite','focus_elsewhere':True},'announcement-dom')
        backend=None
        def find_backend():
            nonlocal backend
            for proc in Path('/proc').iterdir():
                if not proc.name.isdigit():continue
                try:
                    if (proc/'exe').resolve()==Path('/usr/lib64/speech-dispatcher-modules/sd_espeak-ng'):
                        backend={'pid':int(proc.name),'module':'espeak-ng','libespeak_mapped':'libespeak-ng.so' in (proc/'maps').read_text()};return True
                except (FileNotFoundError,PermissionError):pass
            return False
        wait(find_backend)
        pcm=out/'speech.raw';backend['pcm_before_bytes']=pcm.stat().st_size if pcm.exists() else 0
        if args.suppress_live:js('document.querySelector("#status").setAttribute("aria-live","off")')
        (work/'capture').touch();case=dict(prerequisite,browser_pid=browser_pid,text=TEXT,before_ns=time.monotonic_ns())
        js('document.querySelector("#status").textContent='+json.dumps(TEXT))
        until=min(end,time.monotonic()+8)
        while time.monotonic()<until:tick();time.sleep(.05)
        case['after_ns']=time.monotonic_ns();(work/'capture').unlink()
        backend['pcm_after_bytes']=pcm.stat().st_size if pcm.exists() else 0
        result.update(case=case,backend=backend,events=read_rows(out/'events.jsonl'),speech=read_rows(out/'speech.jsonl'))
        check_announcement(case,result['events'],result['speech'],backend)
        result['exit']=0
    except BaseException as error:result['error']=type(error).__name__+': '+str(error)
    finally:
        if session:
            try:call('DELETE','/session/'+session)
            except BaseException:pass
        for process in reversed(processes):
            try:os.killpg(process.pid,signal.SIGTERM)
            except ProcessLookupError:pass
        for process in reversed(processes):
            try:process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                try:os.killpg(process.pid,signal.SIGKILL)
                except ProcessLookupError:pass
                process.wait(timeout=2)
        for log in logs:log.close()
        result['process_exits']=[process.returncode for process in processes]
        (out/'inside.json').write_text(json.dumps(result,indent=2)+'\n')
    return result['exit']


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--observe',type=Path);parser.add_argument('--inside',action='store_true')
    parser.add_argument('--mode',choices=('headless','xvfb'),default='headless');parser.add_argument('--xvfb',type=Path)
    parser.add_argument('--xvfb-receipt',type=Path);parser.add_argument('--output',type=Path);parser.add_argument('--work',type=Path)
    parser.add_argument('--suppress-live',action='store_true');args=parser.parse_args()
    if args.observe:observe(args.observe);return 0
    if args.inside:return inside(args)
    group=require_limits();out=args.output
    require(out and out.is_absolute() and out.is_relative_to(DATA/'results') and not out.exists(),'announcement-output')
    scratch=Path(os.environ['FABRIC_SCRATCH_ROOT']);require(scratch.is_relative_to(DATA),'announcement-scratch')
    work=scratch/('announcement-'+out.name);work.mkdir(mode=0o700);out.mkdir(mode=0o700)
    for name in ('tmp','runtime','config','cache','data'):(work/name).mkdir(mode=0o700)
    subprocess.run(['systemctl','--user','set-property','--runtime',group.name,'MemoryHigh=1G','MemoryMax=2G','MemorySwapMax=0','CPUQuota=200%','TasksMax=256'],check=True,capture_output=True)
    require({key:(group/key).read_text().strip() for key in ('memory.high','memory.max','memory.swap.max','cpu.max','pids.max')}=={
        'memory.high':str(2**30),'memory.max':str(2**31),'memory.swap.max':'0','cpu.max':'200000 100000','pids.max':'256'},'announcement-limits')
    if args.mode=='xvfb':
        require(args.xvfb_receipt is not None,'announcement-xvfb-admission')
        prepared=json.loads(args.xvfb_receipt.read_text());args.xvfb=Path(prepared['binary'])
        require(prepared.get('state')=='prepared' and prepared.get('exit')==0
            and args.xvfb.is_relative_to(DATA) and hashlib.sha256(args.xvfb.read_bytes()).hexdigest()==prepared['binary_sha256'],'announcement-xvfb-admission')
    command=['/usr/bin/bwrap','--die-with-parent','--unshare-user','--unshare-pid','--unshare-net','--cap-add','CAP_NET_ADMIN','--unshare-ipc','--unshare-uts',
        '--ro-bind','/','/','--proc','/proc','--dev','/dev','--bind',str(work),str(work),'--bind',str(out),str(out),
        '--bind',str(work/'tmp'),'/tmp','--bind',str(work/'runtime'),f'/run/user/{os.getuid()}','--clearenv']
    environment={'PATH':'/usr/bin:/usr/sbin:/bin','LANG':'C.UTF-8','TMPDIR':str(work/'tmp'),
        'FABRIC_STORAGE_ROOT':str(DATA),'FABRIC_SCRATCH_ROOT':str(scratch),
        'XDG_RUNTIME_DIR':f'/run/user/{os.getuid()}','XDG_CONFIG_HOME':str(work/'config'),'XDG_CACHE_HOME':str(work/'cache'),
        'XDG_DATA_HOME':str(work/'data'),'GSETTINGS_BACKEND':'keyfile','SPEECHD_ADDRESS':f'unix_socket:/run/user/{os.getuid()}/speech.sock',
        'SPEECHD_CMD':'/bin/false','ALSA_CONFIG_PATH':str(work/'asound.conf'),'GTK_MODULES':'atk-bridge','NO_AT_BRIDGE':'0',
        'DBUS_SYSTEM_BUS_ADDRESS':'unix:path='+str(work/'absent-system-bus')}
    if args.mode=='xvfb':environment.update(DISPLAY=':87',XAUTHORITY=str(work/'xauthority'))
    for key,value in environment.items():command.extend(['--setenv',key,value])
    command+=['/usr/bin/dbus-run-session','--',sys.executable,'-B',str(Path(__file__)),'--inside','--output',str(out),'--work',str(work),'--mode',args.mode]
    if args.xvfb:command+=['--xvfb',str(args.xvfb)]
    if args.suppress_live:command+=['--suppress-live']
    tools=[Path(name) for name in ('/usr/bin/bwrap','/usr/bin/dbus-run-session','/usr/bin/orca',
        '/usr/bin/firefox','/usr/bin/speech-dispatcher','/usr/libexec/at-spi-bus-launcher',
        '/usr/lib64/speech-dispatcher-modules/sd_espeak-ng','/usr/sbin/ip','/usr/bin/xauth')]
    tools.append(DATA/'tools/ui/geckodriver')
    if args.xvfb:tools.append(args.xvfb)
    record={'scope':'public AT environment only; no package acceptance','command':command,'exit':1,
        'source_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'tool_sha256':{str(tool):hashlib.sha256(tool.read_bytes()).hexdigest() for tool in tools},
        'namespaces_before':{name:os.readlink('/proc/self/ns/'+name) for name in ('pid','net','mnt','ipc')}}
    process=None
    try:
        with (out/'namespace.log').open('xb') as log:
            process=subprocess.Popen(command,stdout=log,stderr=subprocess.STDOUT,start_new_session=True);end=time.monotonic()+180
            while process.poll() is None:
                require(time.monotonic()<end,'announcement-deadline');require((out/'namespace.log').stat().st_size<=1048576,'announcement-stream-bound');time.sleep(.05)
            record['exit']=process.returncode
    except BaseException as error:record['error']=type(error).__name__+': '+str(error)
    finally:
        if process and process.poll() is None:
            os.killpg(process.pid,signal.SIGKILL);process.wait(timeout=5)
        record['namespace_leader_reaped']=process is not None and process.poll() is not None
        if (out/'inside.json').exists():
            inner=json.loads((out/'inside.json').read_text())
            isolated=all(inner['namespaces'][name]!=value for name,value in record['namespaces_before'].items())
            record['actual_private_namespaces']=isolated
            if not isolated:record.update(exit=1,error='announcement-namespace-isolation')
        record['resources']={key:(group/key).read_text().strip() for key in ('memory.peak','memory.events','memory.swap.current')}
        if record['exit']==0:shutil.rmtree(work);record['cleanup']='owned private configuration/profile/runtime removed'
        else:record['failure_evidence']=str(work)
        (out/'receipt.json').write_text(json.dumps(record,indent=2)+'\n')
    return record['exit']


if __name__=='__main__':raise SystemExit(main())
