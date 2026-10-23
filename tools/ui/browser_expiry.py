"""Exact controlled-time AB/C supplement; ordinary browser clocks stay unchanged."""
import hashlib
import math
import os
from pathlib import Path
import time

import session_clock_oracle as oracle
from session_clock import freeze_images, kernel_time, process_binding


def locked_view(snapshot):
    expected = dict(title='Connect to FabricO11y', sign_in_visible=True,
        principal_visible=True, sign_out_disabled=True, protected_results=False,
        owner_visible=False)
    return isinstance(snapshot, dict) and all(name in snapshot
        and type(snapshot[name]) is type(value) and snapshot[name] == value
        for name, value in expected.items())


def denied_check_request(rows):
    if not isinstance(rows, list) or len(rows) != 1 or not isinstance(rows[0], dict):
        return False
    row = rows[0]
    return (row.get('path') == '/v1/console/session' and row.get('method') == 'GET'
        and row.get('status') == 401 and row.get('state') == 'response'
        and all(type(row.get(key)) in (int, float) and math.isfinite(row[key])
                for key in ('started_ms', 'finished_ms'))
        and 0 <= row['started_ms'] <= row['finished_ms'])


def align(clock, second, deadline, pace):
    """Choose forward-only offsets; adjacent boundaries wait in real time."""
    pace()
    previous=int.from_bytes(clock.frames[-1][16:24],'little')
    while True:
        oracle.require(time.monotonic()<deadline,'fixture-deadline')
        raw=kernel_time();host,fraction=divmod(raw,1000000000)
        current=host+previous
        oracle.require(current<=second,'session-boundary-inconclusive')
        if current==second and 50_000_000<=fraction<=200_000_000:return
        if current<second-1 and 50_000_000<=fraction<=200_000_000:
            clock.advance(second-host);return
        time.sleep(.005)


def cookie_hash(cookie):return hashlib.sha256(cookie['value'].encode()).hexdigest()


def run_expiry(args,receipt,clocks,groups,parent,states,first_fixture,start_owner,
               wd,js,async_js,click,wait,check,api,bearer_api,pace,deadline,cadence,
               cookie,view,run_started):
    from live_browser_check import (browser_directory,browser_members,stop_browser_groups,
                                    browser_reap_domain)
    observations,sessions,fixtures=[],{},{}
    receipt['session_expiry']={'scope':'controlled wall time; not elapsed endurance or qualification',
        'observations':observations,'sessions':sessions,'fixtures':fixtures}
    identities=receipt['session_expiry_inputs']
    run_ns=int(run_started*1000000000)

    def guarded_request(clock,token,second,path='/v1/console/session'):
        return clock.request(lambda:bearer_api(None,path,cookie=token['name']+'='+token['value'],paced=False),second)

    def bind(key,fixture):
        clock=clocks[key+'server']
        frozen=freeze_images(args.server,args.session_expiry_helper)
        binding=process_binding(clock.process.pid,args.server,args.session_expiry_helper,expected=frozen)
        fdinfo=Path(f'/proc/{clock.process.pid}/fdinfo/{clock.fd}').read_text()
        check(os.readlink(f'/proc/{clock.process.pid}/fd/{clock.fd}')==clock.socket_identity
            and int(fdinfo.split('flags:',1)[1].split()[0],8)&os.O_CLOEXEC,'actual server clock channel remains CLOEXEC')
        fixture.update({name:identities[name] for name in ('candidate_commit','package_sha256','helper_sha256')},
            executable_sha256=frozen['executable_sha256'],process_binding=binding,
            run_started_monotonic_ns=run_ns,deadline_monotonic_ns=run_ns+240000000000,
            control_fd={'number':clock.fd,'identity':clock.socket_identity,'fdinfo':fdinfo})
        fixtures[key]=fixture
        # Actual descendants must have neither helper mapping nor inherited control.
        isolated=[]
        for group in groups:
            if not group['group']['label'].startswith(key):continue
            browser_members(group['group'])
            for member in group['group']['members']:
                pid=member['pid']
                if pid==clock.process.pid:continue
                proc=Path('/proc')/str(pid)
                if not proc.exists():continue
                maps=(proc/'maps').read_text()
                environment=(proc/'environ').read_bytes().split(b'\0')
                fd_present=False
                for fd in (proc/'fd').iterdir():
                    try:fd_present |= os.readlink(fd)==clock.socket_identity
                    except FileNotFoundError:pass
                check(str(args.session_expiry_helper) not in maps and not any(
                    item.startswith((b'LD_PRELOAD=',b'FABRIC_CLOCK_FD=')) for item in environment)
                    and not fd_present,'actual ordinary descendant excludes clock helper, keys and channel')
                isolated.append({'pid':pid,'start_ticks':member['start_ticks'],'executable':str((proc/'exe').resolve()),
                    'helper_mapped':False,'control_env_names':[],'control_fd_present':False})
        fixture['ordinary_descendants']=isolated
        return clock

    def quiet():
        # Overview has no polling. Drain pending requests before freezing baselines.
        js('''const button=Array.from(document.querySelectorAll('button')).find(e=>e.textContent.trim()==='Pause 5-second polling');if(button)button.click();''')
        wait(lambda:js('return (window.fixtureApiHeaders||[]).every(e=>e.finished_ms!=null)'), 'all browser API requests terminal')

    def baseline(clock,tokens):
        quiet();pace()
        # Bind the final successful sessions-list read to an ordinary native second.
        while not 50_000_000<=kernel_time()%1000000000<=200_000_000:
            oracle.require(time.monotonic()<deadline,'fixture-deadline');time.sleep(.005)
        second=kernel_time()//1000000000
        (status,body),window=guarded_request(clock,tokens[-1][1],second,'/v1/console/sessions')
        check(status==200 and isinstance(body,list),'actual owner sessions-list baseline succeeds')
        receipt['session_expiry'].setdefault('baselines',[]).append(dict(window,server_second=second,sessions=body))
        for name,token in tokens:
            identity=cookie_hash(token);metadata=next(row for row in body if row['id']==identity)
            sessions[name]={'hash':identity,'issued_s':metadata['issued_unix_s'],
                'expires_s':metadata['expires_unix_s'],'baseline_seen_s':metadata['last_seen_unix_s']}
        # Any incidental browser auth dispatch invalidates the frozen experiment.
        js('''window.fixtureExpiryUnexpected=[];window.fixtureExpiryFetch=window.fetch;
          window.fetch=(...a)=>{const path=new URL(typeof a[0]==='string'?a[0]:a[0].url,location.href).pathname;
            if(path.startsWith('/v1/console/')){window.fixtureExpiryUnexpected.push(path);return Promise.reject(Error('expiry fixture froze incidental browser requests'));}
            return window.fixtureExpiryFetch(...a)}''')

    def observe(clock,cell,key,token,second,status,seen):
        align(clock,second,deadline,pace)
        answer,window=guarded_request(clock,token,second)
        code,body=answer
        row=dict(window,cell=cell,session_hash=sessions[key]['hash'],pid=clock.process.pid,
            issued_s=sessions[key]['issued_s'],expires_s=sessions[key]['expires_s'],
            last_seen_s=seen,request_s=second,status=code,error=body.get('error') if code>=400 and isinstance(body,dict) else None)
        observations.append(row)
        check(code==status and row['error']==('sign in required' if status==401 else None),cell+' exact semantic response')
        check(js('return window.fixtureExpiryUnexpected.length===0'),'no incidental authenticated browser request after baseline')

    def locked():
        before=kernel_time();browser=js('return Date.now()');after=kernel_time()
        check(before//1000000<=browser<=after//1000000,'actual browser Date remains ordinary')
        receipt['session_expiry'].setdefault('browser_clocks',[]).append({'raw_before_ns':before,'date_ms':browser,'raw_after_ns':after})
        js('window.fetch=window.fixtureExpiryFetch;delete window.fixtureExpiryFetch')
        check(js('return document.querySelector(".live-connection .connection-title")?.innerText.trim()==="Connected"'),
            'actual browser is connected before denied Check session')
        before=js('return window.fixtureApiHeaders.length')
        click('Check session')
        def snapshot():
            return js('''const connection=document.querySelector('.live-connection');
              const visible=e=>!!e && e.getClientRects().length>0 && getComputedStyle(e).visibility!=='hidden';
              const status=connection?.querySelector('[role=status]');
              const principal=Array.from(document.querySelectorAll('label')).find(e=>e.textContent.trim().startsWith('Principal ID'))?.querySelector('input');
              const signout=Array.from(connection?.querySelectorAll('button')||[]).find(e=>e.textContent.trim()==='Sign out');
              return {title:connection?.querySelector('.connection-title')?.innerText.trim(),
                sign_in_visible:visible(status)&&status.innerText.trim()==='Sign in to view your telemetry.',
                principal_visible:visible(principal),sign_out_disabled:!!signout&&signout.disabled,
                protected_results:!!document.querySelector('.results,.tail-terminal'),
                owner_visible:document.body.textContent.includes('Browser fixture owner · owner')};''')
        wait(lambda:locked_view(snapshot()),'actual denied check locks browser')
        request_rows=js('return window.fixtureApiHeaders.slice(arguments[0])',before)
        check(denied_check_request(request_rows),'actual Check session consumed one complete browser401')
        receipt['session_expiry'].setdefault('locked_views',[]).append({'snapshot':snapshot(),'requests':request_rows})
        check(js('return !document.querySelector(".results") && !document.querySelector(".tail-terminal") && !document.body.textContent.includes("Browser fixture owner · owner")'),
            'actual session 401 clears protected browser views')

    def stopped(key):
        fixture=fixtures[key];clock=clocks[key+'server']
        wd('',method='DELETE')
        selected=[binding for binding in groups if binding['group']['label'].startswith(key)]
        stop_browser_groups(parent,selected);browser_reap_domain(parent,groups)
        held=next(item for item in states if str(item['path'])==fixture['state_before']['path'])
        browser_directory(held)
        fixture['state_after']={'path':str(held['path']),'device':held['dev'],'inode':held['ino'],'observed_monotonic_ns':kernel_time(1)}
        fixture['owned_groups']=[{'pgid':b['group']['pid'],'start_ticks':b['group']['start_ticks']} for b in selected]
        fixture['stop_observations']=[dict(row,population=0,confirmed=b['group'].get('kernel_empty') is True and b['group'].get('children_reaped') is True,
            observed_monotonic_ns=kernel_time(1)) for row,b in zip(fixture['owned_groups'],selected)]
        fixture['stopped_monotonic_ns']=kernel_time(1)
        clock.close();fixture['ledger']=clock.rows;fixture['frames']=[frame.hex() for frame in clock.frames]
        clock.closed=True

    clock=bind('AB',first_fixture);a=cookie
    wd('/cookie/'+a['name'],method='DELETE')
    code,start=api('/v1/console/auth/login/start','POST',{'principal_id':view['principal_id']})
    check(code==200,'independent no-cookie B login starts')
    credential=async_js('const done=arguments[arguments.length-1];window.fabricPasskey(false,JSON.stringify(arguments[0])).then(value=>done(JSON.parse(value))).catch(error=>done({fixture_error:String(error)}))',start['public_key'])
    check('fixture_error' not in credential,'B uses actual resident UV assertion')
    code,_=api('/v1/console/auth/login/finish','POST',{'ceremony_id':start['ceremony_id'],'credential':credential})
    check(code==200,'independent no-cookie B login finishes')
    b=next(item for item in wd('/cookie',method='GET') if item['name']==a['name'])
    check(cookie_hash(a)!=cookie_hash(b),'A and B are distinct genuinely issued sessions')
    baseline(clock,[('A',a),('B',b)])
    observe(clock,'idle_before','A',a,sessions['A']['baseline_seen_s']+1799,200,sessions['A']['baseline_seen_s'])
    observe(clock,'idle_at','B',b,sessions['B']['baseline_seen_s']+1800,401,sessions['B']['baseline_seen_s'])
    locked();stopped('AB')
    _,c,_,_,_,_,fixture=start_owner('C')
    clock=bind('C',fixture);baseline(clock,[('C',c)])
    seen=sessions['C']['baseline_seen_s'];issued=sessions['C']['issued_s']
    for step in range(1000,28001,1000):
        observe(clock,'absolute_keepalive','C',c,issued+step,200,seen);seen=issued+step
    observe(clock,'absolute_before','C',c,issued+28799,200,seen)
    observe(clock,'absolute_at','C',c,issued+28800,401,issued+28799)
    locked();stopped('C')
    oracle.check_session_fixtures(observations,sessions,fixtures)
    check(True,'all exact32 native-bound AB/C session observations satisfy unchanged oracle')
