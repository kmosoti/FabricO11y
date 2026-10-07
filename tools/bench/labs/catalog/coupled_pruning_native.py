"""Additive O7 group soundness diagnostic; never changes historical C2 gates."""
import argparse, base64, copy, gzip, hashlib, json, os, shutil, signal, subprocess, sys, time
from pathlib import Path
ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT/'tools'))
sys.path.insert(0, str(ROOT/'tools/bench'))
from resource_group import require_limits, STORAGE
import run_responsibility_isolation as grader
CAP = 20*1024**2
FAILURE = 8*1024**2

def dump(path, value):
    path.write_text(json.dumps(value, indent=2)+'\n')

def sha(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

def footprint(root):
    files = {}
    for path in root.rglob('*'):
        if path.is_file():
            st = path.stat(); files[st.st_dev,st.st_ino] = (st.st_size,st.st_blocks*512)
    return max(sum(x[0] for x in files.values()), sum(x[1] for x in files.values()))

def bounds(work, out):
    if footprint(out)+FAILURE>CAP or footprint(work)>8*1024**3 or shutil.disk_usage(STORAGE).free<16*1024**3:
        raise RuntimeError('O7 evidence/scratch/free-space admission failed')

def producer_rows(records):
    rows = []
    for record in records:
        batch = grader.query_oracle.decode_batch(base64.b64decode(record['bytes']))
        for i,row in enumerate(grader.query_oracle.decode_logs_request(batch['logs_bytes'])):
            rows.append({'node_id':batch['node_id'].hex(), 'sequence':batch['sequence'], 'index':i,
                         'time':row['observed_time_unix_nano'], 'body_sha256':hashlib.sha256(row['body'].encode()).hexdigest()})
    return rows

def check(report, expected):
    groups = report['groups']; windows = report['windows']
    if len(groups)<2 or [g['id'] for g in groups]!=list(range(len(groups))):
        raise ValueError('missing/duplicate/reordered group')
    all_rows = [r for g in groups for r in g['rows']]
    key = lambda r:(r['time'],r['node_id'],r['sequence'],r['index'])
    if all_rows!=sorted(expected,key=key):
        raise ValueError('group identity/content/order partition differs from producer')
    for group in groups:
        if not group['rows'] or not isinstance(group['min'],int) or not isinstance(group['max'],int) or group['min']>group['max']:
            raise ValueError('malformed bounds')
        if any(not group['min']<=row['time']<=group['max'] for row in group['rows']):
            raise ValueError('non-conservative bound')
    if [w['id'] for w in windows]!=list(range(len(windows))) or len(windows)<5:
        raise ValueError('window coverage')
    details = []
    for window in windows:
        q=window['query'];lo=q['from_ns'];hi=q['to_ns']
        if lo>=hi or q!={'kind':'logs','from_ns':lo,'to_ns':hi,'limit':50}:
            raise ValueError('window schema')
        admitted=[g['id'] for g in groups if g['max']>=lo and g['min']<hi]
        if window['admitted']!=admitted:
            raise ValueError('omitted/altered admitted group list')
        matched=[r for r in expected if lo<=r['time']<hi]
        covered=[r for g in groups if g['id'] in admitted for r in g['rows'] if lo<=r['time']<hi]
        if sorted(matched,key=key)!=sorted(covered,key=key):
            raise ValueError('matching identity omitted by pruning')
        details.append({'id':window['id'],'matched_rows':len(matched),'admitted_groups':admitted,
                        'footer_admitted_rows':sum(len(g['rows']) for g in groups if g['id'] in admitted)})
    if not any(d['matched_rows']==0 for d in details) or not any(d['matched_rows']==1100 for d in details):
        raise ValueError('empty/broad coverage absent')
    if not any(groups[i]['rows'][-1]['time']==groups[i+1]['rows'][0]['time'] for i in range(len(groups)-1)):
        raise ValueError('tied boundary fixture missing')
    return details

def controls(report, expected):
    defects = {}
    omit=copy.deepcopy(report)
    window=next(w for w in omit['windows'] if w['admitted'])
    window['admitted'].pop();defects['omitted_admitted_group']=omit
    narrow=copy.deepcopy(report);g=narrow['groups'][0];g['min']=min(r['time'] for r in g['rows'])+1
    defects['narrowed_bound']=narrow
    missing=copy.deepcopy(report);missing['groups'].pop();defects['missing_group']=missing
    altered=copy.deepcopy(report);altered['groups'][0]['rows'][0]['body_sha256']='0'*64
    defects['altered_identity_content']=altered
    rejected=[]
    for name,bad in defects.items():
        try:check(bad,expected)
        except ValueError:rejected.append(name)
        else:raise RuntimeError('negative control accepted: '+name)
    return rejected

def compress(src,dst):
    with src.open('rb') as s,gzip.open(dst,'wb') as d:shutil.copyfileobj(s,d)
    with gzip.open(dst,'rb') as f:
        if hashlib.file_digest(f,'sha256').hexdigest()!=sha(src):raise RuntimeError('gzip readback mismatch')
    return {'original_sha256':sha(src),'gzip_sha256':sha(dst),'original_bytes':src.stat().st_size,'gzip_bytes':dst.stat().st_size}

def main():
    require_limits();parser=argparse.ArgumentParser();parser.add_argument('--bin',type=Path,required=True);parser.add_argument('--out',type=Path,required=True)
    args=parser.parse_args();binary=args.bin.resolve(strict=True);args.out.mkdir(parents=True,exist_ok=False)
    base=Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    if not base.is_relative_to(STORAGE/'scratch'):raise RuntimeError('owned data-drive scratch required')
    work=base/'coupled-pruning-native';trial=work/'trial';work.mkdir();bounds(work,args.out)
    sources=[Path(__file__),ROOT/'crates/fabric-server/examples/coupled_pruning_probe.rs',ROOT/'crates/fabric-server/src/segment/bounded.rs',ROOT/'crates/fabric-server/src/segment.rs',ROOT/'crates/fabric-server/src/query.rs',Path(grader.__file__),Path(grader.query_oracle.__file__),ROOT/'docs/experiments/benchmarks/coupled-pruning-native-protocol.md']
    dump(args.out/'provenance.json',{'revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),'binary_sha256':sha(binary),'sources':{str(p.relative_to(ROOT)):sha(p) for p in sources}})
    compress(binary,args.out/'coupled_pruning_probe.gz');bounds(work,args.out)
    argv=[str(binary),str(trial)];dump(args.out/'command.json',{'argv':argv})
    started=time.monotonic()
    with (args.out/'native.json').open('wb') as output,(args.out/'native.err').open('wb') as error:
        child=subprocess.Popen(argv,cwd=ROOT,stdout=output,stderr=error,start_new_session=True)
        try:
            while child.poll() is None:
                bounds(work,args.out)
                if time.monotonic()-started>180:raise RuntimeError('native deadline')
                time.sleep(.25)
        finally:
            if child.poll() is None:os.killpg(child.pid,signal.SIGKILL);child.wait(timeout=10)
    if child.returncode:raise RuntimeError('native child exit '+str(child.returncode))
    native_elapsed=time.monotonic()-started
    report=json.loads((args.out/'native.json').read_text());records,bodies=grader.decode_records(trial/'records.jsonl')
    if len(bodies)!=1100 or any(len(b)!=16384 for b in bodies) or report['fixture']['run_mib_selector']!='16':raise RuntimeError('fixture drift')
    expected=producer_rows(records);details=check(report,expected);negative=controls(report,expected)
    verdicts=[];archives={}
    archives['records.jsonl']=compress(trial/'records.jsonl',args.out/'records.jsonl.gz')
    for window in report['windows']:
        for plan in ['walk','scan']:
            path=trial/f"chain-{window['id']}-{plan}.jsonl";pages=[json.loads(line) for line in path.read_text().splitlines()]
            verdict=grader.query_oracle.check(records,window['query'],pages)
            if not verdict['passed']:raise RuntimeError('independent full-chain oracle rejected')
            verdicts.append({'window':window['id'],'plan':plan,**verdict})
            if verdict['expected_rows']==1100:
                bad=copy.deepcopy(pages);bad[0]['rows'].append(bad[0]['rows'][0])
                if grader.query_oracle.check(records,window['query'],bad)['passed'] or grader.query_oracle.check(records,window['query'],pages[:-1])['passed']:
                    raise RuntimeError('independent oracle negative accepted')
            archives[path.name]=compress(path,args.out/(path.name+'.gz'));bounds(work,args.out)
    dump(args.out/'result.json',{'soundness':True,'historical_C2_pruning_equal':'failed unchanged','windows':details,'controls_rejected':negative,'full_chain_verdicts':verdicts,'canonical_chains':len(verdicts),'archive_readbacks':archives,'native_exit':0,'native_elapsed_s':native_elapsed,'diagnostic_elapsed_s':time.monotonic()-started,'scope':'footer selection soundness, not measured query IO'})
    bounds(work,args.out);shutil.rmtree(work);dump(args.out/'cleanup.json',{'removed':True})
if __name__=='__main__':main()
