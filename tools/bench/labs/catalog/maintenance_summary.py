"""Read-only CQ2 completeness, retained-byte and raw-timing consolidation."""
import argparse
from collections import Counter, OrderedDict
import gzip
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits
sys.path.insert(0, str(ROOT / 'tools/qualification'))
import query_oracle

START = 1_600_000_000_000_000_000
SLICES = ('preflight', 'diagnostic', 'pair1', 'pair2', 'pair3')


def check(actual, expected, name):
    if actual != expected:
        raise RuntimeError(name + ' differs')


def load(path):
    if path.exists():
        return json.loads(path.read_text())
    with gzip.open(path.with_suffix(path.suffix + '.gz'), 'rt') as stream:
        return json.load(stream)


def sha(path, compressed=False):
    digest, size = hashlib.sha256(), 0
    with (gzip.open if compressed else open)(path, 'rb') as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
            size += len(chunk)
    return digest.hexdigest(), size


def matrix(keys):
    check(len(keys), 15, 'slice count')
    check(len(set(keys)), 15, 'unique slices')
    check(set(keys), {(rate, stage) for rate in (0, 1, 4) for stage in SLICES}, 'slice matrix')


def schedule(stage):
    if stage == 'preflight':
        return [('plain', 128, False, 0), ('plain', 128, True, 0), ('plain', 128, True, 32)]
    if stage == 'diagnostic':
        return [('counted', 65536, False, 0), ('counted', 65536, True, 0)]
    return [('plain', 65536, eager, 0) for eager in ([True, False] if stage == 'pair2' else [False, True])]


def header(row, expected, rate, binary):
    index, variant, seed, eager, skip = expected
    label = f'{index}-{variant}-{seed}-{"eager" if eager else "lazy"}-skip{skip}'
    for key, value in {'label': label, 'eager': eager, 'seed_records': seed, 'rate': rate,
                       'skip': skip, 'answers': 32 * rate + 2, 'binary_sha256': binary,
                       'control_count': 6}.items():
        check(row[key], value, 'trial ' + key)


def answer_ids(maps, expected):
    check(len(maps), expected, 'actual complete oracle count')
    for i, mapping in enumerate(maps):
        check(mapping['id'], i, 'unique ordered answer ID')
        check(mapping['verdict']['passed'], True, 'complete oracle verdict')


class Objects:
    def __init__(self):
        self.cache = OrderedDict()
        self.bytes = 0
        self.readback_count = 0

    def read(self, folder, name):
        if Path(name).name != name:
            raise RuntimeError('object path is not a basename')
        path = (folder / name).resolve(strict=True)
        if not path.is_relative_to(ROOT / 'docs/experiments/benchmarks/data'):
            raise RuntimeError('object outside retained repository evidence')
        if path in self.cache:
            self.cache.move_to_end(path)
            return self.cache[path]
        with gzip.open(path, 'rb') as stream:
            data = stream.read(4 * 1024**2 + 1)
        if len(data) > 4 * 1024**2:
            raise RuntimeError('retained object exceeds bounded decoder')
        check(hashlib.sha256(data).hexdigest(), name.split('.')[0], 'object decoded SHA')
        self.readback_count += 1
        while self.cache and self.bytes + len(data) > 96 * 1024**2:
            _, old = self.cache.popitem(last=False)
            self.bytes -= len(old)
        self.cache[path] = data
        self.bytes += len(data)
        return data

    def mapping(self, folder, obj):
        prefix, suffix = bytes.fromhex(obj['prefix_hex']), bytes.fromhex(obj['suffix_hex'])
        digest, size = hashlib.sha256(prefix), len(prefix)
        for name in obj['chunks']:
            data = self.read(folder, name)
            if len(data) > 65536:
                raise RuntimeError('row chunk exceeds declared size')
            digest.update(data)
            size += len(data)
        digest.update(suffix)
        size += len(suffix)
        check((digest.hexdigest(), size), (obj['raw_sha256'], obj['raw_bytes']), 'whole native JSON hash/size')
        # Payload validity/meaning was checked by the original full oracle.
        # Recheck exact payload bytes and parse the independent envelope cheaply.
        return json.loads(prefix + b'[]' + suffix)


def timings(rows, rate, eager, skip, counted):
    expected = {'append_ack': 32, 'refresh': 32 - int(skip == 32) if eager else 0,
                'query': 32 * rate, 'serialize': 32 * rate, 'total_schedule': 1}
    check(Counter(row['stage'] for row in rows), Counter({k: v for k, v in expected.items() if v}), 'phase counts')
    totals = [row for row in rows if row['stage'] == 'total_schedule']
    total = totals[0]
    for key, value in {'appends': 32, 'queries': 32 * rate, 'refreshes': expected['refresh'],
                       'includes': 'append_ack+refresh+query+serialize+bookkeeping'}.items():
        check(total[key], value, 'schedule ' + key)
    phases = {name: {'calls': expected[name], 'cpu_ns': 0, 'wall_ns': 0,
                     'allocated_bytes': 0 if counted else None,
                     'maximum_phase_peak_increment_bytes': 0 if counted else None}
              for name in ('append_ack', 'refresh', 'query', 'serialize')}
    for row in rows:
        for metric in ('cpu_ns', 'wall_ns'):
            if type(row[metric]) is not int or row[metric] < 0:
                raise RuntimeError('invalid raw native timing')
        if row['stage'] == 'total_schedule':
            continue
        phase = phases[row['stage']]
        for metric in ('cpu_ns', 'wall_ns'):
            phase[metric] += row[metric]
        allocation = row['allocation']
        if counted:
            if not isinstance(allocation, dict) or any(type(allocation[k]) is not int for k in ('live_delta', 'peak_increment', 'allocated_bytes')):
                raise RuntimeError('counted allocation evidence missing')
            if allocation['peak_increment'] < 0 or allocation['allocated_bytes'] < 0:
                raise RuntimeError('negative allocation count')
            phase['allocated_bytes'] += allocation['allocated_bytes']
            phase['maximum_phase_peak_increment_bytes'] = max(phase['maximum_phase_peak_increment_bytes'], allocation['peak_increment'])
        elif allocation is not None:
            raise RuntimeError('plain binary has counted allocation ledger')
    for metric in ('cpu_ns', 'wall_ns'):
        if sum(p[metric] for p in phases.values()) > total[metric]:
            raise RuntimeError('nonoverlapping phase sum exceeds total schedule')
    if total['wall_ns'] <= 0 or total['cpu_ns'] <= 0:
        raise RuntimeError('zero schedule denominator')
    return {'total': total, 'phases': phases,
            'schedule_acks_per_second': 32 * 1e9 / total['wall_ns']}


def controls():
    valid = [(r, s) for r in (0, 1, 4) for s in SLICES]
    matrix(valid)
    row = {'label': '0-plain-65536-lazy-skip0', 'eager': False, 'seed_records': 65536,
           'rate': 1, 'skip': 0, 'answers': 34, 'binary_sha256': 'fixed', 'control_count': 6}
    header(row, (0, 'plain', 65536, False, 0), 1, 'fixed')
    changed = dict(row, answers=33)
    provenance = dict(row, binary_sha256='changed')
    answers = [{'id': i, 'verdict': {'passed': True}} for i in range(2)]
    answer_ids(answers, 2)
    raw_rows = [{'stage': 'append_ack', 'cpu_ns': 1, 'wall_ns': 1, 'allocation': None} for _ in range(32)]
    total = {'stage': 'total_schedule', 'cpu_ns': 33, 'wall_ns': 33, 'appends': 32,
             'queries': 0, 'refreshes': 0, 'includes': 'append_ack+refresh+query+serialize+bookkeeping'}
    timings([*raw_rows, total], 0, False, 0, False)

    class MemoryObjects(Objects):
        def read(self, folder, name):
            return b'[]'

    payload = b'{"rows":[]}'
    mapped = {'prefix_hex': b'{"rows":'.hex(), 'suffix_hex': b'}'.hex(), 'chunks': ['rows'],
              'raw_sha256': hashlib.sha256(payload).hexdigest(), 'raw_bytes': len(payload)}
    decoder = MemoryObjects()
    decoder.mapping(Path('.'), mapped)
    changed_map = dict(mapped, raw_sha256='changed')
    missing_map = dict(mapped, chunks=[])
    rejected = {}
    for name, fn in [('missing_slice', lambda: matrix(valid[:-1])),
                     ('duplicate_slice', lambda: matrix(valid[:-1] + [valid[0]])),
                     ('wrong_answer_count', lambda: header(changed, (0, 'plain', 65536, False, 0), 1, 'fixed')),
                     ('changed_provenance', lambda: header(provenance, (0, 'plain', 65536, False, 0), 1, 'fixed')),
                     ('missing_oracle_verdict', lambda: answer_ids(answers[:-1], 2)),
                     ('duplicate_oracle_verdict', lambda: answer_ids(answers[:-1] + [answers[0]], 2)),
                     ('missing_append_timing', lambda: timings([*raw_rows[:-1], total], 0, False, 0, False)),
                     ('wrong_raw_refresh_count', lambda: timings([*raw_rows, dict(total, refreshes=1)], 0, False, 0, False)),
                     ('changed_reconstruction_hash', lambda: decoder.mapping(Path('.'), changed_map)),
                     ('missing_reconstruction_chunk', lambda: decoder.mapping(Path('.'), missing_map))]:
        try:
            fn()
        except RuntimeError:
            rejected[name] = True
        else:
            raise RuntimeError('summary accepted ' + name)
    return rejected


def ledger(folder, target, seed, objects, identities, validated):
    name = load(target / 'ledger-map.json')
    index = json.loads(objects.read(folder, name))
    check(len(index['frames']), seed // 128 + 32, 'recovered Batch count')
    raw_digest, raw_size = hashlib.sha256(), 0
    for sequence, frame in enumerate(index['frames'], 1):
        payload = objects.read(folder, frame['object'])
        raw = bytes.fromhex(frame['prefix_hex']) + payload + bytes.fromhex(frame['suffix_hex'])
        raw_digest.update(raw)
        raw_size += len(raw)
        record = json.loads(raw)
        batch_bytes = bytes.fromhex(record['hex'])
        digest = hashlib.sha256(batch_bytes).hexdigest()
        check(digest, record['sha256'], 'actual Batch digest')
        check(record['label'], 'fixture', 'actual credential label')
        check(digest, identities.setdefault(sequence, digest), 'cross-trial actual custody bytes')
        if digest not in validated:
            batch = query_oracle.decode_batch(batch_bytes)
            for key, value in {'version': 1, 'node_id': bytes([7]) * 16, 'generation': 1,
                               'sequence': sequence, 'gaps': [], 'metrics_bytes': b'', 'traces_bytes': b''}.items():
                check(batch[key], value, 'actual fixture ' + key)
            logs = query_oracle.decode_logs_request(batch['logs_bytes'])
            check(len(logs), 128, 'logs per Batch')
            for position, log in enumerate(logs):
                check(log['observed_time_unix_nano'], START + (sequence - 1) * 128 + position, 'chronological fixture')
                check(len(log['body'].encode()), 1024, 'body length')
                check(log['attributes'], {}, 'fixture attributes')
            validated.add(digest)
    check((raw_digest.hexdigest(), raw_size), (index['raw_sha256'], index['raw_bytes']), 'exact recovered ledger')


def trial(target, row, expected, rate, frozen, objects, identities, validated):
    index, variant, seed, eager, skip = expected
    header(row, expected, rate, frozen['binaries'][variant]['decoded_sha256'])
    check(load(target / 'result.json'), row, 'slice/trial summary association')
    native = load(target / 'stdout.jsonl')
    refreshes = 32 - int(skip == 32) if eager else 0
    for key, value in {'complete': True, 'eager': eager, 'seed': 42, 'seed_records': seed,
                       'seed_segments': 64 if seed == 65536 else 1, 'append_records': 4096,
                       'query_rate': rate, 'refreshes': refreshes, 'skip_refresh_at': skip,
                       'borrowed_logs': False, 'shared_catalog': False, 'counted': variant == 'counted',
                       'answers': 32 * rate + 2}.items():
        check(native[key], value, 'native ' + key)
    commands = load(target / 'commands.json')
    check(commands['all_exit_status'], 0, 'native/drain exits')
    check(len(commands['commands']), 32 * rate + 3, 'native plus all drains')
    check(commands['commands'][0][2:], ['eager' if eager else 'lazy', str(rate)], 'schedule command')
    for i, command in enumerate(commands['commands'][1:]):
        check(command[2:], ['drain', str(i)], 'drain association')
    for key, value in {'BENCH_SEED_RECORDS': str(seed), 'BENCH_SKIP_REFRESH_AT': str(skip),
                       'BENCH_PHASES': '0', 'BENCH_SHARED_CATALOG': '0', 'BENCH_EMPTY_TEXT': '0',
                       'BENCH_QUERY_LIMIT': '10000', 'BENCH_ORDER': 'sorted', 'BENCH_OBSERVER': 'minimal'}.items():
        check(commands['environment'][key], value, 'native environment ' + key)
    rejected = load(target / 'controls.json')
    check(set(rejected), {'changed', 'missing', 'duplicate', 'archive_changed_framing',
                         'archive_missing_chunk', 'archive_duplicate_chunk'}, 'negative-control coverage')
    for name, verdict in rejected.items():
        check(verdict.get('rejected') if name.startswith('archive_') else verdict.get('passed'),
              True if name.startswith('archive_') else False, 'negative control ' + name)
    folder = target.parent / 'objects'
    ledger(folder, target, seed, objects, identities, validated)
    maps = load(target / 'answer-map.json')
    answer_ids(maps, 32 * rate + 2)
    for i, mapping in enumerate(maps):
        check(mapping['id'], i, 'unique ordered answer ID')
        quiet = i >= 32 * rate
        append = 32 if quiet else i // rate + 1
        newest = seed // 128 + append
        check(mapping['quiet'], quiet, 'quiet control association')
        check(mapping['prefix_batches'], newest, 'oracle committed prefix')
        shape = 'broad' if (i - 32 * rate if quiet else i) % 2 == 0 else 'selective'
        query = {'kind': 'logs', 'from_ns': START, 'to_ns': START + seed + 4096, 'limit': 10000}
        if shape == 'selective':
            query['contains'] = 'bench-0007 '
        wrapper = objects.mapping(folder, mapping['wrapper'])
        for key, value in {'id': i, 'shape': shape, 'query': query, 'newest': newest, 'records': newest * 128}.items():
            check(wrapper[key], value, 'retained measured wrapper ' + key)
        expected_rows = newest * 128 if shape == 'broad' else 1
        verdict = mapping['verdict']
        for key, value in {'passed': True, 'violations': [], 'expected_rows': expected_rows,
                           'answered_rows': expected_rows}.items():
            check(verdict[key], value, 'retained full oracle ' + key)
        check(len(mapping['pages']), (expected_rows + 9999) // 10000, 'complete pagination count')
        for page_index, page in enumerate(mapping['pages']):
            envelope = objects.mapping(folder, page)
            check(envelope['complete'], True, 'complete page envelope')
            check(envelope['snapshot'], f'g1-{newest}', 'snapshot')
            check(envelope['next_page'] is None, page_index == len(mapping['pages']) - 1, 'terminal continuation')
            if page_index == 0:
                check(envelope, wrapper['answer'], 'exact first-page envelope')
                check(page['chunks'], mapping['wrapper']['chunks'], 'exact first-page row bytes')
                framed = mapping['wrapper']
                prefix = bytes.fromhex(framed['prefix_hex'])
                suffix = bytes.fromhex(framed['suffix_hex'])
                empty = prefix + b'[]' + suffix
                text = empty.decode('utf-8')
                start = text.index('"answer":') + len('"answer":')
                _, end = json.JSONDecoder().raw_decode(text, start)
                begin_byte, end_byte = len(text[:start].encode()), len(text[:end].encode())
                check(bytes.fromhex(page['prefix_hex']), prefix[begin_byte:], 'exact first-page prefix bytes')
                check(bytes.fromhex(page['suffix_hex']), empty[len(prefix) + 2:end_byte], 'exact first-page suffix bytes')
    if (target / 'compaction.json').exists():
        for entry in load(target / 'compaction.json'):
            check(entry['exact_readback_compared'], True, 'compaction readback')
            check(entry['original_removed'], True, 'compaction completion')
            path = ROOT / entry['retained_path']
            check(sha(path, True), (entry['decoded_sha256'], entry['decoded_bytes']), 'compacted decoded bytes')
            check(sha(path), (entry['new_compressed_sha256'], entry['new_compressed_bytes']), 'compacted archive bytes')
    rows = [json.loads(line) for line in (target / 'timings.jsonl').read_text().splitlines()]
    measured = timings(rows, rate, eager, skip, variant == 'counted')
    check(measured['total'], row['total'], 'raw timing/summary total')
    return {'label': row['label'], 'rate': rate, 'variant': variant, 'eager': eager,
            'seed_records': seed, 'skip': skip, 'oracle_chains': len(maps), **measured}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--slices', type=Path, nargs=15, required=True)
    parser.add_argument('--freeze', type=Path, required=True)
    parser.add_argument('--out', type=Path)
    args = parser.parse_args()
    require_limits()
    frozen = load(args.freeze / 'freeze.json')
    check(frozen['status'], 'complete', 'freeze completion')
    check(load(args.freeze / 'cleanup.json')['removed'], True, 'freeze cleanup')
    check(frozen['environment']['FABRIC_BORROWED_LOG_EXPERIMENT'], '0', 'frozen borrowed selector')
    check(set(frozen['binaries']), {'plain', 'counted'}, 'frozen variant set')
    for variant, identity in frozen['binaries'].items():
        archive = args.freeze / identity['archive']
        check(sha(archive)[0], identity['archive_sha256'], 'frozen compressed binary')
        check(sha(archive, True), (identity['decoded_sha256'], identity['decoded_bytes']), 'frozen executable bytes')
    objects, identities, validated, keys, trials = Objects(), {}, set(), [], []
    frozen_hashes = {k: v['decoded_sha256'] for k, v in frozen['binaries'].items()}
    for directory in args.slices:
        env = load(directory / 'environment.json')
        check(env['freeze_directory'], str(args.freeze.resolve()), 'frozen origin')
        check(env['freeze_receipt'], frozen, 'exact freeze receipt')
        check(env['binary_hashes'], frozen_hashes, 'slice binary provenance')
        check(env['oracle_sha256'], frozen['source_hashes']['tools/qualification/query_oracle.py'], 'oracle provenance')
        for name, digest in env['source_hashes_at_launch'].items():
            check(digest, frozen['source_hashes'][name], 'frozen source ' + name)
        argv = env['driver_argv']
        stage = argv[argv.index('--slice') + 1]
        rate = int(argv[argv.index('--rate') + 1])
        if stage not in SLICES or rate not in (0, 1, 4):
            raise RuntimeError('unexpected slice selectors')
        keys.append((rate, stage))
        cleanup = load(directory / 'cleanup.json')
        check((cleanup['status'], cleanup['removed']), ('complete', True), 'slice cleanup')
        rows = load(directory / 'results.json')
        expected = schedule(stage)
        check(len(rows), len(expected), 'slice trial count')
        for i, (row, item) in enumerate(zip(rows, expected)):
            result = trial(directory / row['label'], row, (i, *item), rate, frozen, objects, identities, validated)
            result['slice'] = stage
            trials.append(result)
    matrix(keys)
    check(len(trials), 33, 'all completed trials')
    check(sum(t['oracle_chains'] for t in trials), 1826, 'all actual complete oracle chains')
    rates = {}
    for rate in (0, 1, 4):
        pairs = []
        for stage in ('pair1', 'pair2', 'pair3'):
            pair = {t['eager']: t for t in trials if t['rate'] == rate and t['slice'] == stage}
            ratios = {metric: pair[True]['total'][metric] / pair[False]['total'][metric]
                      for metric in ('cpu_ns', 'wall_ns')}
            pairs.append({'pair': stage, 'eager_over_lazy': ratios,
                          'lazy_schedule_acks_per_second': pair[False]['schedule_acks_per_second'],
                          'eager_schedule_acks_per_second': pair[True]['schedule_acks_per_second']})
        rates[str(rate)] = {'plain_pairs': pairs, 'H1_supported': all(p['eager_over_lazy']['cpu_ns'] < 1 for p in pairs),
                            'decision': 'H1' if all(p['eager_over_lazy']['cpu_ns'] < 1 for p in pairs) else 'H0-compatible'}
    result = {'slices': 15, 'trials': 33, 'actual_complete_oracle_chains': 1826,
              'negative_controls': controls(), 'rates': rates, 'trials_from_raw_timings': trials,
              'counted_allocation_scope': 'per nonoverlapping phase diagnostic; no total-schedule allocation or peak claim',
              'schedule_throughput_scope': '32 native durable ACKs / total schedule wall; no Spool or forwarding claim',
              'oracle_scope': 'validated retained complete original oracle verdicts; no new semantic-oracle execution',
              'object_decoded_readbacks': objects.readback_count, 'validated_actual_batches': len(validated)}
    text = json.dumps(result, indent=2) + '\n'
    if args.out:
        if args.out.exists():
            raise RuntimeError('summary output already exists')
        args.out.write_text(text)
    print(text, end='')


if __name__ == '__main__':
    main()
