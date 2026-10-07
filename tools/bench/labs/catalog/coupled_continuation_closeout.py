"""Readback, documentation and cleanup receipts for the continued experiments."""
import ast
import argparse
import gzip
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path
import coupled_overlap

import coupled_admit
from coupled_cleanup import inactive, validate_archive
from resource_group import require_limits

ROOT = Path(__file__).resolve().parents[4]
DATA = ROOT / 'docs/experiments/benchmarks/data'
COORD = DATA / 'lab-completion-run-01/coordinator'
ID = 'catalog-coupled-continuation-closeout-01'


def digest(path, decoded=False):
    with (gzip.open if decoded else open)(path, 'rb') as stream:
        value = hashlib.sha256()
        length = 0
        while block := stream.read(1024 * 1024):
            length += len(block)
            value.update(block)
        return value.hexdigest(), length


def check_retained(entry, path, canonical):
    if path.is_symlink() or canonical.is_symlink():
        raise RuntimeError('unexpected evidence symlink')
    if (path.stat().st_dev, path.stat().st_ino) != (canonical.stat().st_dev, canonical.stat().st_ino):
        raise RuntimeError('hardlink identity drift')
    if digest(path) != (entry['new_compressed_sha256'], entry['new_compressed_bytes']):
        raise RuntimeError('retained representation drift')
    if digest(path, entry['operation'] == 'decoded_gzip_hardlink') != (entry['decoded_sha256'], entry['decoded_bytes']):
        raise RuntimeError('retained decoded evidence drift')


def control():
    path = Path(os.environ['FABRIC_SCRATCH_ROOT']) / 'retained-control'
    path.write_bytes(b'unchanged retained bytes')
    sha, size = digest(path)
    entry = {'operation': 'exact_bytes_hardlink', 'new_compressed_sha256': sha,
             'new_compressed_bytes': size, 'decoded_sha256': sha, 'decoded_bytes': size}
    try:
        check_retained(entry, path, path)
        try:
            check_retained(dict(entry, new_compressed_sha256='MUTATION'), path, path)
        except RuntimeError:
            return {'unchanged_accepted': True, 'changed_hash_rejected': True}
        raise RuntimeError('retained readback accepted changed hash')
    finally:
        path.unlink()


def small_acceptance_bins():
    """Exploratory rates from retained client clocks, separate from offered load."""
    result = {}
    root = DATA / 'catalog-overlap-small-pair1-01'
    for mode in ('serial', 'overlap'):
        arm = root / mode
        with gzip.open(arm / 'source.jsonl.gz', 'rt') as stream:
            origin = min(json.loads(line)['offered_ns'] for line in stream)
        bins = {name: {key: 0 for key in ('spool_batches', 'spool_bytes', 'spool_logs',
                                        'acked_batches', 'acked_bytes', 'acked_logs')}
                for name in ('before_offer', '0_5s', '5_10s', '10_15s', '15_20s', 'after_20s')}
        def add(clock, kind, byte_count, logs):
            elapsed = (clock - origin) / 1e9
            name = ('before_offer' if elapsed < 0 else
                    ('0_5s', '5_10s', '10_15s', '15_20s')[int(elapsed // 5)] if elapsed < 20 else 'after_20s')
            row = bins[name]
            row[kind + '_batches'] += 1
            row[kind + '_bytes'] += byte_count
            row[kind + '_logs'] += logs
        for index in range(20):
            name = f'node{index:02}'
            events = [json.loads(line) for line in (arm / (name + '.stdout')).read_text().splitlines()]
            committed = {e['sequence']: e['unix_ns'] for e in events if e['event'] == 'commit'}
            acked = {e['sequence']: e['unix_ns'] for e in events
                     if e['event'] == 'attempt' and e['outcome'] == f"Ack({e['sequence']})"}
            with gzip.open(arm / (name + '-producer.jsonl.gz'), 'rt') as stream:
                for line in stream:
                    entry = json.loads(line)
                    raw = bytes.fromhex(entry['hex'])
                    batch = coupled_overlap.query_oracle.decode_batch(raw)
                    logs = len(coupled_overlap.query_oracle.decode_logs_request(batch['logs_bytes']))
                    add(committed[entry['sequence']], 'spool', len(raw), logs)
                    add(acked[entry['sequence']], 'acked', len(raw), logs)
        for name in ('0_5s', '5_10s', '10_15s', '15_20s'):
            row = bins[name]
            row.update({key + '_per_second': value / 5 for key, value in list(row.items())})
        result[mode] = {'origin_first_source_offer_ns': origin, 'bins': bins,
                        'scope': 'client-observed durable Spool/ACK clocks; metrics Batches included; five-second averages, not sustainable capacity'}
    return result


def provenance_context():
    source_protocol = ROOT / 'docs/experiments/benchmarks/coupled-reclamation-protocol.md'
    source_freeze = DATA / 'catalog-overlap-freeze-01/freeze.json'
    targets = [(source_freeze, COORD / 'catalog-coupled-reclaim-01/data/catalog-overlap-freeze-01/freeze.json'),
               (source_freeze, COORD / 'catalog-coupled-reclaim-02/data/catalog-overlap-freeze-01/freeze.json'),
               (source_protocol, COORD / 'catalog-coupled-reclaim-02/coupled-reclamation-protocol.md')]
    result = []
    for source, target in targets:
        if source.is_symlink() or target.is_symlink():
            raise RuntimeError('linked context provenance')
        raw = source.read_bytes()
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists():
            if target.read_bytes() != raw:
                raise RuntimeError('conflicting context provenance')
        else:
            with target.open('xb') as stream:
                stream.write(raw)
                stream.flush()
                os.fsync(stream.fileno())
        if target.read_bytes() != raw:
            raise RuntimeError('context provenance copy drift')
        result.append({'source': str(source.relative_to(ROOT)), 'target': str(target.relative_to(ROOT)),
                       'sha256': hashlib.sha256(raw).hexdigest(), 'bytes': len(raw), 'exact_readback': True})
    return result


def main():
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--id', choices=(ID, 'catalog-coupled-continuation-closeout-02'), default=ID)
    args = parser.parse_args()
    job_id = args.id
    coupled_admit.observe(2 * 1024**2, 0)
    out = DATA / job_id
    out.mkdir(exist_ok=False)
    context = provenance_context() if job_id.endswith('-02') else []
    negative_control = control()
    states = {}
    manifests = sorted(COORD.glob('catalog-coupled-reclaim-*/transformations.jsonl'))
    for transforms in manifests:
        for line in transforms.read_text().splitlines():
            entry = json.loads(line)
            key = (transforms.parent.name, entry['path'])
            if entry['state'] == 'prepared':
                if key in states:
                    raise RuntimeError('duplicate transformation preparation')
                states[key] = entry
            elif entry['state'] == 'replaced':
                prepared = dict(entry, state='prepared')
                if states.get(key) != prepared:
                    raise RuntimeError('replacement lacks matching durable preparation')
                states[key] = entry
            else:
                raise RuntimeError('unknown transformation state')
    for entry in states.values():
        if entry['state'] != 'replaced':
            raise RuntimeError('unfinished transformation')
        path, canonical = ROOT / entry['path'], ROOT / entry['canonical_path']
        check_retained(entry, path, canonical)
    start = json.loads((COORD / 'catalog-coupled-reclaim-01/receipt.json').read_text())['started_unix_ns']
    runs = []
    outer_dir = COORD / 'launcher-receipts'
    outer_dir.mkdir(exist_ok=True)
    for path in COORD.glob('*/receipt.json'):
        row = json.loads(path.read_text())
        if row['started_unix_ns'] < start or row['id'] == job_id:
            continue
        if row['state'] == 'running':
            raise RuntimeError('prior continuation job still running')
        unit = Path(row['cgroup']).name.removesuffix('.service')
        inactive(unit)
        outer = ROOT / 'target/resource-containment/runs' / (unit + '.json')
        raw = outer.read_bytes()
        target = outer_dir / (row['id'] + '.json')
        if target.exists() and target.read_bytes() != raw:
            raise RuntimeError('outer receipt changed')
        target.write_bytes(raw)
        if target.read_bytes() != raw:
            raise RuntimeError('receipt copy drift')
        failed = json.loads(raw).get('retained_failure_evidence')
        if failed and Path(failed).exists():
            raise RuntimeError('failed scratch remains unarchived: ' + row['id'])
        if failed:
            cleanup_reports = list((DATA / 'lab-completion-run-01').glob('catalog-coupled-continuation-cleanup-*.json'))
            verified = []
            for report_path in cleanup_reports:
                report = json.loads(report_path.read_text())
                if report.get('state') == 'completed':
                    verified.extend(r for r in report.get('jobs', [])
                                    if r.get('id') == row['id'] and r.get('byte_verified') and r.get('scratch_removed'))
            if len(verified) != 1:
                raise RuntimeError('missing unique verified failed-scratch cleanup: ' + row['id'])
            archive = DATA / 'lab-completion-run-01' / verified[0]['archive']
            manifest = json.loads(archive.with_suffix('.manifest.json').read_text())
            validate_archive(archive, manifest)
        if row.get('scratch') and Path(row['scratch']).exists():
            raise RuntimeError('owned job scratch remains: ' + row['id'])
        runs.append({key: row.get(key) for key in ('id', 'state', 'exit', 'elapsed_s')})
    syntax = []
    for path in sorted((ROOT / 'tools/bench/labs/catalog').glob('coupled_*.py')):
        ast.parse(path.read_text(), filename=str(path))
        syntax.append(str(path.relative_to(ROOT)))
    command = [sys.executable, '-B', 'tools/bench/labs/completion/checks.py',
               '--profile', 'documentation', '--id', job_id]
    status = subprocess.run(command).returncode
    result = {'transformation_readbacks': len(states), 'readback_control': negative_control,
              'provenance_context_copies': context,
              'manifests': [str(path.relative_to(ROOT)) for path in manifests], 'syntax_checked': syntax,
              'command': command, 'documentation_exit': status,
              'continuation_jobs': runs, 'owned_scratch_absent': True,
              'small_acceptance': small_acceptance_bins(),
              'inventory': coupled_admit.observe(0, 0),
              'historical_receipt_preservation': 'helpers target owned duplicate files; historical receipt hashes not independently baselined',
              'fast_checks': 'prior completion-fast-02; no Rust changes in this continuation',
              'limitation': 'current coordinator receipt finishes after this inventory'}
    (out / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result), flush=True)
    raise SystemExit(status)


if __name__ == '__main__':
    main()
