import hashlib,json,pathlib,subprocess
R=pathlib.Path('/tmp/fabric-cli-review'); W=pathlib.Path('/home/kmosoti/fabric-cli-review-big'); B='/tmp/fabric-cli-b/tools/storage-probe/target/release/fabric-research'
log=W/'big-over64.fol2'; source=W/'big-input.json'; prior_snap=W/'big-snap'; prior_trust=W/'big-trust.json'; fresh_snap=W/'repair-snap';fresh_trust=W/'repair-trust.json'
assert log.stat().st_size>64*1024*1024
assert prior_snap.exists() and prior_trust.exists()
assert not fresh_snap.exists() and not fresh_trust.exists()
def sha(p):
 h=hashlib.sha256()
 with p.open('rb') as f:
  for chunk in iter(lambda:f.read(1<<20),b''): h.update(chunk)
 return h.hexdigest()
before={'log':sha(log),'source':sha(source),'old_trust':sha(prior_trust)}
checks=[]
def run(label,args):
 p=subprocess.run([B,*map(str,args)],capture_output=True,text=True,timeout=120)
 d={'label':label,'argv':list(map(str,args)),'exit':p.returncode,'stdout':p.stdout.strip(),'stderr':p.stderr.strip()};checks.append(d)
 assert p.returncode==1 and not p.stdout and '64 MiB' in p.stderr,d
run('publish-valid-over64', ['publish',log,fresh_snap,fresh_trust,1,999])
assert not fresh_snap.exists() and not fresh_trust.exists()
run('ingest-existing-valid-over64', ['ingest',source,log,1,1])
assert before=={'log':sha(log),'source':sha(source),'old_trust':sha(prior_trust)}
assert prior_snap.exists() and prior_trust.exists()
result={'source_sha256':sha(pathlib.Path('/tmp/fabric-cli-b/tools/storage-probe/src/bin/fabric-research.rs')),'log_bytes':log.stat().st_size,'old_outputs_preserved':True,'fresh_outputs_absent':True,'source_and_log_hashes_unchanged':True,'checks':checks}
(R/'repair-report.json').write_text(json.dumps(result,indent=2))
print(json.dumps(result))
