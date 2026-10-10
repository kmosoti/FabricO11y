"""Registered R3 fixed-query adapter; independent fixture projection, unchanged oracle."""
import base64
import hashlib
import random
import argparse
import concurrent.futures
import gzip
import json
from pathlib import Path
import shutil
import subprocess
import threading
import time
import urllib.error
import urllib.request

import query_oracle as oracle
from release_fixture import NS, SEEDS, integer, message, query_batch, traces, varint
from workload import entropy_body


def file_digest(path):
    digest=hashlib.sha256()
    with Path(path).open('rb') as source:
        for block in iter(lambda:source.read(1024**2),b''):
            digest.update(block)
    return digest.hexdigest()


def append_observation(path, value, *, max_bytes):
    """Close each bounded line before proceeding, preserving completed observations."""
    encoded=json.dumps(value,separators=(',',':'))+'\n'
    existing=path.stat().st_size if path.exists() else 0
    if existing+len(encoded.encode())>max_bytes:
        raise RuntimeError('incremental observation archive exceeds finite allowance')
    with path.open('a') as output:
        output.write(encoded)


def query_plan(seed, start_ns, *, smoke=False):
    """Prospectively fixed coordinates; no returned data influences selection."""
    if seed not in SEEDS or start_ns <= 0:
        raise ValueError('unregistered query plan')
    rng = random.Random(seed)
    nodes, duration, repeats = (2, 5, 1) if smoke else (100, 500, 20)
    result = []
    for repetition in range(repeats):
        node = rng.randrange(nodes)
        offset = rng.randrange(max(1, duration-60))
        begin = start_ns + offset*NS
        end = min(start_ns+duration*NS, begin+60*NS)
        common = {'from_ns': begin, 'to_ns': end, 'limit': 1000}
        result.append({'kind': 'source_logs', 'repetition': repetition,
                       'query': dict(common, kind='logs', node=f'query{node:04d}')})
        # Choose an odd entropy coordinate inside the declared search window.
        coordinate = offset*10+1
        needle = entropy_body(seed, rng.randrange(nodes), coordinate)[100:112]
        result.append({'kind': 'rare_text', 'repetition': repetition,
                       'query': dict(common, kind='logs', contains=needle)})
        result.append({'kind': 'source_counter', 'repetition': repetition,
                       'query': {'kind': 'metrics', 'node': f'query{rng.randrange(nodes):04d}',
                                 'name': 'release.counter', 'from_ns': start_ns,
                                 'to_ns': start_ns+duration*NS, 'limit': 1000}})
        begin = start_ns+rng.randrange(duration)*NS
        result.append({'kind': 'fleet_counter', 'repetition': repetition,
                       'query': {'kind': 'metrics', 'name': 'release.counter',
                                 'from_ns': begin, 'to_ns': begin+NS, 'limit': 1000}})
        _, span_rows = traces(seed, rng.randrange(nodes)*10+rng.randrange(1 if smoke else 10), start_ns)
        result.append({'kind': 'trace', 'repetition': repetition,
                       'query': {'kind': 'spans', 'trace_id': span_rows[0]['trace_id'],
                                 'from_ns': start_ns, 'to_ns': start_ns+duration*NS,
                                 'limit': 1000}})
    rng.shuffle(result)
    return result


def join_verified_recovery(originals, recovered):
    """Use recovery only for commit metadata after exact original-byte proof."""
    expected = {}
    for original in originals:
        raw = base64.b64decode(original['bytes'], validate=True)
        batch = oracle.decode_batch(raw)
        key = (batch['node_id'], batch['generation'], batch['sequence'])
        if key in expected:
            raise ValueError('duplicate independent source identity')
        expected[key] = original
    seen, joined, additional = set(), [], []
    for record in recovered:
        raw = base64.b64decode(record['bytes'], validate=True)
        batch = oracle.decode_batch(raw)
        key = (batch['node_id'], batch['generation'], batch['sequence'])
        if key in seen:
            raise ValueError('duplicate recovered Batch identity')
        seen.add(key)
        if key not in expected:
            additional.append(record)
            continue
        original = expected[key]
        if raw != base64.b64decode(original['bytes'], validate=True) or record['label'] != original['label']:
            raise ValueError('recovered bytes or enrolled label differ from independent source')
        if type(record['received_ns']) is not int or record['received_ns'] <= 0:
            raise ValueError('invalid recovery commit metadata')
        joined.append(dict(original, received_ns=record['received_ns']))
    if set(expected)-seen:
        raise ValueError('ACKed independent source missing from recovery')
    return joined, additional


def project_records(records, query, authorized_labels):
    """Project only scope/signal metadata, never the query's row selection.

    Original records remain the custody witness. This deliberately admits only
    the registered zero-gap fixture, not general scoped gap-only traffic.
    """
    selected = {'logs': 6, 'metrics': 5, 'spans': 9}.get(query.get('kind'))
    if selected is None:
        raise ValueError('console fixed queries support logs, metrics and spans only')
    authorized = set(authorized_labels)
    if query.get('node') is not None and query['node'] not in authorized:
        raise ValueError('query label is outside independent fixture authority')
    projected = []
    for record in records:
        if record['label'] not in authorized:
            continue
        raw = base64.b64decode(record['bytes'], validate=True)
        batch = oracle.decode_batch(raw)
        if batch['gaps']:
            raise ValueError('authorized fixture gap violates zero-gap projection admission')
        if query.get('node') is not None and record['label'] != query['node']:
            continue
        payload = batch[{6: 'logs_bytes', 5: 'metrics_bytes', 9: 'traces_bytes'}[selected]]
        rows = {6: oracle.decode_logs_request, 5: oracle.decode_metrics_request,
                9: oracle.decode_traces_request}[selected](payload)
        if not rows or (selected == 5 and not any(metric['points'] for metric in rows)):
            continue
        parts = []
        for number, entries in oracle.parse_fields(raw).items():
            if number in (5, 6, 9) and number != selected:
                continue
            for wire, value in entries:
                if wire == 0:
                    parts.append(integer(number, value))
                elif wire == 2:
                    parts.append(message(number, value))
                elif wire in (1, 5):
                    parts.append(varint(number * 8 + wire) + value)
                else:
                    raise ValueError('unsupported fixture protobuf wire type')
        projected.append({'label': record['label'], 'received_ns': record['received_ns'],
                          'bytes': base64.b64encode(b''.join(parts)).decode('ascii')})
    return projected


def prepare_projections(records, authorized_labels):
    """Cache scope/signal projection only; row selection stays in the oracle."""
    return {kind: project_records(records, {'kind':kind}, authorized_labels)
            for kind in ('logs','metrics','spans')}


def grade(records, query, pages, authorized_labels, *, prepared=None):
    if any(page.get('gaps') for page in pages):
        return {'passed': False, 'violations': [{'rule': 'UNEXPECTED-FIXTURE-GAP',
                                                'detail': 'zero-gap scoped fixture returned gaps'}]}
    if prepared is None:
        projected = project_records(records, query, authorized_labels)
    else:
        if query.get('node') is not None and query['node'] not in authorized_labels:
            raise ValueError('query label is outside independent fixture authority')
        projected = [record for record in prepared[query['kind']]
                     if query.get('node') is None or record['label']==query['node']]
    oracle_query = {key: value for key, value in query.items() if value is not None}
    result = oracle.check(projected, oracle_query, pages)
    result['projection_records'] = len(projected)
    projection_digest=hashlib.sha256()
    for record in projected:
        projection_digest.update(json.dumps({'label':record['label'],'received_ns':record['received_ns'],
            'batch_sha256':hashlib.sha256(base64.b64decode(record['bytes'])).hexdigest()},
            sort_keys=True,separators=(',',':')).encode()+b'\n')
    result['projection_sha256'] = projection_digest.hexdigest()
    return result


class ResourceSampler:
    """Observe setup, native operation, restart, and post-stop grading alike."""
    def __init__(self, fixture, interval=1):
        self.fixture=fixture;self.interval=interval
        self.samples=[];self.errors=[];self.lock=threading.Lock()
        self.stop=threading.Event()

    def observe(self):
        from release_runtime import cgroups, process_kib
        with self.lock:
            f=self.fixture
            if f.server.poll() is None:
                result=f.resource_sample()
            else:
                result={'monotonic_ns':time.monotonic_ns(),**f.storage_sample(),
                        'groups':cgroups.snapshot(f.parent)}
            result['grader_rss_kib']=process_kib(__import__('os').getpid())
            if len(self.samples)>=4000:
                raise RuntimeError('resource observation allowance exceeded')
            append_observation(f.out/'continuous-resources.jsonl',result,max_bytes=16*1024**2)
            self.samples.append(result)

    def run(self):
        while not self.stop.wait(self.interval):
            try:self.observe()
            except BaseException as error:
                self.errors.append(str(error));return

    def check(self):
        if self.errors:
            raise RuntimeError('resource sampler failed: '+self.errors[0])

    def __enter__(self):
        self.observe()
        self.thread=threading.Thread(target=self.run,daemon=True)
        self.thread.start();return self

    def __exit__(self, kind, error, traceback):
        self.stop.set();self.thread.join(timeout=15)
        if self.thread.is_alive():
            raise RuntimeError('resource sampler did not stop')
        if kind is None:
            self.observe();self.check()


def perform(f, args, summary, meter):
    dependencies=[Path(__file__),Path(oracle.__file__),Path(__file__).with_name('release_fixture.py'),
                  Path(__file__).with_name('release_runtime.py'),Path(__file__).with_name('console_bridge.py'),
                  Path(__file__).with_name('soak_companion.py'),Path(__file__).with_name('release_timing.py'),
                  Path(__file__).with_name('workload.py'),Path(__file__).with_name('delivery_oracle.py')]
    source_hashes={str(path):file_digest(path) for path in dependencies}
    summary['harness_inputs']=source_hashes
    nodes, batches = (2, 1) if args.smoke else (100, 100)
    start_ns = time.time_ns()-501*NS
    labels = {f'query{i:04d}' for i in range(nodes)}
    source_file = f.work/'independent-source.jsonl'
    originals, enrollments = [], []
    for node in range(nodes):
        meter.check()
        # Console fixture calls stay below its admission limit; no 429 counts
        # as permission evidence or a successful enrollment.
        enrollments.append(f.enroll(f'query{node:04d}'))
        time.sleep(.15)
    with source_file.open('w') as output:
        for node in range(nodes):
            for sequence in range(1,batches+1):
                raw=query_batch(args.seed,node,sequence,start_ns)
                record={'label':f'query{node:04d}', 'bytes':base64.b64encode(raw).decode()}
                originals.append(record)
                output.write(json.dumps(record)+'\n')
    summary['fixture']={'sources':nodes,'batches':len(originals),'logs':nodes*batches*50,
                        'metrics':nodes*batches*50,'traces':nodes*min(10,batches),
                        'spans':nodes*min(10,batches)*3,'start_ns':start_ns,
                        'source_sha256':file_digest(source_file)}
    reader=f.reader([e['enrollment_id'] for e in enrollments])
    samples=meter.samples;resource_lock=meter.lock
    acked=[]; ack_lock=threading.Lock()
    attempt_file=f.work/'actual-attempt-events.jsonl'
    query_file=f.work/'actual-query-observations.jsonl'
    def deliver(node):
        for record in originals[node*batches:(node+1)*batches]:
            raw=base64.b64decode(record['bytes'])
            sequence=oracle.decode_batch(raw)['sequence']
            began=time.monotonic()
            attempts=[]
            def record_attempt(event):
                with ack_lock:
                    append_observation(attempt_file,{'label':record['label'],'sequence':sequence,**event},
                                       max_bytes=32*1024**2)
            while True:
                meter.check()
                attempted_ns=time.monotonic_ns()
                request=urllib.request.Request(f.origin+'/v1/batches',data=raw,
                    headers={'authorization':'Bearer '+enrollments[node]['token'],
                             'content-type':'application/x-protobuf'})
                try:
                    with urllib.request.urlopen(request,context=f.context,timeout=15) as response:
                        status=response.status;answer=json.loads(response.read(65537))
                    attempts.append({'status':status,'answer':answer,'before_ns':attempted_ns,
                                     'after_ns':time.monotonic_ns(),'sha256':hashlib.sha256(raw).hexdigest()})
                    record_attempt(attempts[-1])
                    if status!=200 or answer.get('status')!='ack' or answer.get('committed_through')!=sequence:
                        raise ValueError('fixture Batch received an invalid ACK')
                    with ack_lock:
                        acked.append({'label':record['label'],'sequence':sequence,
                                      'sha256':hashlib.sha256(raw).hexdigest(),'attempts':attempts,
                                      'elapsed_s':time.monotonic()-began})
                    break
                except urllib.error.HTTPError as error:
                    attempts.append({'status':error.code,'before_ns':attempted_ns,
                                     'after_ns':time.monotonic_ns(),'sha256':hashlib.sha256(raw).hexdigest()})
                    record_attempt(attempts[-1])
                    if error.code not in (429,503) or time.monotonic()-began>120:
                        raise RuntimeError('fixture delivery rejected or failed to drain') from None
                    time.sleep(.2)
                except (OSError,urllib.error.URLError):
                    attempts.append({'status':'no_response','before_ns':attempted_ns,
                                     'after_ns':time.monotonic_ns(),'sha256':hashlib.sha256(raw).hexdigest()})
                    record_attempt(attempts[-1])
                    if time.monotonic()-began>120:
                        raise RuntimeError('fixture delivery failed to drain') from None
                    time.sleep(.2)
    answers=[]
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            for future in [pool.submit(deliver,node) for node in range(nodes)]:
                future.result()
        if args.mode=='segment':
            deadline=time.monotonic()+300
            while list((f.work/'state/journal').glob('sealed-*.faj')):
                if time.monotonic()>deadline:
                    raise RuntimeError('closed fixture journals did not publish')
                time.sleep(.2)
        manifests=list((f.work/'state/segments').rglob('manifest.json'))
        summary['storage_mode']={'segment_manifests':len(manifests),
                                 'closed_journals':len(list((f.work/'state/journal').glob('sealed-*.faj')))}
        if args.mode=='journal' and manifests:
            raise RuntimeError('journal-only fixture unexpectedly published Segments')
        if args.mode=='segment' and not args.smoke and not manifests:
            raise RuntimeError('Segment fixture published no Segments')
        plan=query_plan(args.seed,start_ns,smoke=args.smoke)
        (f.out/'query-plan.json').write_text(json.dumps(plan,indent=2)+'\n')
        for phase in ('before_restart','after_restart'):
            if phase=='after_restart':
                with resource_lock:
                    f.restart()
            f.bridge.poll_ui(True)
            for item in plan:
                meter.check()
                elapsed,pages=reader.pages(item['query'])
                answers.append(dict(item,phase=phase,elapsed_s=elapsed,pages=pages))
                append_observation(query_file,answers[-1],max_bytes=128*1024**2)
                # Test calls are paced; successful latency includes every page.
                time.sleep(.1)
            f.bridge.poll_ui(False)
        with resource_lock:
            f.stop_server()
    finally:
        meter.check()
    recovery=f.work/'verified-recovery.jsonl'
    with recovery.open('wb') as output:
        subprocess.run([str(f.helpers/'server_dump'),str(f.config),'--records'],
                       stdout=output,stderr=subprocess.PIPE,check=True,timeout=120)
    with recovery.open() as incoming:
        recovered=[json.loads(line) for line in incoming if line.strip()]
    joined,additional=join_verified_recovery(originals,recovered)
    # A companion source prefix is obtained independently from the stopped Spool.
    import soak_companion
    companion=soak_companion.dump_stopped_spool(f.helpers/'spool_dump',
                f.work/'state/self-spindle/node.conf', f.work/'companion-custody',
                processes_stopped=True)
    projected_recovery=[]
    for record in additional:
        batch=oracle.decode_batch(base64.b64decode(record['bytes']))
        projected_recovery.append(soak_companion.project({'type':'recovered',
            'node_id':batch['node_id'].hex(),'generation':batch['generation'],
            'sequence':batch['sequence'],'bytes':record['bytes']}))
    summary['companion_custody']=soak_companion.validate_recovery(companion,projected_recovery)
    if len(projected_recovery)!=summary['companion_custody']['recovered_sources']:
        raise RuntimeError('recovery contains an unexplained non-fixture source')
    import delivery_oracle
    ledger=list(companion['projected_sources'])
    acknowledgments={(event['label'],event['sequence']):event for event in acked}
    if len(acknowledgments)!=len(originals):
        raise ValueError('missing or duplicate actual ACK observation')
    for record in originals:
        batch=oracle.decode_batch(base64.b64decode(record['bytes']))
        ident={'node_id':batch['node_id'].hex(),'generation':batch['generation'],'sequence':batch['sequence']}
        data=soak_companion.project(dict(record))['bytes']
        ledger.append({'type':'source',**ident,'bytes':data})
        observed=acknowledgments[(record['label'],batch['sequence'])]
        for attempt in observed['attempts']:
            if attempt['sha256']!=hashlib.sha256(base64.b64decode(record['bytes'])).hexdigest():
                raise ValueError('actual attempted bytes differ from retained source')
            ledger.append({'type':'attempt',**ident,'bytes':data,'injected_conflict':False})
            if attempt['status']==200:
                if attempt['answer']['status']!='ack' or attempt['answer']['committed_through']!=batch['sequence']:
                    raise ValueError('actual ACK differs from retained source identity')
                ledger.append({'type':'response',**ident,'kind':'ack',
                               'committed_through':attempt['answer']['committed_through']})
            else:
                ledger.append({'type':'response',**ident,
                               'kind':'no_response' if attempt['status']=='no_response' else 'unavailable'})
    for record in recovered:
        batch=oracle.decode_batch(base64.b64decode(record['bytes']))
        ledger.append(soak_companion.project({'type':'recovered','node_id':batch['node_id'].hex(),
            'generation':batch['generation'],'sequence':batch['sequence'],'bytes':record['bytes']}))
    ledger.append({'type':'end'})
    custody=delivery_oracle.check(json.dumps(record) for record in ledger)
    summary['delivery_oracle']={'passed':custody.passed,'violations':custody.violations}
    archive=f.archive_dir()
    for source in [source_file,recovery,f.work/'companion-custody/spool.jsonl',attempt_file,query_file]:
        with source.open('rb') as incoming,gzip.open(archive/(source.name+'.gz'),'wb') as output:
            shutil.copyfileobj(incoming,output)
    (f.out/'delivery-acks.json').write_text(json.dumps(acked)+'\n')
    with gzip.open(archive/'answers.json.gz','wt') as output:
        json.dump(answers,output)
    with gzip.open(archive/'independent-receive-join.jsonl.gz','wt') as output:
        for record in joined:
            output.write(json.dumps(record)+'\n')
    with gzip.open(archive/'custody-ledger.jsonl.gz','wt') as output:
        for record in ledger:
            output.write(json.dumps(record)+'\n')
    for source in dependencies:
        shutil.copyfile(source,archive/source.name)
    source_count=len(originals)
    del originals,recovered,ledger,additional,projected_recovery,acknowledgments
    groups={}
    all_passed=True
    preparation_start=time.monotonic()
    prepared=prepare_projections(joined,labels)
    del joined
    summary['projection_preparation_s']=time.monotonic()-preparation_start
    from release_runtime import cgroups, directory_bytes, process_kib
    def grader_sample():
        result={'monotonic_ns':time.monotonic_ns(),'grader_rss_kib':process_kib(__import__('os').getpid()),
                'groups':cgroups.snapshot(f.parent),
                **f.storage_sample()}
        return result
    grader_samples=[grader_sample()]
    for index,answer in enumerate(answers):
        meter.check()
        started=time.monotonic()
        verdict=grade([],answer['query'],answer['pages'],labels,prepared=prepared)
        grader_samples.append(grader_sample())
        answer['verdict']=verdict
        answer['grading_s']=time.monotonic()-started
        all_passed=all_passed and verdict['passed']
        key=answer['phase']+':'+answer['kind']
        groups.setdefault(key,[]).append(answer['elapsed_s'])
        (f.out/f'verdict-{index:03d}.json').write_text(json.dumps({k:v for k,v in answer.items() if k!='pages'})+'\n')
    from release_timing import p99
    summary['latency']={key:{'n':len(values),'p99_s':p99(values),'max_s':max(values)} for key,values in groups.items()}
    summary['resources']=samples
    summary['grader_resources']=grader_samples
    summary['gates']={'all_answers_oracle':all_passed,'all_fixture_acked':len(acked)==source_count,
        'delivery_oracle':custody.passed,
        'query_p99_le_2s':all(p99(values)<=2 for values in groups.values()),
        'server_rss_le_2gib':bool(samples) and max(s['server_hwm_kib'] for s in samples if 'server_hwm_kib' in s)<=2*1024**2,
        'source_manifest_unchanged':file_digest(source_file)==summary['fixture']['source_sha256'],
        'harness_source_unchanged':all(file_digest(path)==source_hashes[str(path)] for path in dependencies),
        'no_oom':all('oom_kill 0' in group['memory.events'] for sample in samples+grader_samples for group in sample['groups'].values())}
    summary['passed']=all(summary['gates'].values())
    # Persist every verdict and reply, then recheck combined scratch/archive caps.
    with gzip.open(archive/'graded-answers.json.gz','wt') as output:
        json.dump(answers,output)
    # resource_sample requires a live server; disk admission is checked directly.
    from release_runtime import directory_bytes
    summary['final_storage']=f.storage_sample()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--deb-receipt',required=True)
    parser.add_argument('--rpm-receipt',required=True)
    parser.add_argument('--out',required=True)
    parser.add_argument('--seed',required=True,type=lambda value:int(value,0))
    parser.add_argument('--mode',choices=('segment','journal'),required=True)
    parser.add_argument('--smoke',action='store_true')
    args=parser.parse_args()
    if args.seed not in SEEDS or (args.mode=='journal' and args.seed!=SEEDS[0]):
        parser.error('unregistered fixed-query cell')
    from release_runtime import CandidateFixture
    summary={'classification':'disposable query smoke' if args.smoke else 'registered fixed-query cell',
             'seed':args.seed,'mode':args.mode,'passed':False}
    with CandidateFixture(args.deb_receipt,args.rpm_receipt,args.out,mode=args.mode) as f:
        try:
            with ResourceSampler(f) as meter:
                perform(f,args,summary,meter)
            f.receipt['passed']=summary['passed']
        except BaseException as error:
            summary['passed']=False
            summary['failure_type']=type(error).__name__
            summary['failure']=str(error)[:1200]
            raise
        finally:
            (f.out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    return 0 if summary['passed'] else 1


if __name__=='__main__':
    raise SystemExit(main())
