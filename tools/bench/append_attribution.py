#!/usr/bin/env python3
"""Run/validate the registered S0 paired append attribution experiment."""
import argparse
import csv
import hashlib
import json
import math
import os
from pathlib import Path
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
PHASES = ('encode', 'data_write', 'data_sync', 'marker_write', 'marker_sync')
HEADER = 'phase,event_index,elapsed_ns,events,full_rejections,file_bytes,peak_rss_kib,alloc_calls,alloc_requested_bytes'.split(',')
COUNT = 2000


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def validate(path, instrumented, count=COUNT):
    with path.open(newline='') as stream:
        reader = csv.DictReader(stream)
        if reader.fieldnames != HEADER:
            raise ValueError('unexpected CSV schema')
        rows = list(reader)
    allowed = {'append', 'ingest'} | (set(PHASES) if instrumented else set())
    if any(row['phase'] not in allowed or None in row or any(v is None for v in row.values()) for row in rows):
        raise ValueError('unexpected or malformed CSV row')
    ingestion = [r for r in rows if r['phase'] == 'ingest']
    if len(ingestion) != 1 or int(ingestion[0]['events']) != count:
        raise ValueError('wrong ingest count')
    metrics = {}
    for phase in ['append'] + (list(PHASES) if instrumented else []):
        samples = [r for r in rows if r['phase'] == phase]
        if [int(r['event_index']) for r in samples] != list(range(1, count + 1)):
            raise ValueError(f'missing, duplicate or out-of-order {phase} samples')
        metrics[phase] = [int(r['elapsed_ns']) for r in samples]
        if any(n < 0 for n in metrics[phase]):
            raise ValueError('negative elapsed time')
    if instrumented:
        for i in range(count):
            if sum(metrics[p][i] for p in PHASES) > metrics['append'][i]:
                raise ValueError('phase sum exceeds outer append')
    ingest_ns = int(ingestion[0]['elapsed_ns'])
    if ingest_ns <= 0 or sum(metrics['append']) > ingest_ns:
        raise ValueError('invalid pipeline interval')
    appends = sorted(metrics['append'])
    result = {
        'events': count, 'ingest_ns': ingest_ns, 'events_per_second': count * 1e9 / ingest_ns,
        'append_p50_ns': appends[math.ceil(count * .50) - 1],
        'append_p99_ns': appends[math.ceil(count * .99) - 1],
        'append_total_ns': sum(appends),
    }
    for field in ['full_rejections', 'file_bytes', 'peak_rss_kib', 'alloc_calls', 'alloc_requested_bytes']:
        result[field] = int(ingestion[0][field]) if ingestion[0][field] else None
    if instrumented:
        result['phase_totals_ns'] = {p: sum(metrics[p]) for p in PHASES}
        result['phase_fractions'] = {p: sum(metrics[p]) / sum(appends) for p in PHASES}
        result['sync_fraction'] = sum(result['phase_fractions'][p] for p in ('data_sync', 'marker_sync'))
    return result


def record_run(command, out, name):
    start = time.monotonic_ns()
    with (out / (name + '.stdout')).open('wb') as stdout, (out / (name + '.stderr')).open('wb') as stderr:
        process = subprocess.Popen(command, cwd=ROOT, stdout=stdout, stderr=stderr)
        _, status, usage = os.wait4(process.pid, 0)
        process.returncode = os.waitstatus_to_exitcode(status)
    record = {'command': [str(x) for x in command], 'exit': process.returncode,
              'wall_ns': time.monotonic_ns() - start,
              'user_cpu_s': usage.ru_utime, 'system_cpu_s': usage.ru_stime,
              'max_rss_kib': usage.ru_maxrss, 'input_blocks': usage.ru_inblock,
              'output_blocks': usage.ru_oublock}
    with (out / 'commands.jsonl').open('a') as stream:
        stream.write(json.dumps({'name': name, **record}) + '\n')
    if process.returncode:
        raise RuntimeError(f'{name} exited {process.returncode}; see preserved stderr')
    return record


def verify_output(path, log):
    with path.open(newline='') as stream:
        reader = csv.DictReader(stream)
        if reader.fieldnames != HEADER:
            raise ValueError('unexpected verify schema')
        rows = list(reader)
    if [r['phase'] for r in rows] != ['open', 'replay']:
        raise ValueError('missing verify phases')
    if any(int(r['events']) != COUNT or int(r['file_bytes']) != log.stat().st_size for r in rows):
        raise ValueError('wrong verified count or size')


def summarize(out):
    trials = []
    hashes = set()
    for trial in ['warmup', '1', '2', '3', '4', '5', 'alloc']:
        pair = {}
        for variant in ['default', 'attribution']:
            stem = f'{trial}-{variant}'
            log = out / (stem + '.fol2')
            result = validate(out / (stem + '-write.stdout'), variant == 'attribution')
            verify_output(out / (stem + '-verify.stdout'), log)
            if result['file_bytes'] != log.stat().st_size:
                raise ValueError('wrong stored byte count')
            hashes.add(digest(log))
            pair[variant] = result
        trials.append({'trial': trial, **pair})
    if len(hashes) != 1:
        raise ValueError('default and feature logs differ')
    measured = [r for r in trials if r['trial'].isdigit()]
    relative = [r['attribution']['ingest_ns'] / r['default']['ingest_ns'] - 1 for r in measured]
    median = statistics.median(relative)
    return {'trials': trials, 'paired_relative_pipeline_change': relative,
            'median_relative_pipeline_change': median, 'material_perturbation': abs(median) > .10,
            'sync_dominates_each_trial': all(r['attribution']['sync_fraction'] > .50 for r in measured),
            'log_sha256': hashes.pop(), 'correctness': 'all exact replay, counts and identical file hashes passed'}


def run(out):
    out.mkdir(parents=True, exist_ok=False)
    sources = ['Cargo.toml','Cargo.lock','src/log.rs','src/generator.rs','src/buffer.rs',
               'examples/local_log_probe.rs','tools/bench/append_attribution.py',
               'docs/experiments/benchmarks/append-attribution-s0.md']
    environment = {'source_hashes': {p: digest(ROOT / p) for p in sources},
                   'RUSTFLAGS': os.environ.get('RUSTFLAGS'), 'utc': time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime())}
    for key, cmd in [('head',['git','rev-parse','HEAD']), ('status',['git','status','--short']),
                     ('kernel',['uname','-a']), ('cpu',['lscpu']), ('mount',['findmnt','-T',str(ROOT),'-o','SOURCE,FSTYPE,OPTIONS','-n']),
                     ('rustc',['rustc','--version']), ('cargo',['cargo','--version'])]:
        result = subprocess.run(cmd, cwd=ROOT, text=True, capture_output=True)
        environment[key] = {'command':cmd,'exit':result.returncode,'stdout':result.stdout,'stderr':result.stderr}
    (out/'environment.json').write_text(json.dumps(environment,indent=2)+'\n')
    binaries = {}
    for name, features in [('default', []), ('attribution',['append-attribution']),
                           ('alloc-default',['stage6-alloc-probe']), ('alloc-attribution',['stage6-alloc-probe','append-attribution'])]:
        target = ROOT / 'target' / 'append-attribution-build' / name
        cmd = ['cargo','build','--release','--offline','--locked','--example','local_log_probe','--target-dir',str(target)]
        if features:
            cmd += ['--features',','.join(features)]
        record_run(cmd,out,'build-'+name)
        binaries[name] = target/'release/examples/local_log_probe'
    for trial in ['warmup', '1', '2', '3', '4', '5', 'alloc']:
        variants = ['default','attribution']
        if trial.isdigit() and int(trial)%2 == 0:
            variants.reverse()
        for variant in variants:
            binary = binaries[('alloc-' if trial == 'alloc' else '')+variant]
            stem = f'{trial}-{variant}'
            log = out/(stem+'.fol2')
            for mode in ['write','verify']:
                record_run([str(binary),mode,str(log),'42',str(COUNT)],out,stem+'-'+mode)
    summary = summarize(out)
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    (out/'artifacts.json').write_text(json.dumps({p.name:digest(p) for p in sorted(out.iterdir()) if p.is_file() and p.name!='artifacts.json'},indent=2)+'\n')
    print(json.dumps({k:v for k,v in summary.items() if k!='trials'},indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode',choices=['run','summarize','validate'])
    parser.add_argument('path',type=Path)
    parser.add_argument('--instrumented',action='store_true')
    parser.add_argument('--count',type=int,default=COUNT)
    args = parser.parse_args()
    try:
        if args.mode == 'run':
            run(args.path.resolve())
        elif args.mode == 'summarize':
            print(json.dumps(summarize(args.path.resolve()),indent=2))
        else:
            print(json.dumps(validate(args.path,args.instrumented,args.count)))
    except (ValueError,RuntimeError,OSError,KeyError) as error:
        print(str(error),file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
