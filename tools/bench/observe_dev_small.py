#!/usr/bin/env python3
"""Finite native profiles with resource, acceptance and HTTP consumer evidence."""
import argparse
import base64
import copy
import gzip
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tarfile
import threading
import time
from collections import Counter

import run_dev_small as native
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import STORAGE, require_limits

ROOT = native.REPO
PROTOCOL = ROOT/'docs/experiments/benchmarks/dev-small-observation-protocol.md'
QUERY = dict(kind='logs', from_ns=0, to_ns=9_000_000_000_000_000_000, limit=50)
METRIC = 'system.cpu.time'


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def cgroup():
    rel = next(s[3:] for s in Path('/proc/self/cgroup').read_text().splitlines() if s.startswith('0::'))
    return Path('/sys/fs/cgroup')/rel.lstrip('/')


def proc(pid):
    out = native.process_stats(pid)
    fields = Path(f'/proc/{pid}/stat').read_text().rpartition(') ')[2].split()
    hz = os.sysconf('SC_CLK_TCK')
    out.update(user_s=int(fields[11])/hz, system_s=int(fields[12])/hz,
               threads=int(fields[17]), io={k:int(v) for k,v in
               (line.split(':') for line in Path(f'/proc/{pid}/io').read_text().splitlines())})
    return out


def exact_live(answer, expected):
    """Compare returned sentinel rows to a source hash; never accept duplicates."""
    rows = answer['rows']
    return (len(rows) == 1 and
            hashlib.sha256(rows[0]['body'].encode()).hexdigest() == expected and
            answer['complete'] and not answer['unavailable'] and not answer['gaps'] and
            answer['next_page'] is None)


def windows(epoch, last):
    return [('normal', epoch, epoch+60_000_000_000), ('burst', epoch+60_000_000_000, epoch+120_000_000_000),
            ('recovery', epoch+120_000_000_000, epoch+180_000_000_000), ('drain', epoch+180_000_000_000, last)]


def classify(t, epoch):
    delta = t-epoch
    return 'settling' if delta < 0 else 'normal' if delta < 60_000_000_000 else 'burst' if delta < 120_000_000_000 else 'recovery' if delta < 180_000_000_000 else 'drain'


def phase_rates(epoch, last, batch_sizes, events, source_path):
    """Group raw event instants; epoch nanoseconds must never pass through float."""
    counts = {phase:Counter() for phase,_,_ in windows(epoch,last)}
    def add(t, values):
        if epoch <= t < last:
            counts[classify(t,epoch)].update(values)
    seen = set()
    for node, rows in events.items():
        for e in rows:
            if e['kind']=='cycle':
                add(e['t'], {'spool_batches':1, 'spool_logs':int(e['logs']),
                             'spool_bytes':batch_sizes[(node,int(e['batch']))]})
            else:
                values = {'send_attempts':1, 'status_'+e['status']:1}
                key = (node,int(e['sequence']))
                if e['status']=='ack' and key not in seen:
                    seen.add(key)
                    values.update(unique_acks=1, ack_bytes=batch_sizes[key])
                add(e['t'], values)
    with gzip.open(source_path, 'rt') as stream:
        for line in stream:
            source = json.loads(line)
            add(source[2], {'source_logs':1, 'source_bytes':901})
    result = {}
    for phase, begin, end in windows(epoch,last):
        duration = (end-begin)/1_000_000_000
        values = counts[phase]
        result[phase] = {'window_seconds':duration, 'counts':dict(values),
            'per_second':{k:v/duration for k,v in values.items()},
            'ack_attempt_fraction':values['status_ack']/values['send_attempts'] if values['send_attempts'] else None}
    return result


def cpu_windows(samples, epoch):
    result = {}
    for name, begin, end in [('whole', samples[0]['wall_ns'], samples[-1]['wall_ns']+1), *windows(epoch, samples[-1]['wall_ns']+1)]:
        rows = [s for s in samples if begin <= s['wall_ns'] < end]
        if len(rows) < 2:
            result[name] = {'samples': len(rows)}
            continue
        seconds = (rows[-1]['mono_ns']-rows[0]['mono_ns'])/1e9
        groups = {'server':[s['processes'][0] for s in rows],
                  'observer':[s['observer'] for s in rows]}
        for i in range(1, len(rows[0]['processes'])):
            groups[f'node{i-1:02}'] = [s['processes'][i] for s in rows]
        metrics = {}
        for label, values in groups.items():
            interval = [(b['cpu_s']-a['cpu_s'])/((r['mono_ns']-l['mono_ns'])/1e9)
                        for a,b,l,r in zip(values,values[1:],rows,rows[1:])]
            metrics[label] = {'cpu_seconds': values[-1]['cpu_s']-values[0]['cpu_s'],
                'mean_cores': (values[-1]['cpu_s']-values[0]['cpu_s'])/seconds,
                'max_interval_cores': max(interval),
                'user_seconds': values[-1]['user_s']-values[0]['user_s'],
                'system_seconds': values[-1]['system_s']-values[0]['system_s'],
                'peak_rss_mib': max(v['rss_kib'] for v in values)/1024,
                'io_delta': {k:values[-1]['io'][k]-values[0]['io'][k] for k in values[0]['io']}}
        nodes = [v for k,v in metrics.items() if k.startswith('node')]
        metrics['all_nodes'] = {'cpu_seconds':sum(v['cpu_seconds'] for v in nodes),
            'mean_cores':sum(v['mean_cores'] for v in nodes),
            'peak_aggregate_rss_mib':max(sum(p['rss_kib'] for p in s['processes'][1:])/1024 for s in rows)}
        result[name] = {'samples':len(rows), 'observed_seconds':seconds, 'processes':metrics}
    return result


class Observer:
    def __init__(self):
        self.done = threading.Event()
        self.targets = queue.Queue()
        self.target_rows = []
        self.queries = []
        self.threads = []
        self.finals = []

    def sample(self, root, kids, events):
        group = cgroup()
        progress = []
        for i in range(len(kids)-1):
            ev = list(events.get(f'node{i:02}', []))
            cycles = [e for e in ev if e['kind']=='cycle']
            acks = [e for e in ev if e['kind']=='ack_attempt' and e['status']=='ack']
            latest = cycles[-1] if cycles else {}
            batch = int(latest.get('batch', 0))
            ack = int(acks[-1]['committed_through']) if acks else 0
            progress.append({'batch':batch, 'acked':ack, 'pending_batches':batch-ack,
                'file_backlog_bytes':int(latest.get('log_backlog_bytes', 0)),
                'spool_bytes':int(latest.get('spool_bytes', 0))})
        return {'processes':[proc(p.pid) for p in kids], 'observer':proc(os.getpid()),
            'cgroup':{name:(group/name).read_text().strip() for name in
                      ['memory.current','memory.peak','memory.stat','memory.events','memory.swap.current',
                       'memory.pressure','cpu.stat','cpu.pressure','io.stat','io.pressure','pids.current']},
            'progress':progress, 'segment_bytes':native.footprint(root/'state/segments'),
            'spool_disk_bytes':sum(native.footprint(root/f'node{i:02}-spool') for i in range(len(kids)-1))}

    def request(self, shape, query, scheduled):
        start = time.time_ns()
        mono = time.monotonic_ns()
        try:
            answer = self.api('/v1/admin/query', query)
            error = None
        except Exception as exc:
            answer, error = None, repr(exc)
        row = {'shape':shape, 'scheduled_ns':scheduled, 'start_ns':start,
               'end_ns':time.time_ns(), 'elapsed_ms':(time.monotonic_ns()-mono)/1e6,
               'query':query, 'answer':answer, 'error':error,
               'json_answer_bytes':len(json.dumps(answer).encode()) if answer else 0}
        self.queries.append(row)
        return row

    def start(self, api, epoch, nodes):
        self.api, self.epoch = api, epoch
        def client():
            tick = 0
            due = time.monotonic()
            while not self.done.wait(max(0, due-time.monotonic())):
                now = time.time_ns()
                if tick % 3 == 0:
                    shape, query = 'recent_logs', dict(QUERY, from_ns=now-10_000_000_000, to_ns=now)
                elif tick % 3 == 1:
                    shape, query = 'absent_text', dict(QUERY, contains='ABSENT-profile-sentinel')
                else:
                    shape, query = 'cpu_metrics', dict(QUERY, kind='metrics', name=METRIC)
                self.request(shape, query, now+int((due-time.monotonic())*1e9))
                tick += 1
                due = max(due+1, time.monotonic())
        def visibility():
            while not self.done.is_set():
                try:
                    target = self.targets.get(timeout=.1)
                except queue.Empty:
                    continue
                while not self.done.is_set() and time.time_ns()-target['source_ns'] <= 30e9:
                    row = self.request('visibility', dict(QUERY, node='node'+target['tag'][:2],
                        contains='load-'+target['tag']+' ', limit=2), time.time_ns())
                    if row['answer'] and exact_live(row['answer'], target['sha256']):
                        target['visible_ns'] = row['end_ns']
                        target['row'] = row['answer']['rows'][0]
                        break
                    if self.done.wait(.25):
                        break
        for fn in (client, visibility):
            thread = threading.Thread(target=fn)
            thread.start()
            self.threads.append(thread)

    def target(self, tag, sha, source_ns, phase):
        target = dict(tag=tag, sha256=sha, source_ns=source_ns, phase=phase, visible_ns=None)
        self.target_rows.append(target)
        self.targets.put(target)

    def stop(self):
        self.done.set()
        for thread in self.threads:
            thread.join(12)
        if any(t.is_alive() for t in self.threads):
            raise RuntimeError('consumer thread did not stop')

    def quiescent(self, api):
        queries = [dict(QUERY, node='node00', contains='load-00:000', limit=7),
                   dict(QUERY, contains='ABSENT-profile-sentinel'),
                   dict(QUERY, kind='metrics', node='node00', name=METRIC)]
        for query in queries:
            pages, current = [], query
            for _ in range(100):
                page = api('/v1/admin/query', current)
                pages.append(page)
                if page['next_page'] is None:
                    break
                current = dict(query, page=page['next_page'])
            else:
                raise RuntimeError('quiescent pagination exceeded 100 pages')
            self.finals.append({'query':query, 'pages':pages})

    def archive(self, root):
        with gzip.open(root/'queries.jsonl.gz', 'wt') as out:
            for row in self.queries:
                out.write(json.dumps(row)+'\n')
        native.dump(root/'visibility.json', self.target_rows)
        native.dump(root/'quiescent.json', self.finals)

    def finish(self, root, summary, samples, events):
        self.archive(root)
        # Only the returned live log rows are checked here. Full snapshot chains
        # below go through the independent oracle without changing that oracle.
        live = {}
        for request in self.queries:
            if request['answer'] and request['query']['kind']=='logs':
                for row in request['answer']['rows']:
                    key = (row['node'], row['sequence'], row['index'])
                    live.setdefault(key, []).append(row)
        unmatched = set(live)
        wrong = 0
        with (root/'recovered.jsonl').open() as stream:
            for line in stream:
                record = json.loads(line)
                mat = native.query_oracle.materialize_record(record['label'], record['received_ns'], base64.b64decode(record['bytes']))
                for row in mat.log_rows:
                    key = (row['node'], row['sequence'], row['index'])
                    if key in live:
                        wrong += sum(r != row for r in live[key])
                        unmatched.discard(key)
        records = native.query_oracle.load_records_jsonl(str(root/'recovered.jsonl'))
        verdicts = [native.query_oracle.check(records, q['query'], q['pages']) for q in self.finals]
        controls = []
        for defect in ('missing', 'changed'):
            changed = copy.deepcopy(self.finals[0])
            assert changed['pages'][0]['rows'], 'query control requires nonempty source'
            if defect == 'missing':
                changed['pages'][0]['rows'].pop()
            else:
                changed['pages'][0]['rows'][0]['body'] += 'changed'
            controls.append({'defect':defect, 'verdict':native.query_oracle.check(records, changed['query'], changed['pages'])})
        del records
        native.dump(root/'query-verdicts.json', {'final':verdicts, 'controls':controls,
            'unique_live_rows':len(live), 'unmatched_live_rows':len(unmatched), 'changed_live_rows':wrong})
        batch_sizes = {}
        with gzip.open(root/'recovered-hashes.jsonl.gz', 'rt') as stream:
            for line in stream:
                node, seq, sha, size, received = json.loads(line)
                batch_sizes[(node, seq)] = size
        buckets, cycles, acks = {}, {}, {}
        for node, rows in events.items():
            for e in rows:
                second = (e['t']-self.epoch)//1_000_000_000
                b = buckets.setdefault(second, Counter())
                if e['kind']=='cycle':
                    key = (node, int(e['batch']))
                    if key in cycles:
                        raise RuntimeError('duplicate successful cycle observation')
                    cycles[key] = e
                    b.update(spool_batches=1, spool_logs=int(e['logs']), spool_bytes=batch_sizes[key])
                else:
                    b['send_attempts'] += 1
                    b['status_'+e['status']] += 1
                    if e['status']=='ack':
                        key = (node, int(e['sequence']))
                        if key not in acks:
                            acks[key] = e
                            b.update(unique_acks=1, ack_bytes=batch_sizes[key])
        lags = []
        with gzip.open(root/'sources.jsonl.gz', 'rt') as stream:
            for line in stream:
                tag, sha, source, phase, lag = json.loads(line)
                lags.append(lag)
                b = buckets.setdefault((source-self.epoch)//1_000_000_000, Counter())
                b.update(source_logs=1, source_bytes=901)
        native.dump(root/'rates-per-second.json', [{'second':k, **v} for k,v in sorted(buckets.items())])
        last = max(s['wall_ns'] for s in samples)
        rates = phase_rates(self.epoch, last, batch_sizes, events, root/'sources.jsonl.gz')
        visibility = {k:[] for k in ('source_ms','spool_observed_ms','ack_observed_ms')}
        missing_targets = 0
        for target in self.target_rows:
            if target['visible_ns'] is None:
                missing_targets += 1
                continue
            row = target['row']
            key = (row['node'], row['sequence'])
            target['spool_observed_ns'] = cycles[key]['t']
            target['ack_observed_ns'] = acks[key]['t']
            for metric, field in [('source_ms','source_ns'), ('spool_observed_ms','spool_observed_ns'), ('ack_observed_ms','ack_observed_ns')]:
                visibility[metric].append((target['visible_ns']-target[field])/1e6)
        query_stats = {}
        for shape in ('recent_logs', 'absent_text', 'cpu_metrics', 'visibility'):
            query_stats[shape] = {}
            for phase in ('all','normal','burst','recovery','drain'):
                population = [r for r in self.queries if r['shape']==shape and
                    (phase=='all' or classify(r['start_ns'],self.epoch)==phase)]
                query_stats[shape][phase] = {'latency_ms':native.percentile([r['elapsed_ms'] for r in population]),
                    'errors':sum(r['error'] is not None for r in population),
                    'schedule_lateness_ms':native.percentile([(r['start_ns']-r['scheduled_ns'])/1e6 for r in population]),
                    'response_json_bytes':sum(r['json_answer_bytes'] for r in population)}
        additions = {'producer_lag_p99_le_100ms':native.percentile(lags)['p99'] <= 100,
            'all_visibility_targets_returned':missing_targets==0 and len(self.target_rows)==36,
            'source_to_visible_le_30s':missing_targets==0 and max(visibility['source_ms'], default=float('inf')) <= 30000,
            'no_query_errors':all(r['error'] is None for r in self.queries),
            'no_incomplete_answers':all(r['answer'] and r['answer']['complete'] and not r['answer']['unavailable'] and not r['answer']['gaps'] for r in self.queries),
            'live_returned_logs_exact':not unmatched and wrong==0,
            'final_queries_exact':all(v['passed'] for v in verdicts),
            'query_negative_controls_rejected':all(not c['verdict']['passed'] for c in controls),
            'spool_cycle_custody':set(cycles)==set(batch_sizes)==set(acks)}
        summary['gates'].update(additions)
        summary['passed'] = all(summary['gates'].values())
        summary['observation'] = {'epoch_ns':self.epoch, 'cpu':cpu_windows(samples,self.epoch),
            'rates':rates, 'queries':query_stats,
            'visibility':{k:native.percentile(v) for k,v in visibility.items()},
            'visibility_missing':missing_targets, 'producer_lateness_ms':native.percentile(lags),
            'cgroup_peak_bytes':max(int(s['cgroup']['memory.peak']) for s in samples),
            'cgroup_sampled_current_peak_bytes':max(int(s['cgroup']['memory.current']) for s in samples),
            'cgroup_last':samples[-1]['cgroup'],
            'peak_pending_batches':max(sum(p['pending_batches'] for p in s['progress']) for s in samples),
            'peak_file_backlog_bytes':max(sum(p['file_backlog_bytes'] for p in s['progress']) for s in samples),
            'peak_spool_disk_bytes':max(s['spool_disk_bytes'] for s in samples),
            'progress_final':samples[-1]['progress'],
            'spool_append_attempt_acceptance_fraction':None,
            'spool_append_attempt_acceptance_note':'Success-only cycle output; no complete append attempt denominator.'}
        self.archive(root)
        native.dump(root/'summary.json', summary)


def controls():
    sha = hashlib.sha256(b'original').hexdigest()
    answer = dict(rows=[dict(body='original')], complete=True, unavailable=[], gaps=[], next_page=None)
    assert exact_live(answer, sha)
    for rows in ([], [dict(body='changed')], [dict(body='original')]*2):
        assert not exact_live(dict(answer, rows=rows), sha)
    class Gone:
        def is_file(self): return True
        def stat(self): raise FileNotFoundError('concurrent rename')
    class Inventory:
        def rglob(self, pattern): return [Gone()]
    assert native.footprint(Inventory()) == 0
    assert classify(60_000_000_000, 0)=='burst'
    assert classify(120_000_000_000, 0)=='recovery'
    # Origin: development observation's independent accounting check. A float
    # epoch rounded below the integer epoch, dropping second zero from normal.
    epoch = 1_791_150_000_000_000_123
    assert classify(epoch, epoch)=='normal'
    assert classify(epoch+59_999_999_999,epoch)=='normal'
    assert classify(epoch+60_000_000_000,epoch)=='burst'
    assert all(isinstance(t,int) for _,a,b in windows(epoch,epoch+200_000_000_000) for t in (a,b))
    assert native.compare({'tag':('sha',0,'normal')}, {})['missing']==1
    empty = dict(rows=[], complete=True, retained_from_ns=0, retained_to_ns=0,
                 freshness={}, gaps=[], unavailable=[], snapshot='control', next_page=None)
    assert native.query_oracle.check([], QUERY, [empty])['passed']
    return {'changed_missing_duplicate_sentinel_rejected':True, 'rename_inventory_safe':True,
            'half_open_phase_boundaries':True, 'oracle_interface':True, 'missing_source_rejected':True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tier', choices=['development','small'])
    parser.add_argument('--controls', action='store_true')
    args = parser.parse_args()
    require_limits()
    checks = controls()
    if args.controls:
        print(json.dumps(checks))
        return
    if not args.tier:
        parser.error('--tier or --controls required')
    cpus = sorted(os.sched_getaffinity(0))
    assert len(cpus)>=4
    data = ROOT/'docs/experiments/benchmarks/data/dev-small-observation-run-01'/args.tier
    work = Path(os.environ['TMPDIR'])/args.tier
    assert work.is_relative_to(STORAGE/'scratch') and not work.exists()
    data.mkdir(parents=True, exist_ok=False)
    bins = Path(os.environ['CARGO_TARGET_DIR'])/'release'
    binaries = [bins/'fabric-server', bins/'fabric-node', bins/'examples/server_dump']
    hashes = {str(p.relative_to(bins)):digest(p) for p in binaries}
    native.dump(data/'controls.json', checks)
    # An exact evidence copy is not another documentation page with relocated links.
    shutil.copy(PROTOCOL, data/'protocol.txt')
    (data/'working-tree.diff').write_bytes(subprocess.check_output(['git','diff','HEAD'], cwd=ROOT))
    with tarfile.open(data/'source-snapshot.tar.gz', 'x:gz') as archive:
        names = subprocess.check_output(['git','ls-files','-z','--cached','--others','--exclude-standard'],cwd=ROOT)
        for name in names.decode().split('\0'):
            p = ROOT/name
            if name and p.is_file() and not name.startswith('docs/experiments/benchmarks/data/') and (p.suffix in ('.rs','.py') or p.name in ('Cargo.toml','Cargo.lock')):
                archive.add(p, arcname=name)
    group = cgroup()
    native.dump(data/'environment.json', {'command':sys.argv, 'revision':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'protocol_sha256':digest(PROTOCOL), 'protocol_commit':'blocked: .git is read-only in session',
        'harness_sha256':digest(Path(__file__)), 'binaries':hashes, 'server_cpus':cpus[:2], 'client_cpus':cpus[2:4],
        'cgroup':str(group), 'limits':{n:(group/n).read_text().strip() for n in ['memory.max','memory.high','memory.swap.max','cpu.max']},
        'rustc':subprocess.check_output(['rustc','-Vv'],text=True), 'uname':list(os.uname()),
        'cpuinfo':Path('/proc/cpuinfo').read_text(), 'storage':str(work), 'free_bytes':shutil.disk_usage(STORAGE).free})
    observer = Observer()
    status = 'interrupted'
    began = time.time()
    try:
        os.sched_setaffinity(0, cpus[2:4])
        summary = native.trial(work, bins, args.tier, cpus[:2], cpus[2:4], observer)
        assert hashes == {str(p.relative_to(bins)):digest(p) for p in binaries}, 'binary changed during trial'
        status = 'passed' if summary['passed'] else 'failed'
    except Exception as exc:
        native.dump(data/'failure.json', {'error':repr(exc)})
        raise
    finally:
        if work.exists():
            observer.archive(work)
            for p in work.iterdir():
                if p.is_file() and (p.suffix in ('.json','.gz','.err','.out')):
                    shutil.copy(p, data/p.name)
            size = native.footprint(work)
            if status=='passed':
                shutil.rmtree(work)
            native.dump(data/'cleanup.json', {'status':status, 'work':str(work), 'removed':not work.exists(),
                'logical_bytes':size, 'elapsed_seconds':time.time()-began,
                'failed_work_moves_with_launcher_tmpdir':status!='passed'})
    if status!='passed':
        raise SystemExit(1)


if __name__=='__main__':
    main()
