#!/usr/bin/env python3
"""Bounded offline builder census; differential ledgers are not an oracle.

Run only through tools/resource_group.py after registering the protocol/seed.
Allocation counters cover the probe's build interval; RSS/CPU/cgroup samples
cover generation, build and verification. No reservation observer exists.
"""
import argparse
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
from types import SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits

MIB = 1024**2
DATA = Path('/run/media/kmosoti/data/FabricO11y')
RAW_LIMIT = 128 * MIB
ARCHIVE_LIMIT = 64 * MIB
EVIDENCE_LIMIT = 288 * MIB
REPLAY_FAILURE_RESERVE = 48 * MIB
RESERVE = 16 * 1024**3
SAMPLE_SECONDS = 0.1
TRIAL_SECONDS = 120
CAMPAIGN_SECONDS = 900
PROBE = ROOT / 'crates/fabric-server/examples/readiness_memory_lab.rs'
FILTER_TABLES = {'text_filter.bin': ('logs.parquet', 'body'),
                 'spans_filter.bin': ('spans.parquet', 'trace_id')}


def decode_filters(data):
    """Independent FTF1 framing reader; no Rust filter reader is invoked."""
    if len(data) < 8 or data[:4] != b'FTF1':
        raise ValueError('invalid FTF1 header')
    count = int.from_bytes(data[4:8], 'little')
    if count > 65536 or 8 + 4*count > len(data):
        raise ValueError('invalid/truncated FTF1 group count')
    sizes = [int.from_bytes(data[8+4*i:12+4*i], 'little') for i in range(count)]
    if any(bits < 4096 or bits > 2**20 or bits & (bits-1) for bits in sizes):
        raise ValueError('invalid FTF1 bit count')
    at = 8 + 4*count
    if at + sum(bits//8 for bits in sizes) != len(data):
        raise ValueError('FTF1 length differs from header')
    filters = []
    for bits in sizes:
        filters.append(data[at:at+bits//8])
        at += bits//8
    return filters


def expected_filter(bodies):
    """Reconstruct from actual row bytes, using a fixed 2MiB trigram bitmap.

    FTF1's two specified 32-bit multiplicative hashes are reproduced in Python;
    the producer's Bloom constructor and membership predicate are never called.
    """
    seen = bytearray(2*MIB)
    distinct = occurrences = rows = 0
    for body in bodies:
        rows += 1
        for index in range(max(0, len(body)-2)):
            token = body[index] | body[index+1] << 8 | body[index+2] << 16
            slot, bit = token >> 3, 1 << (token & 7)
            if not seen[slot] & bit:
                seen[slot] |= bit
                distinct += 1
            occurrences += 1
    bits = min(2**20, max(4096, 1 << max(0, (8*distinct-1).bit_length())))
    expected = bytearray(bits//8)
    for slot, tokens in enumerate(seen):
        while tokens:
            low_bit = tokens & -tokens
            token = (slot << 3) + low_bit.bit_length()-1
            tokens ^= low_bit
            first = ((token * 0x9E3779B1) & 0xffffffff) >> 8
            rotated = (token * 0x85EBCA6B) & 0xffffffff
            rotated = ((rotated << 13) | (rotated >> 19)) & 0xffffffff
            second = ((rotated * 0xC2B2AE35) & 0xffffffff) >> 8
            for position in (first & (bits-1), second & (bits-1)):
                expected[position >> 3] |= 1 << (position & 7)
    return bytes(expected), {'rows': rows, 'trigram_occurrences': occurrences,
                             'distinct_trigrams': distinct, 'bits': bits}


def aligned_filter_counts(filters, group_count, declared):
    return len(filters) == group_count == declared


def validate_filter_bytes(actual, expected):
    if len(actual) != len(expected):
        raise ValueError('filter bit size differs from row-derived construction')
    if any(wanted & ~present for present, wanted in zip(actual, expected)):
        raise ValueError('filter excludes an actual row trigram')
    if actual != expected:
        raise ValueError('filter contains bits outside exact row-derived construction')


def physical_filters(state, row, parquet, deadline):
    """Authenticate files; validate filters against their own real row groups."""
    output = state / 'state/segments/seg-00000000000000000001'
    manifest = json.loads((output / 'manifest.json').read_text())
    if manifest != row['manifest']:
        raise ValueError('disk manifest differs from reported manifest')
    for name, entry in manifest['files'].items():
        path = output / name
        if path.stat().st_size != entry['bytes'] or digest(path) != entry['sha256']:
            raise ValueError('file authentication failed: ' + name)
    checks = {}
    started = time.monotonic()
    for filter_name, (table_name, column) in FILTER_TABLES.items():
        if table_name not in manifest['files']:
            if filter_name in manifest['files']:
                raise ValueError('filter exists without its table')
            continue
        if filter_name not in manifest['files']:
            raise ValueError('expected constructed filter omitted: ' + filter_name)
        filters = decode_filters((output / filter_name).read_bytes())
        table = parquet.ParquetFile(output / table_name)
        groups = table.metadata.num_row_groups
        declared = manifest['files'][filter_name]['rows']
        if not aligned_filter_counts(filters, groups, declared):
            raise ValueError('filter count differs from own Parquet groups: ' + filter_name)
        if table.metadata.num_rows != manifest['files'][table_name]['rows']:
            raise ValueError('Parquet logical row count differs from manifest')
        group_checks = []
        for group, actual in enumerate(filters):
            def bodies():
                for batch in table.iter_batches(batch_size=64, row_groups=[group],
                                                columns=[column], use_threads=False):
                    if time.monotonic() > deadline:
                        raise RuntimeError('campaign deadline during physical filter validation')
                    for value in batch.column(0):
                        text = value.as_py()
                        if not isinstance(text, str):
                            raise ValueError('filter source row is not a non-null string')
                        yield text.encode('utf-8')
            expected, counts = expected_filter(bodies())
            if counts['rows'] != table.metadata.row_group(group).num_rows:
                raise ValueError('decoded group rows differ from own footer')
            validate_filter_bytes(actual, expected)
            group_checks.append({'index': group, **counts})
        checks[filter_name] = {'aligned': True, 'exact_row_derived_bits': True,
                               'false_negatives': 0, 'parquet_groups': groups,
                               'declared_filter_rows': declared,
                               'logical_rows': table.metadata.num_rows,
                               'groups': group_checks}
    return {'files_authenticated': True, 'provider': 'pyarrow.parquet + independent Python FTF1 reconstruction',
            'validation_wall_seconds': time.monotonic()-started, 'filters': checks}


def physical_checks_valid(row):
    checks = row.get('physical_filter_check', {})
    if not checks.get('files_authenticated'):
        return False
    for name, (table, _) in FILTER_TABLES.items():
        entry = row['manifest']['files'].get(name)
        table_entry = row['manifest']['files'].get(table)
        check = checks.get('filters', {}).get(name)
        if table_entry is None:
            if entry is not None or check is not None:
                return False
            continue
        if not entry or not check:
            return False
        if not (check['aligned'] and check['exact_row_derived_bits'] and check['false_negatives'] == 0
                and check['parquet_groups'] == check['declared_filter_rows'] == entry['rows']
                and check['logical_rows'] == table_entry['rows']):
            return False
    return True


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(MIB), b''):
            h.update(chunk)
    return h.hexdigest()


def inventory(root):
    """Logical regular-file bytes, tolerating only concurrent disappearance."""
    totals = dict(total=0, journal=0, segment=0, temporary=0)
    def onerror(error):
        if not isinstance(error, FileNotFoundError):
            raise error
    for directory, _, names in os.walk(root, onerror=onerror):
        for name in names:
            path = Path(directory) / name
            try:
                entry = path.stat()
            except FileNotFoundError:
                continue
            if not stat.S_ISREG(entry.st_mode):
                continue
            totals['total'] += entry.st_size
            parts = path.relative_to(root).parts
            if 'input' in parts:
                totals['journal'] += entry.st_size
            elif any(part.startswith('.building-') or '.run-' in part for part in parts):
                totals['temporary'] += entry.st_size
            elif 'segments' in parts:
                totals['segment'] += entry.st_size
    return totals


def grade(reference, bounded):
    def metadata(row):
        value = copy.deepcopy(row['manifest'])
        value['files'] = {name: {} if name in FILTER_TABLES else {'rows': entry['rows']}
                          for name, entry in value['files'].items()}
        return value
    files = ['metrics.parquet'] + ([] if bounded['shape'] == 'bigrows' else ['logs.parquet'])
    valid_filters = physical_checks_valid(reference) and physical_checks_valid(bounded)
    return {
        'same_fixture': all(reference[key] == bounded[key] for key in
                            ['shape', 'target_mib', 'seed', 'groups', 'input_sha256', 'journal_bytes', 'encoded_group_bytes']),
        'ordered_rows_and_custody': reference['ordered_row_ledgers'] == bounded['ordered_row_ledgers'],
        'manifest_metadata': valid_filters and metadata(reference) == metadata(bounded),
        'physical_filters_valid': valid_filters,
        'applicable_file_bytes': all(reference['manifest']['files'][name]['sha256'] ==
                                     bounded['manifest']['files'][name]['sha256'] for name in files),
        'no_leftovers': all(not row[key] for row in [reference, bounded]
                            for key in ['building_leftovers', 'run_leftovers']),
    }


def equal_member_maps(expected, actual):
    if expected != actual:
        raise ValueError('archive reuse differs: missing, extra, or changed regular members')


def regular_members(state):
    result = {}
    for path in state.rglob('*'):
        mode = path.lstat().st_mode
        if stat.S_ISREG(mode):
            result[str(path.relative_to(state))] = {'bytes': path.stat().st_size, 'sha256': digest(path)}
        elif not stat.S_ISDIR(mode):
            raise ValueError('nonregular fixture member: ' + str(path))
    return result


def archive_members(archive, root_name):
    result = {}
    decoded = 0
    with tarfile.open(archive, 'r:gz') as stream:
        for member in stream:
            name = Path(member.name)
            if name.is_absolute() or '..' in name.parts or not name.parts or name.parts[0] != root_name:
                raise ValueError('archive member outside expected fixture root')
            if member.isdir():
                continue
            if not member.isfile():
                raise ValueError('nonregular archive fixture member')
            key = str(Path(*name.parts[1:]))
            if key == '.' or key in result:
                raise ValueError('invalid/duplicate archive member')
            decoded += member.size
            if decoded > RAW_LIMIT:
                raise ValueError('archive decoded fixture ceiling')
            reader = stream.extractfile(member)
            h = hashlib.sha256()
            size = 0
            for chunk in iter(lambda: reader.read(MIB), b''):
                size += len(chunk)
                h.update(chunk)
            if size != member.size:
                raise ValueError('truncated archived member')
            result[key] = {'bytes': size, 'sha256': h.hexdigest()}
    return result


def decoder_worker(mode, state=None, stdout=None, deadline=None):
    import pyarrow
    import pyarrow.parquet as parquet
    if mode == 'version':
        return {'pyarrow_version': pyarrow.__version__}
    return physical_filters(Path(state), json.loads(Path(stdout).read_text()), parquet, float(deadline))


def decoder_call(mode, deadline, state=None, stdout=None):
    command = [sys.executable, '-B', str(Path(__file__).resolve()), '--decoder-worker', mode]
    if state is not None:
        command += [str(state), str(stdout), str(deadline)]
    # wait reaps the decoder; the native-spawning parent never imports it.
    with tempfile.TemporaryFile(dir=os.environ['FABRIC_SCRATCH_ROOT']) as output, \
            tempfile.TemporaryFile(dir=os.environ['FABRIC_SCRATCH_ROOT']) as error:
        child = subprocess.Popen(command, stdout=output, stderr=error)
        try:
            child.wait(timeout=max(0.001, min(TRIAL_SECONDS, deadline-time.monotonic())))
        except BaseException:
            child.kill()
            child.wait()
            raise
        if output.tell() + error.tell() > MIB:
            raise RuntimeError('decoder output ceiling')
        output.seek(0)
        error.seek(0)
        if child.returncode:
            raise RuntimeError('decoder exit ' + str(child.returncode) + ': ' + error.read().decode(errors='replace'))
        return json.loads(output.read())


def controls():
    row = dict(shape='steady', target_mib=16, seed=1, groups=2,
               input_sha256='input', journal_bytes=1, encoded_group_bytes=1,
               ordered_row_ledgers={'logs': 'l', 'metrics': 'm', 'spans': 's', 'gaps': 'g', 'batches': 'b'},
               manifest={'records': 2, 'files': {name: {'rows': 2, 'sha256': 'hash'}
                         for name in ['logs.parquet', 'metrics.parquet', 'text_filter.bin']}},
               physical_filter_check={'files_authenticated': True, 'filters': {'text_filter.bin':
                   {'aligned': True, 'exact_row_derived_bits': True, 'false_negatives': 0,
                    'parquet_groups': 2, 'declared_filter_rows': 2, 'logical_rows': 2}}},
               building_leftovers=[], run_leftovers=[])
    if not all(grade(row, row).values()):
        raise AssertionError('unchanged positive control rejected')
    results = {}
    for label in ['fixture', 'row', 'custody', 'manifest', 'file', 'leftover', 'filter_count', 'omitted_filter', 'false_negative']:
        bad = copy.deepcopy(row)
        if label == 'fixture': bad['input_sha256'] = 'different'
        if label == 'row': bad['ordered_row_ledgers']['logs'] = 'dropped-or-changed'
        if label == 'custody': bad['ordered_row_ledgers']['batches'] = 'different'
        if label == 'manifest': bad['manifest']['records'] = 1
        if label == 'file': bad['manifest']['files']['metrics.parquet']['sha256'] = 'different'
        if label == 'leftover': bad['run_leftovers'] = ['.run-left']
        if label == 'filter_count': bad['manifest']['files']['text_filter.bin']['rows'] = 1
        if label == 'omitted_filter': del bad['physical_filter_check']['filters']['text_filter.bin']
        if label == 'false_negative': bad['physical_filter_check']['filters']['text_filter.bin']['false_negatives'] = 1
        results[label] = grade(row, bad)
        if all(results[label].values()):
            raise AssertionError('negative control accepted: ' + label)
    different_layout = copy.deepcopy(row)
    different_layout['manifest']['files']['text_filter.bin']['rows'] = 1
    different_layout['physical_filter_check']['filters']['text_filter.bin'].update(parquet_groups=1, declared_filter_rows=1)
    if not all(grade(row, different_layout).values()):
        raise AssertionError('different individually validated physical layout rejected')
    # The empty group has the specified minimum bit count and entirely zero bits.
    golden = b'FTF1' + (1).to_bytes(4, 'little') + (4096).to_bytes(4, 'little') + bytes(512)
    if decode_filters(golden) != [bytes(512)] or expected_filter([b''])[0] != bytes(512):
        raise AssertionError('empty filter golden framing/construction rejected')
    nonempty, _ = expected_filter([b'abcabc'])
    other, _ = expected_filter([b'xyzxyz'])
    extra = bytearray(nonempty)
    for index, present in enumerate(extra):
        if present != 255:
            extra[index] |= (~present & 255) & -(~present & 255)
            break
    byte_controls = {'omitted': lambda: decode_filters(golden[:-1]),
                     'misaligned': lambda: aligned_filter_counts(decode_filters(golden), 2, 1),
                     'cleared': lambda: validate_filter_bytes(bytes(512), nonempty),
                     'extra_bit': lambda: validate_filter_bytes(bytes(extra), nonempty),
                     'wrong_group': lambda: validate_filter_bytes(other, nonempty)}
    rejected = {}
    for name, mutation in byte_controls.items():
        try:
            result = mutation()
        except ValueError:
            rejected[name] = True
        else:
            if result is not False:
                raise AssertionError('actual filter mutation accepted: ' + name)
            rejected[name] = True
    with patch('os.walk', return_value=[('/owned', [], ['vanished', 'survivor'])]), \
            patch.object(Path, 'stat', side_effect=[FileNotFoundError('renamed'),
                         SimpleNamespace(st_mode=stat.S_IFREG, st_size=7)]):
        if inventory(Path('/owned'))['total'] != 7:
            raise AssertionError('renamed file inventory race')
    with patch('os.walk', return_value=[('/owned', [], ['denied'])]), \
            patch.object(Path, 'stat', side_effect=PermissionError('denied')):
        try:
            inventory(Path('/owned'))
        except PermissionError:
            pass
        else:
            raise AssertionError('inventory swallowed permission error')
    output = io.BytesIO()
    writer = ArchiveWriter(output, 2)
    writer.write(b'ab')
    try:
        writer.write(b'c')
    except RuntimeError:
        pass
    else:
        raise AssertionError('archive evidence ceiling bypassed')
    if output.getvalue() != b'ab':
        raise AssertionError('archive wrote beyond ceiling')
    original = {'input/file': {'bytes': 1, 'sha256': 'a'}}
    equal_member_maps(original, dict(original))
    for changed in [{}, {'input/file': {'bytes': 1, 'sha256': 'b'}},
                    {**original, 'extra': {'bytes': 0, 'sha256': 'c'}}]:
        try:
            equal_member_maps(original, changed)
        except ValueError:
            pass
        else:
            raise AssertionError('archive dedupe mutation accepted')
    if 'pyarrow' in sys.modules:
        raise AssertionError('native-spawning parent imported decoder')
    waited = []
    def fake_spawn(*args, **kwargs):
        kwargs['stdout'].write(b'{"pyarrow_version":"control"}')
        return SimpleNamespace(returncode=0, wait=lambda **kw: waited.append('reaped'))
    with patch('subprocess.Popen', side_effect=fake_spawn):
        if decoder_call('version', time.monotonic()+1)['pyarrow_version'] != 'control' or waited != ['reaped']:
            raise AssertionError('decoder result returned without reaping')
    events = []
    def timeout_wait(**kwargs):
        if kwargs:
            events.append('timeout')
            raise subprocess.TimeoutExpired('control', 1)
        events.append('reaped')
    timed_out = SimpleNamespace(returncode=None, wait=timeout_wait, kill=lambda: events.append('killed'))
    with patch('subprocess.Popen', return_value=timed_out):
        try:
            decoder_call('version', time.monotonic()+1)
        except subprocess.TimeoutExpired:
            pass
        else:
            raise AssertionError('decoder timeout swallowed')
    if events != ['timeout', 'killed', 'reaped']:
        raise AssertionError('decoder timeout left child running')
    return {'unchanged_accepted': True, 'rejected_mutations': results,
            'reuse_changed_missing_extra_rejected': True, 'parent_decoder_import_absent': True,
            'decoder_reaped_before_result': True, 'decoder_timeout_killed_and_reaped': True,
            'different_validated_layout_accepted': True, 'filter_byte_controls_rejected': rejected,
            'inventory_disappearance_tolerated': True, 'inventory_permission_error_propagated': True,
            'archive_overage_rejected_before_write': True}


def cgroup():
    relative = next(line[3:] for line in Path('/proc/self/cgroup').read_text().splitlines()
                    if line.startswith('0::'))
    return Path('/sys/fs/cgroup') / relative.lstrip('/')


def sample(group, pid, state, started):
    result = {'elapsed_seconds': time.monotonic() - started,
              'disk_logical_bytes': inventory(state), 'cgroup': {}}
    for name in ['memory.current', 'memory.stat', 'memory.events', 'memory.pressure', 'cpu.stat']:
        result['cgroup'][name] = (group / name).read_text().strip()
    try:
        fields = Path(f'/proc/{pid}/status').read_text().splitlines()
        result['process'] = {line.split(':', 1)[0]: line.split(':', 1)[1].strip()
                             for line in fields if line.startswith(('VmRSS:', 'VmHWM:', 'VmSize:'))}
    except FileNotFoundError:
        result['process'] = None
    return result


def guard(work, destination, deadline):
    if time.monotonic() > deadline:
        raise RuntimeError('campaign deadline')
    if inventory(work)['total'] > RAW_LIMIT:
        raise RuntimeError('128MiB scratch ceiling')
    if inventory(evidence_root(destination))['total'] > EVIDENCE_LIMIT:
        raise RuntimeError('288MiB capacity evidence ceiling')
    if shutil.disk_usage(work).free < RESERVE:
        raise RuntimeError('16GiB free-space reserve')


class ArchiveWriter:
    """Stop compressed writes at the evidence ceiling without deleting source."""
    def __init__(self, stream, limit):
        self.stream = stream
        self.limit = limit
        self.bytes_written = 0

    def write(self, data):
        if self.bytes_written + len(data) > self.limit:
            raise RuntimeError('archive/evidence ceiling; scratch retained')
        written = self.stream.write(data)
        self.bytes_written += written
        return written

    def tell(self):
        return self.stream.tell()

    def flush(self):
        self.stream.flush()


def evidence_root(destination):
    registered = ROOT / 'docs/experiments/benchmarks/data/cross-system-run-01/memory'
    return registered if destination.is_relative_to(registered) else destination


def preserve(state, destination, reuse=None):
    """Keep exact journals and outputs, including failed partial state, before cleanup."""
    raw = inventory(state)
    if raw['total'] > RAW_LIMIT:
        raise RuntimeError('state exceeds preservation envelope; scratch retained')
    mismatch = None
    if reuse is not None:
        try:
            receipt = json.loads((reuse / 'preservation.json').read_text())
            archive = reuse / 'exact-state.tar.gz'
            if archive.stat().st_size > ARCHIVE_LIMIT or digest(archive) != receipt['archive_sha256']:
                raise ValueError('reuse archive authentication failed')
            expected = archive_members(archive, state.name)
            actual = regular_members(state)
            equal_member_maps(expected, actual)
            dump(destination / 'preservation.json', {'archive_reference': str(archive),
                 'archive_sha256': receipt['archive_sha256'], 'archive_bytes': archive.stat().st_size,
                 'source_logical_bytes': raw, 'regular_members': actual,
                 'payload_verification': 'exact regular member path/bytes/sha256 equality; historical archive unchanged'})
            return
        except (ValueError, KeyError, OSError, tarfile.TarError) as error:
            mismatch = str(error)
            dump(destination / 'reuse-failure.json', {'error': mismatch, 'reuse_cell': str(reuse)})
    archive = destination / 'exact-state.tar.gz'
    # The pair directory and builder directory are below the census evidence root.
    remaining = EVIDENCE_LIMIT - inventory(evidence_root(destination.parent.parent))['total'] - MIB
    if remaining <= 0:
        raise RuntimeError('no preservation allowance remains; scratch retained')
    with archive.open('wb') as output:
        with tarfile.open(fileobj=ArchiveWriter(output, min(ARCHIVE_LIMIT, remaining)),
                          mode='w:gz', compresslevel=1) as stream:
            stream.add(state, arcname=state.name)
    # Verify every archived regular-file payload against the source before deletion.
    with tarfile.open(archive, 'r:gz') as stream:
        for entry in stream:
            if entry.isfile():
                source = state.parent / entry.name
                reader = stream.extractfile(entry)
                h = hashlib.sha256()
                for chunk in iter(lambda: reader.read(MIB), b''):
                    h.update(chunk)
                if h.hexdigest() != digest(source):
                    raise RuntimeError('preservation digest mismatch: ' + entry.name)
    dump(destination / 'preservation.json', {'archive': archive.name,
         'archive_sha256': digest(archive), 'archive_bytes': archive.stat().st_size,
         'source_logical_bytes': raw, 'payload_verification': 'sha256 compared with source'})
    if mismatch is not None:
        raise ValueError('archive reuse mismatch; full new archive and scratch retained: ' + mismatch)


def trial(binary, state, destination, shape, seed, group, deadline, work, evidence, reuse):
    argv = [str(binary), str(state), shape, '16', str(seed), destination.name]
    dump(destination / 'command.json', {'argv': argv})
    started = time.monotonic()
    child = None
    row = None
    failure = None
    usage = None
    child_wall_seconds = None
    try:
        with (destination / 'stdout.json').open('wb') as out, (destination / 'stderr.txt').open('wb') as err, \
                (destination / 'timeline.jsonl').open('w') as timeline:
            dump(destination / 'parent-before-spawn.json', {'pid': os.getpid(),
                 'process': sample(group, os.getpid(), state, started)['process'],
                 'pyarrow_imported': 'pyarrow' in sys.modules})
            child = subprocess.Popen(argv, stdout=out, stderr=err)
            while True:
                timeline.write(json.dumps(sample(group, child.pid, state, started)) + '\n')
                timeline.flush()
                pid, status, usage = os.wait4(child.pid, os.WNOHANG)
                if pid:
                    child.returncode = os.waitstatus_to_exitcode(status)
                    child_wall_seconds = time.monotonic()-started
                    break
                guard(work, evidence, deadline)
                if time.monotonic() - started > TRIAL_SECONDS:
                    raise RuntimeError('120-second child deadline')
                if out.tell() + err.tell() > MIB:
                    raise RuntimeError('1MiB child output ceiling')
                time.sleep(SAMPLE_SECONDS)
        if child.returncode:
            raise RuntimeError(f'child exit {child.returncode}')
        row = json.loads((destination / 'stdout.json').read_text())
        expected = {'shape': shape, 'target_mib': 16, 'seed': seed, 'builder': destination.name}
        if any(row.get(key) != value for key, value in expected.items()):
            raise RuntimeError('probe did not report the commanded fixture/builder')
        row['physical_filter_check'] = decoder_call('validate', deadline, state, destination / 'stdout.json')
        dump(destination / 'physical-filter-check.json', row['physical_filter_check'])
    except BaseException as error:
        failure = error
        if child is not None and child.returncode is None:
            child.kill()
            _, status, usage = os.wait4(child.pid, 0)
            child.returncode = os.waitstatus_to_exitcode(status)
            child_wall_seconds = time.monotonic()-started
    finally:
        receipt = {'exit_code': child.returncode if child else None,
                   'wall_seconds': child_wall_seconds,
                   'user_seconds': usage.ru_utime if usage else None,
                   'system_seconds': usage.ru_stime if usage else None,
                   'whole_process_peak_rss_kib': usage.ru_maxrss if usage else None,
                   'error': str(failure) if failure else None}
        dump(destination / 'process.json', receipt)
        # Failed preservation leaves the source intact and prevents further cells.
        if state.exists():
            try:
                preserve(state, destination, reuse if failure is None else None)
                shutil.rmtree(state)
            except BaseException as error:
                dump(destination / 'cleanup.json', {'removed': False,
                     'retained_scratch': str(state), 'error': str(error)})
                raise
        dump(destination / 'cleanup.json', {'removed': not state.exists()})
    if failure:
        raise failure
    return {'probe': row, 'process': receipt}


def main():
    require_limits()
    if len(sys.argv) > 2 and sys.argv[1] == '--decoder-worker':
        mode = sys.argv[2]
        if mode not in ['version', 'validate'] or len(sys.argv) != (3 if mode == 'version' else 6):
            raise ValueError('invalid decoder worker arguments')
        print(json.dumps(decoder_worker(mode, *sys.argv[3:])))
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--protocol', type=Path, required=True)
    parser.add_argument('--seed', type=int, required=True)
    parser.add_argument('--controls-only', action='store_true')
    parser.add_argument('--reuse-census', type=Path)
    args = parser.parse_args()
    if not 0 < args.seed < 2**64:
        parser.error('seed must be a nonzero u64')
    destination = args.destination.resolve()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    registered = ROOT / 'docs/experiments/benchmarks/data/cross-system-run-01/memory'
    if not scratch.is_relative_to(DATA.resolve()) or not (
            destination.is_relative_to(DATA.resolve()) or destination.is_relative_to(registered.resolve())):
        parser.error('scratch must be data-drive; destination must be data-drive or registered memory evidence')
    if destination.exists():
        parser.error('destination must not exist')
    destination.mkdir(parents=True)
    dump(destination / 'negative-controls.json', controls())
    if args.controls_only:
        dump(destination / 'complete.json', {'exit_code': 0, 'controls_only': True})
        return
    try:
        decoder = decoder_call('version', time.monotonic() + TRIAL_SECONDS)
    except RuntimeError as error:
        dump(destination / 'failure.json', {'status': 'environment unavailable',
             'error': 'pyarrow.parquet required for independent physical filter validation',
             'native_cells_started': 0})
        raise RuntimeError('pyarrow.parquet required; no native cell started') from error
    reuse = args.reuse_census.resolve() if args.reuse_census else None
    if reuse is not None:
        if not (reuse.is_relative_to(DATA.resolve()) or reuse.is_relative_to(registered.resolve())):
            raise ValueError('reuse census must be data-drive or registered memory evidence')
        previous = json.loads((reuse / 'metadata.json').read_text())
        completed = json.loads((reuse / 'complete.json').read_text())
        if previous['seed'] != args.seed or previous['target_mib'] != 16 or completed['exit_code'] != 0:
            raise ValueError('reuse census must be completed with identical seed/size')
        if EVIDENCE_LIMIT - inventory(evidence_root(destination))['total'] < REPLAY_FAILURE_RESERVE:
            raise RuntimeError('48MiB replay failure preservation reserve unavailable')
    binary = Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/readiness_memory_lab'
    group = cgroup()
    sources = [PROBE, ROOT / 'Cargo.lock']
    for subtree in ['crates/fabric-server/src', 'crates/fabric-frame/src']:
        sources.extend(sorted((ROOT / subtree).rglob('*.rs')))
    frozen = {'seed': args.seed, 'target_mib': 16, 'cgroup': str(group),
              'binary': str(binary), 'binary_sha256': digest(binary), 'probe_sha256': digest(PROBE),
              'runner_sha256': digest(Path(__file__)), 'protocol_sha256': digest(args.protocol),
              'cpu_affinity': sorted(os.sched_getaffinity(0)), 'sampling_seconds': SAMPLE_SECONDS,
              'reservation_bytes': None, 'reservation_reason': 'probe has no reservation observer',
              'allocation_timeline': None, 'allocation_reason': 'probe emits build-only start/peak/after counters',
              'ledger_provenance': 'same Rust scanner differential; not an independent oracle',
              'physical_filter_provenance': 'pyarrow Parquet decoder + Python reconstruction from row bytes; independent of Rust filter code',
              'pyarrow_version': decoder['pyarrow_version'], 'decoder_ownership': 'reaped subprocess; parent never imports pyarrow',
              'reuse_census': str(reuse) if reuse else None,
              'limits': {'raw_bytes': RAW_LIMIT, 'archive_bytes_per_cell': ARCHIVE_LIMIT,
                         'evidence_bytes': EVIDENCE_LIMIT, 'free_reserve_bytes': RESERVE,
                         'replay_failure_reserve_bytes': REPLAY_FAILURE_RESERVE,
                         'trial_seconds': TRIAL_SECONDS, 'campaign_seconds': CAMPAIGN_SECONDS},
              'git_revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'source_sha256': {str(path.relative_to(ROOT)): digest(path) for path in sources}}
    dump(destination / 'metadata.json', frozen)
    if reuse is not None and frozen['binary_sha256'] != previous['binary_sha256']:
        raise ValueError('reuse census native binary differs')
    for source, name in [(PROBE, 'probe.rs'), (Path(__file__), 'runner.py'), (args.protocol, 'protocol.txt')]:
        shutil.copyfile(source, destination / name)
    (destination / 'working-tree.diff').write_bytes(subprocess.check_output(['git', 'diff', 'HEAD'], cwd=ROOT))
    work = Path(tempfile.mkdtemp(prefix='memory-census-', dir=scratch))
    frozen['scratch'] = str(work)
    dump(destination / 'metadata.json', frozen)
    deadline = time.monotonic() + CAMPAIGN_SECONDS
    pairs = []
    try:
        for index, shape in enumerate(['steady', 'adversarial', 'bigrows']):
            pair_dir = destination / shape
            pair_dir.mkdir()
            order = ['reference', 'bounded'] if index % 2 == 0 else ['bounded', 'reference']
            rows = {}
            for builder in order:
                guard(work, destination, deadline)
                cell_dir = pair_dir / builder
                cell_dir.mkdir()
                rows[builder] = trial(binary, work / f'{shape}-{builder}', cell_dir,
                                      shape, args.seed, group, deadline, work, destination,
                                      reuse / shape / builder if reuse else None)
            gates = grade(rows['reference']['probe'], rows['bounded']['probe'])
            pair = {'shape': shape, 'order': order, 'rows': rows, 'gates': gates}
            dump(pair_dir / 'pair.json', pair)
            pairs.append(pair)
            if not all(gates.values()):
                raise RuntimeError(f'differential gate failure: {shape}: {gates}')
        guard(work, destination, deadline)
        shutil.rmtree(work)
        dump(destination / 'complete.json', {'exit_code': 0, 'pair_count': len(pairs),
             'child_count': sum(len(pair['rows']) for pair in pairs),
             'scratch_removed': not work.exists(), 'qualification': False,
             'scope': 'offline 16MiB differential builder census; partial M1',
             'reservation_metric': 'unsupported', 'independent_oracle': False})
    except BaseException as error:
        dump(destination / 'failure.json', {'error': str(error), 'retained_scratch': str(work),
                                            'remaining_logical_bytes': inventory(work)})
        if work.exists() and not any(work.iterdir()):
            work.rmdir()
        raise


if __name__ == '__main__':
    main()
