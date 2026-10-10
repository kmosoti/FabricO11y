#!/usr/bin/env python3
"""Sequential bounded-builder memory and pending-journal query screens."""
import argparse
import base64
import copy
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
import tarfile

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / 'tools'))
from resource_group import require_limits, STORAGE
sys.path.insert(0, str(REPO / 'tools/qualification'))
import query_oracle


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024**2), b''):
            h.update(chunk)
    return h.hexdigest()


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def grade_first_page(expected, query, answer):
    query_oracle._validate_pages_shape([answer], query)
    violations = query_oracle._check_envelope(expected, [answer], query)
    rows = expected['rows'][:query['limit']]
    if len(answer['rows']) != len(rows):
        violations.append(('FIRST-PAGE-LENGTH', 'wrong first-page row count'))
    for wanted, actual in zip(rows, answer['rows']):
        if not query_oracle._row_equal(wanted, actual, query['kind']):
            violations.append(('FIRST-PAGE-ROW', 'row content/order differs'))
    if bool(answer['next_page']) != (len(expected['rows']) > query['limit']):
        violations.append(('FIRST-PAGE-NEXT', 'wrong continuation presence'))
    return {'passed':not violations, 'violations':violations, 'observed_rows':len(answer['rows'])}


def run(dest, binary, mode, count, variant, repeat):
    label = f'{dest.name}-{mode}-{count}-{variant}-{repeat}'
    work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / ('ingestion-' + label)
    evidence = dest / label
    assert not work.exists() and not evidence.exists(), label
    work.mkdir(); evidence.mkdir()
    print('start', label, flush=True)
    argv = [str(binary), str(work), mode, '1024']
    env = dict(os.environ, BENCH_RECORDS=str(count), BENCH_BODY_SIZE='1024',
               BENCH_ORDER='shuffled', BENCH_BUILDER=variant, BENCH_PHASES='0',
               BENCH_OBSERVER='detailed', BENCH_PREFLIGHT='0')
    dump(evidence / 'command.json', {'argv': argv, 'variant': variant,
         'repeat': repeat, 'env': {k:v for k,v in env.items() if k.startswith('BENCH_')},
         'binary_sha256': digest(binary), 'revision': subprocess.check_output(
             ['git','rev-parse','HEAD'], cwd=REPO, text=True).strip()})
    status = None
    began = time.monotonic()
    try:
        with (evidence/'stdout.jsonl').open('w') as out, (evidence/'stderr.txt').open('w') as err:
            status = subprocess.run(argv, env=env, stdout=out, stderr=err, timeout=300).returncode
        assert status == 0, f'probe exit {status}'
        events = [json.loads(line) for line in (evidence/'stdout.jsonl').read_text().splitlines()]
        assert events[-1]['stage'] == 'complete'
        measurement = next(e for e in events if e['stage'] in (
            'read_and_build_segment', 'pending_read_and_build_segment'))
        a = measurement['allocation']
        summary = {'variant': variant, 'mode': mode, 'count':count, 'repeat':repeat,
                   'measurement':measurement, 'whole_process_rss_peak_kib':events[-1]['vm_hwm_kib'],
                   'incremental_heap_bytes': None if mode == 'pending' else a['incremental_peak_bytes'],
                   'elapsed_s':time.monotonic()-began, 'exit':status}
        summary['exactness'] = next(e for e in events if e['stage'] in ('memory_exactness','pending_exactness'))
        if mode == 'pending':
            records = []
            with (work/'records.jsonl').open() as stream:
                for line in stream:
                    record = json.loads(line)
                    raw = bytes.fromhex(record['hex'])
                    assert hashlib.sha256(raw).hexdigest() == record['sha256']
                    records.append({'label':record['label'], 'received_ns':record['received_ns'],
                                    'bytes':base64.b64encode(raw).decode()})
            verdicts = []
            expected_cache = {}
            controls = []
            with (work/'pending-answers.jsonl').open() as stream:
                for i, line in enumerate(stream):
                    answer = json.loads(line)
                    key = json.dumps(answer['query'], sort_keys=True)
                    if key not in expected_cache:
                        expected_cache[key] = query_oracle.expected(records, answer['query'])
                    expected = expected_cache[key]
                    verdict = grade_first_page(expected, answer['query'], answer['answer'])
                    verdicts.append({'index':i, 'phase':answer['phase'], 'plan':answer['plan'],
                                     'wall_ns':answer['wall_ns'], 'verdict':verdict})
                    assert verdict['passed'], verdict
                    if not controls and answer['answer']['rows']:
                        for name in ['changed','missing','duplicate']:
                            bad = copy.deepcopy(answer['answer'])
                            if name=='changed': bad['rows'][0]['body'] += 'changed'
                            elif name=='missing': bad['rows'] = bad['rows'][1:]
                            else: bad['rows'].append(copy.deepcopy(bad['rows'][0]))
                            assert not grade_first_page(expected, answer['query'], bad)['passed'], name
                            controls.append(name)
            summary['query_verdicts'] = verdicts
            summary['negative_controls_rejected'] = controls
            shutil.copyfile(work/'pending-answers.jsonl', evidence/'pending-answers.jsonl')
        archive_root = STORAGE / 'evidence/ingestion-memory-fixtures'
        archive_root.mkdir(parents=True, exist_ok=True)
        source_hash = digest(work/'records.jsonl')
        archive = archive_root / (source_hash + '.jsonl.gz')
        if not archive.exists():
            with (work/'records.jsonl').open('rb') as source, gzip.open(archive, 'wb') as output:
                shutil.copyfileobj(source, output, 1024**2)
        summary['fixture_archive'] = str(archive)
        summary['fixture_sha256'] = source_hash
        dump(evidence/'summary.json', summary)
        print('done', label, 'heap', summary['incremental_heap_bytes'],
              'wall_ns', measurement['wall_ns'], flush=True)
        return summary
    except BaseException as error:
        dump(evidence/'failure.json', {'error':str(error), 'exit':status})
        retained = STORAGE / 'evidence' / ('failed-' + label)
        assert not retained.exists()
        work.rename(retained)
        dump(evidence/'retained-failure.json', {'path':str(retained)})
        raise
    finally:
        if work.exists():
            size = sum(p.stat().st_size for p in work.rglob('*') if p.is_file())
            shutil.rmtree(work)
            dump(evidence/'cleanup.json', {'logical_bytes':size, 'removed':not work.exists()})


def main():
    require_limits()
    parser = argparse.ArgumentParser()
    parser.add_argument('--phase', choices=['preflight','memory','pending'], required=True)
    parser.add_argument('--destination', type=Path, required=True)
    args = parser.parse_args()
    args.destination.mkdir(parents=True, exist_ok=True)
    binary = Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/responsibility_probe'
    # Preserve the actual working tree used by the frozen binary.
    (args.destination/'working-tree.diff').write_bytes(subprocess.check_output(['git','diff','HEAD'],cwd=REPO))
    with tarfile.open(args.destination/'source-snapshot.tar.gz', 'x:gz') as archive:
        listing = subprocess.check_output(['git','ls-files','-z','--cached','--others','--exclude-standard'],cwd=REPO)
        for name in listing.decode().split('\0'):
            p = REPO / name
            if name and p.is_file() and (p.suffix == '.rs' or p.name in ('Cargo.toml','Cargo.lock')):
                archive.add(p, arcname=name)
    results = []
    counts = [4096] if args.phase == 'preflight' else ([16384,65536,262144] if args.phase=='memory' else [65536])
    for count in counts:
        modes = ['memory','pending'] if args.phase=='preflight' else [args.phase]
        for mode in modes:
            for repeat in range(1, (1 if args.phase=='preflight' else 3)+1):
                for variant in (['reference','bounded'] if repeat%2 else ['bounded','reference']):
                    results.append(run(args.destination, binary, mode, count, variant, repeat))
    dump(args.destination/'complete.json', {'exit':0, 'trials':len(results), 'results':results})


if __name__ == '__main__':
    main()
