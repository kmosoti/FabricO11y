"""Read stopped raw custody without repairing files or importing Rust codecs."""
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import struct
import sys
import zlib

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import query_oracle as oracle

U64 = 2**64 - 1
MAX_BATCH = 1048576
BINDING = ('run_id', 'candidate_frozen', 'candidate_package_sha256',
           'package_source_commit', 'harness_commit')
UNITS = ('fabrico11y-server.service', 'fabrico11y-node.service')
NATIVE_CONFIGURATION_LOGS = ('/var/log/fabric-accept/allowed.log',
                               '/var/log/fabric-accept/denied.log')
NATIVE_CONFIGURATION_INTERVAL_S = 15
SCHEMAS = {
    'logs': {1: 'resource_logs'},
    'resource_logs': {1: 'resource', 2: 'scope_logs', 3: 'utf8'},
    'scope_logs': {1: 'scope', 2: 'record', 3: 'utf8'},
    'record': {1: 1, 2: 0, 3: 'utf8', 5: 'any', 6: 'kv', 7: 0, 8: 5, 9: 2, 10: 2, 11: 1, 12: 'utf8'},
    'resource': {1: 'kv', 2: 0, 3: 'entity'},
    'scope': {1: 'utf8', 2: 'utf8', 3: 'kv', 4: 0},
    'entity': {1: 'utf8', 2: 'utf8', 3: 'utf8', 4: 'utf8'},
    'kv': {1: 'utf8', 2: 'any', 3: 0},
    'any': {1: 'utf8', 2: 0, 3: 0, 4: 1, 5: 'array', 6: 'kvlist', 7: 2, 8: 0},
    'array': {1: 'any'}, 'kvlist': {1: 'kv'},
    # Metric/Span nested leaves stay opaque; product ingest owns their semantics.
    'metrics': {1: 'resource_metrics'}, 'traces': {1: 'resource_traces'},
    'resource_metrics': {1: 'resource', 2: 'scope_metrics', 3: 'utf8'},
    'resource_traces': {1: 'resource', 2: 'scope_traces', 3: 'utf8'},
    'scope_metrics': {1: 'scope', 2: 'metric', 3: 'utf8'}, 'scope_traces': {1: 'scope', 2: 'span', 3: 'utf8'},
    'metric': {1: 2, 2: 2, 3: 2, 5: 2, 7: 2, 9: 2, 10: 2, 11: 2, 12: 2},
    'span': {1: 2, 2: 2, 3: 2, 4: 2, 5: 2, 6: 0, 7: 1, 8: 1, 9: 2, 10: 0,
             11: 2, 12: 0, 13: 2, 14: 0, 15: 2, 16: 5},
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def varint(raw, at):
    value = 0
    for index in range(10):
        require(at < len(raw), 'truncated protobuf varint')
        byte = raw[at]
        at += 1
        require(index != 9 or byte <= 1, 'protobuf varint overflow')
        value |= (byte & 127) << (7 * index)
        if byte < 128:
            return value, at
    raise ValueError('protobuf varint too long')


def read_fields(raw, types, depth=0, at=0, group=None):
    require(0 <= depth <= 100, 'protobuf recursion depth exhausted')
    result = {}
    while at < len(raw):
        key, at = varint(raw, at)
        number, wire = key >> 3, key & 7
        require(0 < number < 2**29 and key <= 2**32 - 1, 'invalid protobuf field/tag')
        require(wire in (0, 1, 2, 3, 4, 5), 'invalid protobuf wire type')
        if wire == 4:
            require(group == number, 'unmatched protobuf end group')
            return result, at
        spec = types.get(number)
        require(spec is not None or depth < 100, 'protobuf recursion depth exhausted')
        expected = 2 if isinstance(spec, str) else spec
        packed = isinstance(spec, tuple)
        require(spec is None or wire == expected or (packed and wire in (2, spec[1])),
                'wrong known-field wire type')
        if wire == 3:
            _, at = read_fields(raw, {}, depth + 1, at, number)
            continue
        if wire == 0:
            value, at = varint(raw, at)
        else:
            size, at = varint(raw, at) if wire == 2 else (8 if wire == 1 else 4, at)
            require(size <= len(raw) - at, 'truncated protobuf length/fixed field')
            value, at = raw[at:at + size], at + size
        if packed and wire == 2:
            require(spec[0] == 'packed' and spec[1] in (0, 1, 5), 'invalid packed specification')
            if spec[1] == 0:
                packed_at = 0
                while packed_at < len(value):
                    _, packed_at = varint(value, packed_at)
            else:
                require(len(value) % (8 if spec[1] == 1 else 4) == 0, 'packed fixed width')
        if group is None:
            # Last-occurrence order identifies the active known oneof arm.
            values = result.pop(number, [])
            values.append((wire, value))
            result[number] = values
    require(group is None, 'unterminated protobuf group')
    return result, at


def fields(raw, types, depth=0):
    return read_fields(raw, types, depth)[0]


def validate(raw, schema, depth=0):
    parsed = fields(raw, SCHEMAS[schema], depth)
    records = [parsed] if schema == 'record' else []
    for number, values in parsed.items():
        spec = SCHEMAS[schema].get(number)
        if isinstance(spec, str):
            for _, value in values:
                if spec == 'utf8':
                    value.decode('utf-8')
                else:
                    require(depth < 100, 'protobuf message recursion depth exhausted')
                    records.extend(validate(value, spec, depth + 1))
    return records


def one(values, number, default=0):
    return values.get(number, [(None, default)])[-1][1]


def length_field(number, raw):
    size, encoded = len(raw), bytearray()
    while size >= 128:
        encoded.append((size & 127) | 128)
        size >>= 7
    return bytes([number << 3 | 2]) + bytes(encoded) + bytes([size]) + raw


def string_projection(values):
    f = fields(b''.join(raw for _, raw in values), SCHEMAS['any'])
    active = next((number for number in reversed(f) if number in SCHEMAS['any']), None)
    return length_field(1, one(f, 1, b'')) if active == 1 else b''


def log_rows(records, batch):
    for index, f in enumerate(records):
        projection = [b'\x59' + one(f, 11, b'\0' * 8), length_field(5, string_projection(f.get(5, [])))]
        for _, attribute in f.get(6, []):
            kv = fields(attribute, SCHEMAS['kv'])
            projection.append(length_field(6, length_field(1, one(kv, 1, b''))
                + length_field(2, string_projection(kv.get(2, [])))))
        row = oracle.decode_log_record(b''.join(projection))
        yield dict(node='spindle-1', node_id=batch['node_id'].hex(), sequence=batch['sequence'],
            index=index, observed_ns=row['observed_time_unix_nano'], body=row['body'], attributes=row['attributes'])


def cursor(raw):
    f = fields(raw, {1: 2, 2: 0, 3: 0, 4: 0, 5: 0, 6: 0, 7: 0, 8: 2}, depth=1)
    path = one(f, 1, b'').decode('utf-8')
    require(len(path.encode()) <= 4096, 'cursor path cap')
    identity = None
    if 8 in f:
        stable = fields(b''.join(value for _, value in f[8]), {1: 2, 2: 0}, depth=2)
        uuid, subvolume = one(stable, 1, b''), one(stable, 2)
        require(len(uuid) == 16 and any(uuid) and subvolume > 0, 'invalid Btrfs identity')
        identity = dict(uuid=uuid.hex(), subvolume_id=subvolume)
    length, crc = one(f, 6), one(f, 7)
    require(all(value <= 2**32 - 1 for number in (6, 7) for _, value in f.get(number, [])), 'cursor u32 overflow')
    require(length <= 64 and length <= one(f, 4) and crc <= 2**32 - 1, 'invalid cursor prefix')
    return dict(path=path, device=one(f, 2), inode=one(f, 3), offset=one(f, 4),
        skipping_oversize=bool(one(f, 5)), prefix_len=length, prefix_crc=crc, btrfs_identity=identity)


def envelope(raw):
    try:
        f = fields(raw, {1: 0, 2: 2, 3: 0, 4: 0, 5: 2, 6: 2, 7: 2, 8: 2, 9: 2})
        require(all(value <= 2**32 - 1 for _, value in f.get(1, [])), 'Batch version u32 overflow')
        require(one(f, 1) == 1 and len(one(f, 2, b'')) == 16
            and one(f, 3) > 0 and 0 < one(f, 4) < U64, 'invalid Batch identity/version')
        signals = [one(f, number, b'') for number in (5, 6, 9)]
        gaps = [value for _, value in f.get(8, [])]
        require(any(signals) or gaps, 'empty Batch')
        require(sum(map(len, signals)) <= MAX_BATCH and len(f.get(7, [])) <= 16
            and len(gaps) <= 130 and all(len(gap) <= 256 for gap in gaps), 'Batch field cap')
        for gap in gaps:
            gap.decode('utf-8')
        cursors = [cursor(value) for _, value in f.get(7, [])]
        batch = dict(version=one(f, 1), node_id=one(f, 2, b''), generation=one(f, 3), sequence=one(f, 4),
                     collection_gaps=[gap.decode('utf-8') for gap in gaps])
        decoded = [validate(signal, schema) for signal, schema in zip(signals, ('metrics', 'logs', 'traces'))]
        rows = list(log_rows(decoded[1], batch))
        return batch, cursors, rows
    except (oracle.MalformedInput, UnicodeError, TypeError, struct.error) as error:
        raise ValueError(str(error)) from error


def identity(info):
    return tuple(getattr(info, 'st_' + field) for field in
        ('dev', 'ino', 'mode', 'uid', 'gid', 'nlink', 'size', 'mtime_ns', 'ctime_ns'))


def owned(path):
    path = Path(path).absolute()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).absolute()
    require(path != scratch and path.is_relative_to(scratch), 'outside owned scratch')
    require(path.resolve(strict=True) == path and scratch.resolve(strict=True) == scratch,
            'symlink or noncanonical input')
    return path


def open_owned(path, flags):
    path = owned(path)
    fd = os.open('/', os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in path.parts[1:-1]:
            next_fd = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = next_fd
        return os.open(path.name, flags | os.O_NOFOLLOW, dir_fd=fd)
    finally:
        os.close(fd)


def read_regular(path, limit):
    before = path.lstat()
    require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1 and before.st_size <= limit,
            'nonregular, aliased or oversized input')
    fd = open_owned(path, os.O_RDONLY)
    try:
        require(identity(os.fstat(fd)) == identity(before), 'input changed before read')
        with os.fdopen(os.dup(fd), 'rb') as stream:
            raw = stream.read(limit + 1)
        require(len(raw) == before.st_size and identity(os.fstat(fd)) == identity(before)
            and identity(path.lstat()) == identity(before), 'input changed during read')
        return raw, identity(before)
    finally:
        os.close(fd)


def diagnostic_inventory(root, service):
    directory, observed, result = root / 'diagnostics', {}, {}
    try:
        require(all(type(service.get(key)) is int and service[key] >= 0 for key in ('uid', 'gid')), 'service identity')
        before = identity(owned(directory).lstat())
        require(stat.S_ISDIR(before[2]) and stat.S_IMODE(before[2]) == 0o700
            and before[3:5] == (service['uid'], service['gid']), 'directory ownership/mode')
        fd = open_owned(directory, os.O_RDONLY | os.O_DIRECTORY)
        try:
            names = sorted(os.listdir(fd))
            require(set(names) in ({'spindle.log'}, {'spindle.log', 'spindle.log.1'}), 'directory inventory')
            for name in names:
                raw, info = read_regular(directory / name, 262144)
                require(stat.S_IMODE(info[2]) == 0o600 and info[3:5] == before[3:5], 'file ownership/mode')
                observed['diagnostics/' + name] = info
                result[name] = dict(bytes=raw, sha256=hashlib.sha256(raw).hexdigest(), size=len(raw),
                    device=info[0], inode=info[1], mode=info[2], uid=info[3], gid=info[4], nlink=info[5])
            require(identity(os.fstat(fd)) == before == identity(directory.lstat())
                and sorted(os.listdir(fd)) == names, 'directory drift')
        finally:
            os.close(fd)
        observed['diagnostics'] = before
        return result, observed
    except (OSError, ValueError, TypeError, KeyError) as error:
        raise ValueError('diagnostic input: ' + str(error)) from error


def applied_configuration(raw, info, service):
    """Strict RemoteView shape; original bytes remain raw inventory authority."""
    try:
        def pairs(items):
            result = {}
            for key, value in items:
                require(key not in result, 'duplicate JSON field')
                result[key] = value
            return result
        require(len(raw) <= 65536 and stat.S_ISREG(info[2]) and info[5] == 1 and info[6] == len(raw),
                'bounded original regular single-link bytes required')
        require(stat.S_IMODE(info[2]) == 0o600 and (info[3], info[4]) == (service['uid'], service['gid']),
                'private service ownership required')
        value = json.loads(raw.decode('utf-8'), object_pairs_hook=pairs)
        require(type(value) is dict and set(value) == {'revision', 'paused', 'logs', 'metric_interval_s'}, 'RemoteView fields')
        require(type(value['revision']) is int and 0 <= value['revision'] <= U64
            and type(value['paused']) is bool and type(value['metric_interval_s']) is int
            and 1 <= value['metric_interval_s'] <= 3600, 'RemoteView scalar types/ranges')
        require(type(value['logs']) is list and len(value['logs']) <= 16
            and all(type(path) is str and Path(path).is_absolute() and len(path.encode()) <= 240
                    for path in value['logs']), 'RemoteView log profile')
        return value
    except (ValueError, TypeError, KeyError, UnicodeError) as error:
        raise ValueError('applied configuration: ' + str(error)) from error


def pause_marker(raw, info, service, configuration, observation, boot_id):
    """Join original pause commands to a private pending collection-gap sidecar."""
    try:
        require(type(raw) is bytes and re.fullmatch(rb'[1-9][0-9]{0,19}\n', raw)
            and int(raw) <= U64 and stat.S_ISREG(info[2]) and info[5] == 1
            and info[6] == len(raw) and stat.S_IMODE(info[2]) == 0o600
            and (info[3], info[4]) == (service['uid'], service['gid']), 'marker bytes/metadata')
        require(type(observation) is dict and set(observation) == {'boot_id', 'request', 'confirmation'}
            and observation['boot_id'] == boot_id, 'pause boot/proof')
        request, confirmation = observation['request'], observation['confirmation']
        prefix = ['fabricctl', 'admin', '/root/accept/admin.conf', 'node']
        require(request['argv'] == prefix + ['pause', 'spindle-1']
            and confirmation['argv'] == prefix + ['list'], 'pause command identity')
        for record in (request, confirmation):
            require(type(record['exit']) is int and record['exit'] == 0
                and type(record['stdout']) is str and type(record['stderr']) is str
                and all(type(record[key]) is int and record[key] > 0
                    for key in ('started_ns', 'finished_ns', 'elapsed_ns'))
                and record['started_ns'] <= record['finished_ns']
                and record['elapsed_ns'] <= 30_000_000_000, 'pause command completion')
        start, finish = request['started_ns'], confirmation['finished_ns']
        require(request['finished_ns'] <= confirmation['started_ns']
            and 0 < finish - start <= 30_000_000_000 and start <= int(raw) <= finish,
            'pause marker outside application interval')
        rows = json.loads(confirmation['stdout'])['nodes']
        matches = [row for row in rows if row.get('name') == 'spindle-1']
        require(len(matches) == 1, 'pause inventory identity')
        row = matches[0]
        revision = configuration['revision']
        require(type(revision) is int and revision in (2, 4)
            and configuration == dict(revision=revision, paused=True,
                logs=list(NATIVE_CONFIGURATION_LOGS), metric_interval_s=NATIVE_CONFIGURATION_INTERVAL_S)
            and all(type(row[key]) is int for key in
                ('desired_revision', 'applied_revision', 'metric_interval_s'))
            and row['desired_revision'] == row['applied_revision'] == revision
            and row['status'] == 'paused' and row['config_error'] is None
            and row['logs'] == configuration['logs']
            and row['metric_interval_s'] == configuration['metric_interval_s'], 'pause applied profile')
        return dict(raw=raw, observation=copy.deepcopy(observation))
    except (ValueError, TypeError, KeyError, AttributeError, IndexError) as error:
        raise ValueError('native pause marker: ' + str(error)) from error


def stopped_snapshot(root, stopped_receipt, archived_allowed_logs, *, fixture_paths=None):
    root = owned(root)
    receipt = copy.deepcopy(stopped_receipt)
    require(fixture_paths is None or (type(fixture_paths) is tuple and len(fixture_paths) == 1
        and isinstance(fixture_paths[0], str) and Path(fixture_paths[0]).is_absolute()
        and fixture_paths[0] in archived_allowed_logs), 'invalid fixture paths')
    require(receipt.get('spool_root') == str(root) and receipt.get('stop_confirmed') is True
        and receipt.get('extra_project_units') == [] and isinstance(receipt.get('boot_id'), str)
        and receipt['boot_id'], 'incomplete stop receipt')
    binding = receipt.get('binding', {})
    require(all(isinstance(binding.get(key), str) and binding[key] for key in BINDING), 'missing binding')
    for key in BINDING[1:]:
        require(re.fullmatch('[0-9a-f]{64}' if key.endswith('sha256') else '[0-9a-f]{40}',
                binding[key]), 'invalid source/package binding')
    require(set(receipt.get('units', {})) == set(UNITS), 'missing or extra unit observation')
    for unit in receipt['units'].values():
        require(unit.get('active_state') in ('inactive', 'failed') and unit.get('query_exit') == 3
            and type(unit.get('query_exit')) is int and unit.get('main_pid') == 0
            and type(unit.get('main_pid')) is int and unit.get('cgroup_empty') is True
            and unit.get('companions') == [], 'unit not confirmed stopped')
    cap = receipt.get('spool_bytes')
    require(type(cap) is int and cap == 268435456, 'unregistered Spool cap')
    root_before = identity(root.lstat())
    require(stat.S_ISDIR(root.lstat().st_mode), 'Spool root is not a directory')
    fd = open_owned(root, os.O_RDONLY | os.O_DIRECTORY)
    try:
        require(identity(os.fstat(fd)) == root_before, 'Spool root changed')
        names = sorted(os.listdir(fd))
        require({'identity', 'writer.lock', 'batches.faj'} <= set(names), 'missing Spool input')
        diagnostics, observed = diagnostic_inventory(root, receipt.get('service_identity', {})) if fixture_paths else ({}, {})
        require(all(name in ('identity', 'acked', 'writer.lock', 'batches.faj') or (fixture_paths and name in ('diagnostics', 'applied-config.json'))
            or (fixture_paths == (NATIVE_CONFIGURATION_LOGS[0],) and name == 'coverage-unknown'
                and receipt.get('pause_observation') is not None)
            or re.fullmatch(r'sealed-[0-9]{20}\.faj', name) for name in names), 'unexpected or failed Spool input')
        files, raw_files = {'diagnostics/' + name: value for name, value in diagnostics.items()}, {}
        require(sum((root / name).lstat().st_size for name in names if name.endswith('.faj')) <= cap - 4096,
                'framed Spool cap')
        for name in names:
            if fixture_paths and name == 'diagnostics': continue
            try:
                raw, info = read_regular(root / name, cap - 4096 if name.endswith('.faj') else 65536 if name == 'applied-config.json' else 28)
                if name == 'applied-config.json': applied_configuration(raw, info, receipt['service_identity'])
            except ValueError as error:
                raise ValueError(('applied configuration: ' if name == 'applied-config.json' else '') + str(error)) from error
            raw_files[name], observed[name] = raw, info
            files[name] = dict(sha256=hashlib.sha256(raw).hexdigest(), size=len(raw),
                              mode=info[2], uid=info[3], gid=info[4])
        coverage_pause = None
        if receipt.get('pause_observation') is not None:
            require('coverage-unknown' in raw_files, 'native pause marker absent')
        if 'coverage-unknown' in raw_files:
            coverage_pause = pause_marker(raw_files['coverage-unknown'], observed['coverage-unknown'],
                receipt['service_identity'], json.loads(raw_files['applied-config.json']),
                receipt.get('pause_observation'), receipt['boot_id'])
        ident = raw_files['identity']
        require(len(ident) == 28 and ident[:4] == b'FAI1', 'invalid identity sidecar')
        node, generation = ident[4:20], struct.unpack('<Q', ident[20:])[0]
        require(generation > 0, 'zero generation')
        ack = 0
        if 'acked' in raw_files:
            acked = raw_files['acked']
            require(len(acked) == 20 and acked[:4] == b'FAK1', 'invalid ACK sidecar')
            ack_generation, ack = struct.unpack('<QQ', acked[4:])
            require(ack_generation == generation and ack < U64, 'ACK identity/exhaustion')
        sources, cursors = [], {}
        ordered = sorted(name for name in names if name.startswith('sealed-')) + ['batches.faj']
        for name in ordered:
            raw, at, first = raw_files[name], 0, True
            while at < len(raw):
                require(len(raw) - at >= 16 and raw[at:at + 4] == b'FAB1', 'incomplete frame/header')
                size, head_crc, payload_crc = struct.unpack('<III', raw[at + 4:at + 16])
                end = at + 16 + size
                require(0 < size <= MAX_BATCH and end + 16 <= len(raw)
                    and zlib.crc32(raw[at:at + 8]) == head_crc, 'frame length/header CRC')
                payload, marker = raw[at + 16:end], raw[end:end + 16]
                require(zlib.crc32(payload) == payload_crc and marker[:4] == b'FAC1'
                    and struct.unpack('<Q', marker[4:12])[0] == end
                    and zlib.crc32(marker[:12]) == struct.unpack('<I', marker[12:])[0], 'payload/commit CRC')
                batch, current, rows = envelope(payload)
                sequence = batch['sequence']
                require(batch['node_id'] == node and batch['generation'] == generation, 'Batch/Spool identity')
                require(not sources or sequence == sources[-1]['sequence'] + 1, 'lost/reordered source')
                require(not first or name == 'batches.faj' or int(name[7:-4]) == sequence, 'sealed label')
                sources.append(dict(sequence=sequence, sha256=hashlib.sha256(payload).hexdigest(),
                                    payload=payload, rows=rows, collection_gaps=batch['collection_gaps']))
                cursors.update((item['path'], item) for item in current)
                at, first = end + 16, False
            require(name == 'batches.faj' or not first, 'empty sealed source')
        require(not sources or sources[0]['sequence'] <= ack + 1 <= sources[-1]['sequence'] + 1,
                'ACK beyond source or missing pending prefix')
        logs, log_observations = {}, {}
        for path, entry in archived_allowed_logs.items():
            require(Path(path).is_absolute(), 'nonabsolute guest source')
            raw, info = read_regular(owned(entry['archive_path']), MAX_BATCH)
            log_observations[entry['archive_path']] = info
            require(all(type(entry[key]) is int and 0 <= entry[key] <= U64 for key in ('device', 'inode')), 'source identity type')
            logs[path] = dict(device=entry['device'], inode=entry['inode'],
                btrfs_identity=copy.deepcopy(entry.get('btrfs_identity')), bytes=raw,
                sha256=hashlib.sha256(raw).hexdigest(), size=len(raw))
        for path, current in cursors.items():
            require(path in logs, 'cursor source not archived')
            log = logs[path]
            namespace = (current['btrfs_identity'] == log['btrfs_identity']
                if current['btrfs_identity'] is not None else current['device'] == log['device'])
            offset = current['offset']
            require(namespace and current['inode'] == log['inode'] and offset <= log['size']
                and not current['skipping_oversize'] and (offset == 0 or log['bytes'][offset - 1:offset] == b'\n')
                and zlib.crc32(log['bytes'][:current['prefix_len']]) == current['prefix_crc'], 'latest cursor/source mismatch')
        if fixture_paths:
            diagnostic = logs.get('/var/lib/fabrico11y/node/spool/diagnostics/spindle.log', {})
            require(all(diagnostic.get(key) == diagnostics['spindle.log'][key]
                for key in ('bytes', 'device', 'inode')), 'diagnostic archive/source mismatch')
        require(owned(root) == root and sorted(os.listdir(fd)) == names and identity(os.fstat(fd)) == root_before
            and identity(root.lstat()) == root_before, 'Spool changed during snapshot')
        require(all(identity((root / name).lstat()) == info for name, info in observed.items()), 'Spool file drift')
        require(all(identity(owned(path).lstat()) == info for path, info in log_observations.items()), 'archive drift')
        return dict(binding=binding, boot_id=receipt['boot_id'], node_id=node.hex(), generation=generation,
            ack=ack, files=files, file_observations=observed, sources=sources, cursors=cursors,
            logs=logs, log_observations=log_observations, candidate_process=receipt.get('candidate_process'),
            fixture_paths=fixture_paths, diagnostics=diagnostics, coverage_pause=coverage_pause, applied_configuration=raw_files.get('applied-config.json'))
    except (OSError, KeyError, TypeError) as error:
        raise ValueError(str(error)) from error
    finally:
        os.close(fd)


def conserve(before, after_install, after_drain, queries, candidate_ack_events, *, after_reboot=None, reboot_ack_events=(), native_configuration_transition=False):
    paths = before.get('fixture_paths')
    selected = lambda row: paths is None or row['attributes'].get('log.file.path') in paths
    for snapshot in (after_install, after_drain):
        require(snapshot.get('fixture_paths') == paths, 'fixture paths changed')
        require(all(snapshot[key] == before[key] for key in ('binding', 'node_id', 'generation', 'boot_id')),
                'candidate/Strand/boot substitution')
    require(all(before[key] == after_install[key] for key in ('ack', 'files', 'sources', 'cursors', 'logs')),
            'stopped install changed retained custody')
    require(type(native_configuration_transition) is bool, 'native configuration transition scope must be boolean')
    if native_configuration_transition:
        require(after_reboot is not None, 'native configuration transition requires four stopped checkpoints')
        native_fixture_paths = (NATIVE_CONFIGURATION_LOGS[0],)
        require(paths == native_fixture_paths and all(snapshot.get('fixture_paths') == native_fixture_paths
            for snapshot in (after_install, after_drain, after_reboot)), 'native configuration transition fixture path')
        checkpoints = ((before, 1, False), (after_install, 1, False),
                       (after_drain, 2, True), (after_reboot, 4, True))
        metadata_profile = None
        for snapshot, revision, paused in checkpoints:
            raw = snapshot.get('applied_configuration')
            entry = snapshot.get('files', {}).get('applied-config.json')
            info = snapshot.get('file_observations', {}).get('applied-config.json')
            require(type(raw) is bytes and type(entry) is dict and type(info) in (tuple, list)
                and len(info) == 9 and all(type(value) is int for value in info)
                and len(raw) <= 65536 and stat.S_ISREG(info[2]) and info[5] == 1 and info[6] == len(raw)
                and stat.S_IMODE(info[2]) == 0o600
                and hashlib.sha256(raw).hexdigest() == entry.get('sha256') and len(raw) == entry.get('size')
                and (info[2], info[3], info[4]) == (entry.get('mode'), entry.get('uid'), entry.get('gid')),
                'native applied configuration bytes/hash/metadata join mismatch')
            # stopped_snapshot already independently validated service ownership against its receipt.
            # Reparse the retained raw bytes here; do not trust a cached profile view.
            value = applied_configuration(raw, info, {'uid': info[3], 'gid': info[4]})
            expected = dict(revision=revision, paused=paused,
                logs=list(NATIVE_CONFIGURATION_LOGS), metric_interval_s=NATIVE_CONFIGURATION_INTERVAL_S)
            require(value == expected, 'native applied configuration profile transition mismatch')
            marker = snapshot.get('coverage_pause')
            marker_entry = snapshot['files'].get('coverage-unknown')
            require((marker is None) == (marker_entry is None), 'native pause marker inventory join')
            if marker is not None:
                require(paused, 'native pause marker before pause')
                marker_info = snapshot['file_observations']['coverage-unknown']
                require(hashlib.sha256(marker['raw']).hexdigest() == marker_entry['sha256']
                    and len(marker['raw']) == marker_entry['size']
                    and (marker_info[2], marker_info[3], marker_info[4])
                        == (marker_entry['mode'], marker_entry['uid'], marker_entry['gid']),
                    'native pause marker raw inventory join')
                pause_marker(marker['raw'], marker_info, {'uid': info[3], 'gid': info[4]},
                    value, marker['observation'], snapshot['boot_id'])
            current_metadata = (stat.S_IMODE(info[2]), info[3], info[4], info[5], stat.S_ISREG(info[2]))
            require(metadata_profile is None or current_metadata == metadata_profile,
                    'native applied configuration metadata changed across checkpoints')
            metadata_profile = current_metadata
    else:
        require(all(snapshot['files'].get('applied-config.json') == before['files'].get('applied-config.json')
            for snapshot in (after_drain,) + ((after_reboot,) if after_reboot is not None else ())),
            'registered lifecycle applied configuration changed')
    pending = [source for source in before['sources'] if source['sequence'] > before['ack']]
    require(pending and after_drain['ack'] >= pending[-1]['sequence'], 'missing pending custody/drain ACK')
    require(all(log == after_drain['logs'].get(path) for path, log in before['logs'].items()
        if paths is None or path in paths), 'source archive rewritten')
    def preserve_diagnostics(prior, drained, stable_namespace=False):
        same = lambda a, b: a['inode'] == b['inode'] and (stable_namespace or a['device'] == b['device'])
        active, new = prior['spindle.log'], drained['spindle.log']
        if same(active, new):
            old, retained_old = prior.get('spindle.log.1'), drained.get('spindle.log.1')
            unchanged = old == retained_old or (stable_namespace and old is not None and retained_old is not None
                and all(value == retained_old.get(key) for key, value in old.items() if key != 'device'))
            require(new['bytes'].startswith(active['bytes']) and unchanged,
                    'diagnostic prefix preservation')
        else:
            old = drained.get('spindle.log.1', {})
            first = new['bytes'].split(b'\n', 1)[0] + b'\n'
            require(old and same(active, old) and old['bytes'].startswith(active['bytes'])
                and b'\n' in new['bytes'] and len(first) <= 4096 and len(old['bytes']) + len(first) > 262144,
                    'diagnostic rotation preservation')
    if paths:
        preserve_diagnostics(before['diagnostics'], after_drain['diagnostics'])
    if after_reboot is not None:
        require(paths is not None and all(after_reboot[key] == before[key] for key in
            ('binding', 'node_id', 'generation', 'fixture_paths')) and after_reboot['boot_id'] == queries['boot_after']
            and after_reboot['boot_id'] != before['boot_id'] and after_reboot['ack'] >= after_drain['ack'], 'reboot checkpoint binding')
        require(all(all(value == after_reboot['logs'].get(path, {}).get(key) for key, value in log.items()
            if key != 'device' or log['btrfs_identity'] is None) for path, log in before['logs'].items() if path in paths),
                'reboot fixture archive rewritten')
        diagnostic = '/var/lib/fabrico11y/node/spool/diagnostics/spindle.log'
        namespace = after_drain['logs'][diagnostic]['btrfs_identity']
        require(namespace == after_reboot['logs'][diagnostic]['btrfs_identity'], 'reboot diagnostic namespace changed')
        preserve_diagnostics(after_drain['diagnostics'], after_reboot['diagnostics'], namespace is not None)
    retained = {source['sequence']: source['sha256'] for source in before['sources']}
    require(all(source['sequence'] not in retained or source['sha256'] == retained[source['sequence']]
        for source in after_drain['sources']), 'retained committed bytes changed')
    def sentinel(snapshot, body):
        rows = [row for source in snapshot['sources'] for row in source['rows'] if selected(row) and row['body'] == body]
        require(len(rows) == 1, 'missing/duplicate raw sentinel')
        row, attrs = rows[0], rows[0]['attributes']
        path = attrs.get('log.file.path')
        require(path in snapshot['logs'] and path in snapshot['cursors'], 'sentinel source absent')
        log = snapshot['logs'][path]
        start, end = int(attrs['log.file.offset.start']), int(attrs['log.file.offset.end'])
        require(0 <= start < end <= snapshot['cursors'][path]['offset']
            and (start == 0 or log['bytes'][start - 1:start] == b'\n')
            and log['bytes'][start:end] == (body + '\n').encode()
            and attrs['log.file.device'] == str(log['device']) and attrs['log.file.inode'] == str(log['inode']),
                'sentinel exact source interval mismatch')
        return row, path, start, end
    pending_row, path, _, end = sentinel(before, queries['pending_body'])
    require(pending_row['sequence'] > before['ack'], 'sentinel already acknowledged')
    require(not any(selected(row) and row['body'] == queries['unread_body'] for source in before['sources'] for row in source['rows']),
            'unread sentinel already committed')
    require(before['cursors'][path]['offset'] == end
        and before['logs'][path]['bytes'][end:] == (queries['unread_body'] + '\n').encode(), 'unread suffix mismatch')
    unread_row, unread_path, start, unread_end = sentinel(after_drain, queries['unread_body'])
    require(unread_path == path and start == end and unread_row['sequence'] > before['sources'][-1]['sequence']
        and after_drain['cursors'][path]['offset'] == unread_end == after_drain['logs'][path]['size'], 'unread not drained')
    old = [row for row in queries['before'] if selected(row)]
    require(all(row['node'] == 'spindle-1' and row['node_id'] == before['node_id']
        and row['sequence'] <= before['ack'] for row in queries['before']), 'old-row baseline identity')
    require(len(old) == 3 and queries['pending_body'] != queries['unread_body']
        and not any(row['body'] in (queries['pending_body'], queries['unread_body']) for row in old), 'invalid old-row baseline')
    require(all(queries['before'].count(row) == 1 for source in before['sources']
        if source['sequence'] <= before['ack'] for row in source['rows']), 'old-row baseline raw mismatch')
    expected = old + [pending_row, unread_row]
    require(all(row in old + [pending_row] for source in pending for row in source['rows'] if selected(row)), 'unexpected pending log row')
    def raw_union(snapshots):
        all_sources = {}
        for snapshot in snapshots:
            for source in snapshot['sources']:
                previous = all_sources.setdefault(source['sequence'], source)
                require(previous == source, 'conflicting retained raw identity')
        rows = [row for source in all_sources.values() if source['sequence'] > before['ack'] for row in source['rows']]
        require(len({(row['node_id'], row['sequence'], row['index']) for row in rows}) == len(rows), 'duplicate raw row identity')
        require(not any(path in gap for source in all_sources.values() if source['sequence'] > before['ack']
            for gap in source['collection_gaps'] for path in paths), 'fixture source gap')
        return all_sources, rows
    for path, current in after_drain['cursors'].items():
        if paths is not None and path not in paths: continue
        log, offset = after_drain['logs'][path], 0
        for row in sorted((row for row in expected if row['attributes'].get('log.file.path') == path),
                          key=lambda row: int(row['attributes']['log.file.offset.start'])):
            attrs = row['attributes']
            start, end = (int(attrs[key]) for key in ('log.file.offset.start', 'log.file.offset.end'))
            require(start == offset and start < end <= current['offset']
                and log['bytes'][start:end] == (row['body'] + '\n').encode()
                and attrs['log.file.device'] == str(log['device']) and attrs['log.file.inode'] == str(log['inode']),
                    'source prefix conservation')
            offset = end
        require(offset == current['offset'], 'source prefix conservation')
    keys = lambda rows: sorted((row['node_id'], row['sequence'], row['index']) for row in rows)
    encoded = lambda rows: sorted(json.dumps(row, sort_keys=True, allow_nan=False) for row in rows)
    require(len(set(keys(expected))) == len(expected), 'duplicate expected identity')
    for phase, snapshots in (('after_drain', (before, after_install, after_drain)),
                             ('after_reboot', (before, after_install, after_drain) + ((after_reboot,) if after_reboot else ()))):
        observed = queries[phase]
        complete = queries['before'] + raw_union(snapshots)[1] if paths else expected
        projected = [row for row in observed if selected(row)]
        require(keys(projected) == keys(expected) and encoded(projected) == encoded(expected), 'exact row conservation')
        require(keys(observed) == keys(complete) and encoded(observed) == encoded(complete), 'exact row conservation')
    require(queries['boot_before'] == before['boot_id'] and queries['boot_after']
        and queries['boot_after'] != queries['boot_before'], 'no actual reboot identity')
    def ack_join(snapshot, required, events):
        process = snapshot['candidate_process']
        require(isinstance(process, dict) and all(process.get(key) == before['binding'][key] for key in BINDING)
            and process.get('unit') == UNITS[1] and type(process.get('pid')) is int and process['pid'] > 0
            and process.get('boot_id') == snapshot['boot_id'] and type(process.get('start_ns')) is int
            and type(process.get('end_ns')) is int and 0 <= process['start_ns'] <= process['end_ns'], 'candidate sender not bound')
        witnessed = set()
        for event in events:
            match = re.fullmatch(r'delivery sequence=([0-9]+) sha256=([0-9a-f]{64}) status=ack committed_through=([0-9]+) elapsed_us=([0-9]+)', event['message'])
            if not match:
                continue
            sequence, digest, through, _ = match.groups()
            sequence, through = int(sequence), int(through)
            require(all(event.get(key) == process[key] for key in ('unit', 'pid', 'boot_id'))
                and type(event.get('at_ns')) is int and process['start_ns'] <= event['at_ns'] <= process['end_ns'], 'ACK sender/window mismatch')
            if sequence in required:
                require(digest == required[sequence] and sequence <= through <= snapshot['ack'], 'ACK exact bytes/cursor mismatch')
                witnessed.add(sequence)
        require(witnessed == set(required), 'missing candidate exact-byte ACK')
    required = {source['sequence']: source['sha256'] for source in pending}
    required[unread_row['sequence']] = next(source['sha256'] for source in after_drain['sources']
                                          if source['sequence'] == unread_row['sequence'])
    ack_join(after_drain, required, candidate_ack_events)
    if after_reboot is not None:
        process = after_reboot['candidate_process']
        require(isinstance(process, dict) and process.get('boot_id') != after_drain['candidate_process']['boot_id'], 'reboot sender not distinct')
        ack_join(after_reboot, {source['sequence']: source['sha256'] for source in after_reboot['sources']
            if source['sequence'] > after_drain['ack']}, reboot_ack_events)
