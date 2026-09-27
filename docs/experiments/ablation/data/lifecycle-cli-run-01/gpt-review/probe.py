import hashlib, json, os, pathlib, subprocess, shutil
R=pathlib.Path('/tmp/fabric-cli-review')
B=pathlib.Path('/tmp/fabric-cli-b/tools/storage-probe/target/release/fabric-research')
W=R/'cases'
if W.exists(): shutil.rmtree(W)
W.mkdir()
checks=[]
def digest(p): return hashlib.sha256(p.read_bytes()).hexdigest()
def put(p, obj): p.write_text(json.dumps(obj, separators=(',',':')))
def call(label, *args, code=0):
    p=subprocess.run([str(B),*map(str,args)],capture_output=True,text=True)
    result={'label':label,'argv':list(map(str,args)),'exit':p.returncode,'stdout':p.stdout.strip()[:800],'stderr':p.stderr.strip()[:800]}
    checks.append(result)
    assert p.returncode==code, result
    return json.loads(p.stdout) if code==0 else result
req=W/'request.json'; cfg=W/'config.json'; source=W/'input.json'
put(req,{'resourceLogs':[{'resource':{'attributes':[{'key':'rk','value':{'stringValue':'rv'}}]},'scopeLogs':[{'logRecords':[{'timeUnixNano':'0','observedTimeUnixNano':'5','body':{'stringValue':'same same'}},{'body':{'stringValue':'same same'}},{'timeUnixNano':'7','body':{'stringValue':'different'}},{'timeUnixNano':'8','body':{'stringValue':'same'}}]}]}]})
put(cfg,{'tenant':4,'source':5,'resource':6,'first_event_id':10})
source_hashes=(digest(req),digest(cfg))
a=call('adapt', 'adapt-otlp',req,cfg,source)
assert a['adapted_events']==4
assert (digest(req),digest(cfg))==source_hashes
rows=json.loads(source.read_text())['events']
assert len(rows)==4 and [r['id'] for r in rows]==[10,11,12,13]
assert rows[0]['payload']['body']==rows[1]['payload']['body']=='same same'
source_hash=digest(source)
call('adapt-existing', 'adapt-otlp',req,cfg,source,code=1)
assert digest(source)==source_hash
for name,raw in [('dup-config',b'{"tenant":4,"tenant":5,"source":5,"resource":6,"first_event_id":10}'),('string-config',b'{"tenant":"4","source":5,"resource":6,"first_event_id":10}'),('unknown-config',b'{"tenant":4,"source":5,"resource":6,"first_event_id":10,"x":1}')]:
    bad=W/(name+'.json'); bad.write_bytes(raw); out=W/(name+'.out')
    call(name,'adapt-otlp',req,bad,out,code=1); assert not out.exists()
for name,raw in [('duplicate-member',b'{"resourceLogs":[],"resourceLogs":[]}'),('lost',b'{"resourceLogs":[{"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"droppedAttributesCount":1}]}]}]}'),('bad-body',b'{"resourceLogs":[{"scopeLogs":[{"logRecords":[{"body":{"intValue":"1"}}]}]}]}')]:
    bad=W/(name+'.json');bad.write_bytes(raw);out=W/(name+'.out')
    call(name,'adapt-otlp',bad,cfg,out,code=1);assert not out.exists()
for cap,batch in [(1,1),(1,7),(2,1),(2,7),(3,2)]:
    log=W/f'log-{cap}-{batch}.fol2'
    first=call(f'ingest-{cap}-{batch}','ingest',source,log,cap,batch)
    assert (first['input_events'],first['appended'],first['peak_buffer_events'])==(4,4,min(4,cap))
    assert first['buffer_retries']>0 if cap<4 else True
    prior=log.read_bytes()
    again=call(f'retry-{cap}-{batch}','ingest',source,log,cap,batch)
    assert (again['already_committed'],again['appended'])==(4,0)
    assert log.read_bytes()==prior
    wrong=W/'wrong.json'; altered=json.loads(source.read_text());altered['events'][0]['payload']['body']='CHANGED';put(wrong,altered)
    call(f'wrong-prefix-{cap}-{batch}','ingest',wrong,log,cap,batch,code=1)
    assert log.read_bytes()==prior and digest(source)==source_hash
log=W/'log-1-7.fol2'; snap=W/'snap'; trusted=W/'trusted.json'
absent=W/'absent.fol2'; no_snap=W/'no-snap'; no_trust=W/'no-trust.json'
call('publish-missing-log','publish',absent,no_snap,no_trust,2,901,code=1)
assert not no_snap.exists() and not no_trust.exists()
inside=snap/'inside.json'
call('trust-inside','publish',log,snap,inside,2,901,code=1)
assert not snap.exists()
aliased=W/'alias';aliased.symlink_to(W, target_is_directory=True)
call('trust-dotdot','publish',log,aliased/'snap',aliased/'snap'/'..'/'trusted.json',2,901,code=1)
assert not snap.exists()
symlink_out=W/'trusted-link.json';symlink_out.symlink_to(trusted)
call('trust-symlink-existing','publish',log,snap,symlink_out,2,901,code=1)
assert not snap.exists()
call('publish','publish',log,snap,trusted,2,901)
pub_hash=digest(trusted)
q=W/'query.json';put(q,{'start_ns':0,'end_ns':9,'tenant':4,'token':'same'})
qhash=digest(q);cp=W/'cp.json'
answer=call('query','query',snap,trusted,q,'all',cp)
assert answer['complete'] and answer['positions']==[0,1,3], answer
assert answer['checkpoint_sha256']==digest(cp)
verify=call('verify','verify',source,trusted,q,cp,answer['checkpoint_sha256'])
assert verify=={'verified':True,'matched':3,'input_events':4}
for name, raw in [('dup-query',b'{"start_ns":0,"start_ns":1,"end_ns":9,"tenant":4,"token":"same"}'),('unknown-query',b'{"start_ns":0,"end_ns":9,"tenant":4,"token":"same","x":1}'),('missing-query',b'{"start_ns":0,"end_ns":9,"tenant":4}')]:
    bad=W/(name+'.json');bad.write_bytes(raw);out=W/(name+'.cp')
    call(name,'query',snap,trusted,bad,'all',out,code=1);assert not out.exists()
wrongdig='0'+answer['checkpoint_sha256'][1:] if not answer['checkpoint_sha256'].startswith('0') else '1'+answer['checkpoint_sha256'][1:]
call('verify-wrong-digest','verify',source,trusted,q,cp,wrongdig,code=1)
call('verify-wrong-source','verify',wrong,trusted,q,cp,answer['checkpoint_sha256'],code=1)
q2=W/'q2.json';put(q2,{'start_ns':0,'end_ns':9,'tenant':4,'token':'different'})
call('verify-wrong-query','verify',source,trusted,q2,cp,answer['checkpoint_sha256'],code=1)
partial=W/'partial.json';ans_partial=call('query-incomplete','query',snap,trusted,q,'10',partial)
assert not ans_partial['complete']
call('verify-incomplete','verify',source,trusted,q,partial,ans_partial['checkpoint_sha256'],code=1)
assert digest(source)==source_hash and digest(trusted)==pub_hash and digest(q)==qhash
# Recomputed digest cannot make a malformed row/history pass the checkpoint loader.
cp_bad=W/'cp-tampered.json'; cp_obj=json.loads(cp.read_text()); cp_obj['pages'][0]['rows'][0]['digest'][0]^=1 if isinstance(cp_obj['pages'][0]['rows'][0]['digest'],list) else 0
put(cp_bad,cp_obj)
call('verify-mutated-checkpoint','verify',source,trusted,q,cp_bad,digest(cp_bad),code=1)
late=W/'late.json'; extended=json.loads(source.read_text());late_row=dict(extended['events'][3]);late_row['id']=14;extended['events'].append(late_row);put(late,extended)
call('append-late','ingest',late,log,1,9)
snap2=W/'snap2';trust2=W/'trust2.json';call('publish-successor','publish',log,snap2,trust2,2,902)
call('verify-old-on-successor','verify',source,trust2,q,cp,answer['checkpoint_sha256'],code=1)
cp2=W/'cp2.json';call('resume-old-on-successor','resume',snap2,trust2,q,cp,answer['checkpoint_sha256'],'all',cp2,code=1)
assert not cp2.exists()
# Empty input and exact zero-block availability.
empty_req=W/'empty-request.json';empty_input=W/'empty-input.json';put(empty_req,{'resourceLogs':[]});call('adapt-empty','adapt-otlp',empty_req,cfg,empty_input)
empty_log=W/'empty.fol2';call('ingest-empty','ingest',empty_input,empty_log,1,4)
empty_snap=W/'empty-snap';empty_trust=W/'empty-trust.json';call('publish-empty','publish',empty_log,empty_snap,empty_trust,1,903)
empty_cp=W/'empty-cp.json';empty_answer=call('query-empty','query',empty_snap,empty_trust,q,'',empty_cp)
assert empty_answer['complete'] and empty_answer['positions']==[]
call('verify-empty','verify',empty_input,empty_trust,q,empty_cp,empty_answer['checkpoint_sha256'])
report={'source_sha256':digest(pathlib.Path('/tmp/fabric-cli-b/tools/storage-probe/src/bin/fabric-research.rs')),'checks':checks,'verdict':'APPROVE','assertions':'all process assertions passed'}
(R/'report.json').write_text(json.dumps(report,indent=2))
print(json.dumps({'checks':len(checks),'verdict':report['verdict'],'source_sha256':report['source_sha256']}))
