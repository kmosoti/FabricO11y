"""Prompt-level method ablation, NOT proof of native skill routing behavior."""
import argparse,json,pathlib,subprocess,tempfile,time,statistics
p=argparse.ArgumentParser();p.add_argument('--out',required=True);p.add_argument('--heldout',action='store_true');a=p.parse_args();out=pathlib.Path(a.out);out.mkdir(parents=True,exist_ok=False)
original=pathlib.Path.home()/'.codex/skills/multidimensional-reasoning/SKILL.md'
methods={'original':original.read_text(),'tuned':pathlib.Path('integrations/codex/tuning/multidimensional-reasoning.md').read_text(),'none':''}
tasks=[('routine','Rename a local variable; behavior unchanged. Should deep architectural exploration be invoked?', 'routine'),('architecture','Two storage designs differ in durability. Should you choose solely on advertised throughput?', 'compare_semantics'),('debugging','A bug disappeared after changing allocator and concurrency together. Is the allocator established as the cause?', 'discriminate'),('performance','A candidate changes exact retrieval into ANN. Can a speedup establish equivalent exact-query performance?', 'not_equivalent'),('impossible','Finite storage, unlimited outage, nonblocking producer and zero loss forever: can all be guaranteed?', 'impossible'),('proof','A bounded abstract allocator proof passed. Is the entire async database Rust implementation proven?', 'unsupported')]
if a.heldout:
 tasks=[('routine','Fix one broken README link with an established replacement. Is broad architecture exploration needed?','routine'),('architecture','Two engines have equal query speed, but only one exposes replayable durable records. What comparison matters?','compare_semantics'),('debugging','Latency improved while workload size and batching both changed. What should happen before naming a cause?','discriminate'),('performance','Two top-k benchmarks use different authorized populations. Are they equivalent measurements?','not_equivalent'),('impossible','Can arbitrary instrumentation guarantee absolutely no CPU or memory impact for every workload?','impossible'),('proof','A natural-number admission theorem ignores machine-integer overflow. Does it establish the concrete u64 implementation?','not_equivalent')]
results=[]
with tempfile.TemporaryDirectory(prefix='fabric-skill-eval-') as d:
 for repeat in range(1 if a.heldout else 2):
  for name,method in methods.items():
   for kind,question,expected in tasks:
    prompt='Answer only JSON with keys decision and reason. decision must be one of routine,compare_semantics,discriminate,not_equivalent,impossible,unsupported. Do not use tools or edit files.\nMethod guidance (empty means no supplied method):\n'+method+'\nQuestion: '+question
    start=time.monotonic();r=subprocess.run(['codex','exec','--ephemeral','--json','--sandbox','read-only','--skip-git-repo-check','-C',d,prompt],capture_output=True,text=True,timeout=180)
    raw=out/f'{repeat}-{name}-{kind}.jsonl';raw.write_text(r.stdout)
    events=[]
    for line in r.stdout.splitlines():
     try:events.append(json.loads(line))
     except json.JSONDecodeError:pass
    messages=[e['item']['text'] for e in events if e.get('type')=='item.completed' and e.get('item',{}).get('type')=='agent_message']
    usage=next((e.get('usage',{}) for e in reversed(events) if e.get('type')=='turn.completed'),{})
    try:answer=json.loads(messages[-1]);correct=answer.get('decision')==expected
    except (ValueError,IndexError):correct=False
    row={'method':name,'task':kind,'repeat':repeat,'exit':r.returncode,'correct':correct,'seconds':time.monotonic()-start,'usage':usage}
    results.append(row);(out/'results.json').write_text(json.dumps(results,indent=2)+'\n');print(json.dumps(row),flush=True)
    if r.returncode:raise SystemExit('Model execution failed; retain diagnostics, do not grade as successful')
summary={name:{'correct':sum(r['correct'] for r in results if r['method']==name),'runs':6 if a.heldout else 12,'median_seconds':statistics.median(r['seconds'] for r in results if r['method']==name),'median_input_tokens':statistics.median(r['usage'].get('input_tokens',0) for r in results if r['method']==name)} for name in methods}
(out/'summary.json').write_text(json.dumps({'scope':'prompt-level supplied-method ablation; no held-out/native-routing claim','methods':summary},indent=2)+'\n')
