"""Diagnostic UI boundary responses, separately labelled from actual server evidence."""
import copy
import hashlib
import json
import time

CASES=('empty','incomplete','gap','410','429','503','transport','timeout','403','401')
ERRORS={'410':'page expired; run the query again','429':'request rate limit','503':'service unavailable',
        '403':'access denied','401':'sign in required'}


def grade(rows):
    if len(rows)!=len(CASES) or {r['case'] for r in rows}!=set(CASES):
        raise ValueError('UI response case inventory missing or duplicated')
    for row in rows:
        case=row['case'];text=row['text']
        good=row['dispatches']==1 and not row['busy']
        if case=='empty':good=good and row['rows']==0 and '0 observations' in text and not row['locked']
        elif case=='incomplete':good=good and 'Incomplete coverage' in text and not row['locked']
        elif case=='gap':good=good and '1 reported gaps' in text and not row['locked']
        elif case in ERRORS:
            good=good and ('HTTP '+case+': '+ERRORS[case]) in row['alert']
            good=good and row['locked']==(case in ('401','403'))
            if case in ('401','403'):good=good and row['rows']==0 and not row['sensitive_present']
            if case in ('429','503'):good=good and 'no automatic retry' in row['alert']
        else:
            good=good and 'Request interrupted, offline, or exceeded its 15-second deadline.' in row['alert'] and not row['locked']
            if case=='timeout':good=good and row['abort_observed'] and row['elapsed_ns']>=14_500_000_000
        if not good:raise ValueError('UI response property rejected: '+case)
    return True


def run(*,js,click,wait,field,nav,baseline,observations,inject=None):
    if not baseline.get('rows'):raise ValueError('actual nonempty query baseline required')
    raw=json.dumps(baseline,separators=(',',':'))
    witness={'classification':'browser response handling diagnostic; injected responses are not server outcomes',
             'actual_response_sha256':hashlib.sha256(raw.encode()).hexdigest(),'actual_rows':len(baseline['rows'])}
    for case in CASES:
        nav('Explore');field('Signal','logs');field('Source name (blank = all sources)','browser-edge')
        field('Body substring (case-sensitive)','browser-page-fixture');click('Last 15 minutes')
        value=copy.deepcopy(baseline);value['next_page']=None;status=200
        if case=='empty':value['rows']=[]
        elif case=='incomplete':value['complete']=False
        elif case=='gap':value['gaps']=[{'reason':'diagnostic UI response fixture'}];value['complete']=False
        elif case in ERRORS:status=int(case);value={'error':ERRORS[case]}
        mode=case
        if inject==case:
            # Contradictory input must be detected at the same UI property.
            mode='empty';status=200;value=copy.deepcopy(baseline)
            if case!='empty':value['rows']=[];value['complete']=True;value['gaps']=[]
        js('''window.responseOriginalFetch=window.fetch;window.responseDispatches=0;window.responseAbort=false;
          const mode=arguments[0],status=arguments[1],text=arguments[2];
          window.fetch=(...args)=>{const path=new URL(typeof args[0]==='string'?args[0]:args[0].url,location.href).pathname;
          if(path!='/v1/console/query')return window.responseOriginalFetch(...args);
          window.responseDispatches++;if(window.responseDispatches!==1)return Promise.reject(new Error('unexpected repeated diagnostic dispatch'));
          if(mode==='transport')return Promise.reject(new TypeError('owned transport fixture'));
          if(mode==='timeout'){const signal=args[1]?.signal||args[0]?.signal;return new Promise((resolve,reject)=>{
            if(!signal)return reject(new Error('actual request signal missing'));
            signal.addEventListener('abort',()=>{window.responseAbort=true;reject(new DOMException('Aborted','AbortError'))},{once:true})})}
          return Promise.resolve(new Response(text,{status,headers:{'Content-Type':'application/json','Cache-Control':'no-store'}}))}''',mode,status,json.dumps(value,separators=(',',':')))
        began=time.monotonic_ns()
        try:
            click('Run query')
            wait(lambda:js('return window.responseDispatches>0&&!document.body.textContent.includes("Working…")'), 'diagnostic query terminates',seconds=20)
            # A short quiet interval detects immediate automatic retries.
            time.sleep(.6)
            row=js('''return {text:document.body.textContent.slice(0,32768),alert:Array.from(document.querySelectorAll('[role=alert]')).map(e=>e.textContent).join(' '),
              rows:document.querySelectorAll('.results tbody tr').length,busy:document.body.textContent.includes('Working…'),
              locked:!document.body.textContent.includes('Browser fixture owner · owner'),sensitive_present:document.body.textContent.includes('ordinal='),
              dispatches:window.responseDispatches,abort_observed:window.responseAbort}''')
            row.update(case=case,elapsed_ns=time.monotonic_ns()-began,injected_response_status=status)
            observations.append(row)
        finally:
            js('window.fetch=window.responseOriginalFetch;delete window.responseOriginalFetch')
        if row['locked']:
            click('Check session')
            wait(lambda:js('return document.body.textContent.includes("Browser fixture owner · owner")&&!document.body.textContent.includes("Working…")'), 'real unchanged session restores UI after injected denial')
    return witness
