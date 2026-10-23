"""Actual console AT caller; effects stay in the existing owned browser cgroup."""
import hashlib
import json
from pathlib import Path
import re
import sys
import time
from announcement_oracle import check_announcement,require,spoken_text
from announcement_contract import check_alert,grade_targets
from announcement_probe import append,read_rows

MODES=('migration','session_expiry_helper','session_expiry_admission','security_probes','ui_responses','role_matrix',
       'poll_seconds','pressure','expired_cursor_probe','shell_lifecycle','recovery','inject_rejection_defect',
       'inject_api_cache_defect','inject_role_defect','inject_nonresident_enrollment','inject_security_defect','inject_ui_response_defect')


def validate_mode(args,parser):
    if args.suppress_announcement and not args.announcement_probes:parser.error('announcement suppression requires its own cell')
    if not args.announcement_probes:
        if args.announcement_xvfb_admission or args.announcement_orca_admission:parser.error('AT admissions require announcement probes')
        return
    if args.browser!='firefox' or any(getattr(args,name,False) for name in MODES):parser.error('announcements require a separate Firefox cell')
    if not args.announcement_xvfb_admission or not args.announcement_orca_admission:parser.error('announcements require exact prepared display and reader')
    if not args.package or not args.packaged_console or not re.fullmatch('[a-f0-9]{40}',args.candidate_commit or ''):
        parser.error('announcements require exact package, packaged console and packaged source commit')


def join_package_identity(provenance,commit,package_sha,snapshots):
    require(provenance['source_commit']==commit and provenance['deb_sha256']==package_sha,'announcement-package-identity')
    require(all(provenance['payload_identity']['usr/bin/'+binary]['sha256']==snapshots[role]['sha256']
        for role,binary in (('server','fabric-server'),('spindle','fabric-node'))),'announcement-package-identity')


def admit_package(args,receipt):
    from resource_group import STORAGE
    package=args.package.resolve();console=args.packaged_console.resolve()
    require(package.is_relative_to(STORAGE) and package.is_file() and console.is_relative_to(STORAGE),
        'announcement-package-identity')
    record=console.parent.parent/'doc/fabrico11y/CANDIDATE-PROVENANCE.json'
    provenance=json.loads(record.read_text());digest=hashlib.sha256(package.read_bytes()).hexdigest()
    join_package_identity(provenance,args.candidate_commit,digest,receipt['immutable_binary_snapshots'])
    receipt['announcement_package']=dict(package=str(package),package_sha256=digest,packaged_source_commit=args.candidate_commit,
        provenance_sha256=hashlib.sha256(record.read_bytes()).hexdigest(),payload_identity=provenance['payload_identity'])


def document_identity(rows,pid,origin):
    return [row for row in rows if row['type']=='document:snapshot' and row['pid']==pid
        and any(value.startswith('DocURL:'+origin+'/console/') for value in row.get('document_attributes',[]))]


class Runtime:
    def __init__(self,work,out,args,deadline):
        self.work=work/'announcement-runtime';self.out=out/'announcements';self.deadline=deadline;self.process=None
        self.work.mkdir(mode=0o700);self.out.mkdir(mode=0o700)
        for name in ('tmp','runtime','config','cache','data'):(self.work/name).mkdir(mode=0o700)
        self.descriptor=self.work/'driver.json';self.args=args;self.write_root=work

    def command(self,argv):
        self.descriptor.write_text(json.dumps(dict(work=str(self.work),out=str(self.out),write_root=str(self.write_root),
            deadline=self.deadline,argv=argv,xvfb_receipt=str(self.args.announcement_xvfb_admission.resolve()),
            orca_receipt=str(self.args.announcement_orca_admission.resolve()))))
        return [sys.executable,'-B',str(Path(__file__).with_name('announcement_probe.py')),'--driver-wrapper',str(self.descriptor)]

    def heard(self,text):
        return any(text in spoken_text(row['text']) and row['ssip_code']==225 and row['message_id']>0
            for row in read_rows(self.out/'speech.jsonl'))

    def start(self,js,wait,pid,origin):
        require(js('return !Array.from(document.querySelectorAll("input[type=password]")).some(e=>e.value) && !Array.from(document.querySelectorAll("code")).some(e=>/^[a-f0-9]{64}$/.test(e.textContent.trim()))'),
            'announcement-secrets-cleared')
        (self.work/'start-reader').touch()
        wait(lambda:(self.out/'at-ready.json').exists(),'private actual reader ready',seconds=20)
        self.ready=json.loads((self.out/'at-ready.json').read_text());self.pid=pid
        require(self.ready['affinity']==[2,3] and self.ready['orca_runtime']['version']=='51.1','announcement-runtime')
        parent={name:Path('/proc/self/ns/'+name).readlink().as_posix() for name in ('pid','net','mnt','ipc')}
        require(all(self.ready['namespaces'][name]!=parent[name] for name in ('pid','mnt','ipc'))
            and self.ready['namespaces']['net']==parent['net'],'announcement-caller-namespaces')
        js('Array.from(document.querySelectorAll("button")).find(e=>e.textContent.trim()==="Check session").focus()')
        wait(lambda:self.heard('Check session'),'actual reader focus presentation',seconds=20)
        wait(lambda:document_identity(read_rows(self.out/'events.jsonl'),pid,origin),
            'actual AT document identity',seconds=20)
        documents=document_identity(read_rows(self.out/'events.jsonl'),pid,origin)
        require(documents,'announcement-document-identity')
        self.ready['documents']=documents

    def window(self,js,wait,selector,texts,action,alert=False):
        original=js('const e=document.querySelector(arguments[0]);if(!e)return null;return {role:e.getAttribute("role"),live:e.getAttribute("aria-live"),focus_elsewhere:document.activeElement!==e}',selector)
        if alert:
            require(original is None,'announcement-alert-baseline')
        else:require(original==dict(role='status',live='polite',focus_elsewhere=True),'announcement-dom')
        require(time.monotonic()+8<=self.deadline,'announcement-deadline')
        before=time.monotonic_ns();pcm=self.out/'speech.raw';before_bytes=pcm.stat().st_size
        if self.args.suppress_announcement and not alert:js('document.querySelector(arguments[0]).setAttribute("aria-live","off")',selector)
        action()
        until=min(self.deadline,time.monotonic()+8)
        while time.monotonic()<until:
            require(self.process.poll() is None,'announcement-subtree-exit');time.sleep(.05)
        after=time.monotonic_ns()
        if alert:original=js('const e=document.querySelector(arguments[0]);return {role:e?.getAttribute("role"),live:e?.getAttribute("aria-live"),focus_elsewhere:document.activeElement!==e}',selector)
        events=read_rows(self.out/'events.jsonl');speech=read_rows(self.out/'speech.jsonl')
        backend=dict(self.ready['backend'],pcm_before_bytes=before_bytes,pcm_after_bytes=pcm.stat().st_size)
        append(self.out/'actions.jsonl',dict(before_ns=before,after_ns=after,dom=original,texts=texts,backend=backend,
            events='events.jsonl',speech='speech.jsonl',event_indices=[i for i,event in enumerate(events)
                if before<=event['monotonic_ns']<=after],speech_indices=[i for i,row in enumerate(speech)
                if before<=row['before_ns']<=row['after_ns']<=after]))
        for text in texts:
            case=dict(original,before_ns=before,after_ns=after,text=text,browser_pid=self.pid)
            (check_alert if alert else check_announcement)(case,events,speech,backend)

    def close(self):
        if self.process is None:return
        (self.work/'stop-reader').touch()
        self.process.wait(timeout=27)
        require(self.process.returncode==0 and (self.out/'inside.json').exists(),'announcement-cleanup')
        receipt=json.loads((self.out/'inside.json').read_text())
        require(receipt['exit']==0 and not receipt['cleanup_errors'],'announcement-cleanup')


def inventory(js,label):
    rows=js('''return Array.from(document.querySelectorAll('button,input:not([type=hidden]),select,textarea,summary,a[href],[role=button]'))
      .filter(e=>!e.disabled && getComputedStyle(e).visibility!=='hidden' && e.getClientRects().length)
      .map((e,i)=>{const r=e.getBoundingClientRect(),s=getComputedStyle(e),p=e.closest('p,li');
        const radius=v=>{const a=v.split(/\s+/);const part=(t,n)=>t.endsWith('%')?parseFloat(t)*n/100:parseFloat(t);
          return [part(a[0],r.width),part(a[1]||a[0],r.height)]};
        return {id:String(i),kind:e.tagName.toLowerCase(),text:(e.textContent||e.getAttribute('aria-label')||e.type||'').trim().slice(0,256),
          x:r.x,y:r.y,width:r.width,height:r.height,transform:s.transform,clip:s.clipPath,
          radii:[s.borderTopLeftRadius,s.borderTopRightRadius,s.borderBottomRightRadius,s.borderBottomLeftRadius].map(radius),
          inline:{display:s.display,ancestor:p?.tagName.toLowerCase(),context:p?.textContent.slice(0,1024)||''}}}).filter(e=>e.width>0&&e.height>0)''')
    return dict(view=label,targets=grade_targets(rows))


def run(runtime,*,js,wd,click,field,nav,wait,principal,origin,browser_pid,receipt):
    runtime.start(js,wait,browser_pid,origin)
    receipt['announcement_runtime']=runtime.ready
    status='[data-testid=live-console] [role=status]';owner='Browser fixture owner · owner'
    receipt['announcement_actions']=str(runtime.out/'actions.jsonl');receipt['target_inventory']=[]
    for width in (1440,375):
        wd('/window/rect',dict(width=width,height=1100 if width==1440 else 900))
        nav('Explore');receipt['target_inventory'].append(inventory(js,'explore-'+str(width)))
        nav('Settings');js('document.querySelectorAll("details").forEach(e=>e.open=true)')
        receipt['target_inventory'].append(inventory(js,'settings-'+str(width)))
    nav('Explore')
    js('''window.announcementFetch=window.fetch;window.fetch=async(...args)=>{const r=await window.announcementFetch(...args);
      const p=new URL(typeof args[0]==='string'?args[0]:args[0].url,location.href).pathname;
      if(p==='/v1/console/session')await new Promise(resolve=>setTimeout(resolve,1000));return r}''')
    try:runtime.window(js,wait,status,['Working…',owner],lambda:click('Check session'))
    finally:js('window.fetch=window.announcementFetch;delete window.announcementFetch')
    runtime.window(js,wait,'[role=alert]',['Filter exceeds 4096 UTF-8 bytes. Edit it before querying.'],
        lambda:field('Body substring (case-sensitive)','x'*4097),alert=True)
    field('Body substring (case-sensitive)','')
    runtime.window(js,wait,status,['Sign in to view your telemetry.'],lambda:click('Sign out'))
    wait(lambda:js('return document.body.textContent.includes("Sign in with a passkey")'),'actual announcement logout')
    for width in (1440,375):
        wd('/window/rect',dict(width=width,height=900));receipt['target_inventory'].append(inventory(js,'locked-auth-'+str(width)))
    field('Principal ID',principal)
    element=wd('/element',dict(using='css selector',value='input[autocomplete=username]'))['element-6066-11e4-a52e-4f735466cecf']
    js('document.querySelector("input[autocomplete=username]").focus()')
    wd('/element/'+element+'/value',dict(text='\ue004',value=['\ue004']))
    require(js('return document.activeElement.textContent.trim()==="Use passkey"'),'announcement-keyboard-auth')
    button=wd('/element/active',method='GET')['element-6066-11e4-a52e-4f735466cecf']
    runtime.window(js,wait,status,[owner],lambda:wd('/element/'+button+'/value',dict(text='\ue007',value=['\ue007'])))
    body=wd('/element',dict(using='css selector',value='body'))['element-6066-11e4-a52e-4f735466cecf']
    js('document.body.setAttribute("tabindex","-1");document.body.focus()')
    wd('/element/'+body+'/value',dict(text='\ue004',value=['\ue004']));js('document.body.removeAttribute("tabindex")')
    require(js('return document.activeElement.classList.contains("skip-link") && document.activeElement.getBoundingClientRect().top>=0'),'announcement-keyboard-skip')
    skip=wd('/element/active',method='GET')['element-6066-11e4-a52e-4f735466cecf']
    wd('/element/'+skip+'/value',dict(text='\ue007',value=['\ue007']))
    require(js('return location.hash==="#main-content" && document.querySelector("#main-content").getBoundingClientRect().bottom>0'),'announcement-keyboard-skip')
    receipt['announcement_inputs']={name:hashlib.sha256(Path(path).read_bytes()).hexdigest() for name,path in
        (('orca_admission',runtime.args.announcement_orca_admission),('xvfb_admission',runtime.args.announcement_xvfb_admission))}
