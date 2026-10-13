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
    'resource_metrics': {1: 2, 2: 'scope_metrics', 3: 2},
    'resource_traces': {1: 2, 2: 'scope_traces', 3: 2},
    'scope_metrics': {1: 2, 2: 'metric', 3: 2}, 'scope_traces': {1: 2, 2: 'span', 3: 2},
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
    types = SCHEMAS[schema]
    for number, values in fields(raw, types, depth).items():
        spec = types.get(number)
        if isinstance(spec, str):
            for _, value in values:
                if spec == 'utf8':
                    value.decode('utf-8')
                else:
                    require(depth < 100, 'protobuf message recursion depth exhausted')
                    validate(value, spec, depth + 1)


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


def log_rows(raw, batch):
    index = 0
    for _, resource in fields(raw, SCHEMAS['logs']).get(1, []):
        for _, scope in fields(resource, SCHEMAS['resource_logs']).get(2, []):
            for _, record in fields(scope, SCHEMAS['scope_logs']).get(2, []):
                f = fields(record, SCHEMAS['record'])
                projection = [b'\x59' + one(f, 11, b'\0' * 8), length_field(5, string_projection(f.get(5, [])))]
                for _, attribute in f.get(6, []):
                    kv = fields(attribute, SCHEMAS['kv'])
                    projection.append(length_field(6, length_field(1, one(kv, 1, b''))
                        + length_field(2, string_projection(kv.get(2, [])))))
                row = oracle.decode_log_record(b''.join(projection))
                yield dict(node='spindle-1', node_id=batch['node_id'].hex(), sequence=batch['sequence'],
                    index=index, observed_ns=row['observed_time_unix_nano'], body=row['body'], attributes=row['attributes'])
                index += 1


def cursor(raw):
    f = fields(raw, {1: 2, 2: 0, 3: 0, 4: 0, 5: 0, 6: 0, 7: 0, 8: 2})
    path = one(f, 1, b'').decode('utf-8')
    require(len(path.encode()) <= 4096, 'cursor path cap')
    identity = None
    if 8 in f:
        stable = fields(b''.join(value for _, value in f[8]), {1: 2, 2: 0})
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
        batch = dict(version=one(f, 1), node_id=one(f, 2, b''), generation=one(f, 3), sequence=one(f, 4))
        for signal, schema in zip(signals, ('metrics', 'logs', 'traces')):
            validate(signal, schema)
        rows = list(log_rows(signals[1], batch))
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


def stopped_snapshot(root, stopped_receipt, archived_allowed_logs):
    root = owned(root)
    receipt = copy.deepcopy(stopped_receipt)
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
        require(all(name in ('identity', 'acked', 'writer.lock', 'batches.faj')
            or re.fullmatch(r'sealed-[0-9]{20}\.faj', name) for name in names), 'unexpected or failed Spool input')
        observed, files, raw_files = {}, {}, {}
        require(sum((root / name).lstat().st_size for name in names if name.endswith('.faj')) <= cap - 4096,
                'framed Spool cap')
        for name in names:
            raw, info = read_regular(root / name, cap - 4096 if name.endswith('.faj') else 28)
            raw_files[name], observed[name] = raw, info
            files[name] = dict(sha256=hashlib.sha256(raw).hexdigest(), size=len(raw),
                              mode=info[2], uid=info[3], gid=info[4])
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
                                    payload=payload, rows=rows))
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
        require(owned(root) == root and sorted(os.listdir(fd)) == names and identity(os.fstat(fd)) == root_before
            and identity(root.lstat()) == root_before, 'Spool changed during snapshot')
        require(all(identity((root / name).lstat()) == info for name, info in observed.items()), 'Spool file drift')
        require(all(identity(owned(path).lstat()) == info for path, info in log_observations.items()), 'archive drift')
        return dict(binding=binding, boot_id=receipt['boot_id'], node_id=node.hex(), generation=generation,
            ack=ack, files=files, file_observations=observed, sources=sources, cursors=cursors,
            logs=logs, log_observations=log_observations, candidate_process=receipt.get('candidate_process'))
    except (OSError, KeyError, TypeError) as error:
        raise ValueError(str(error)) from error
    finally:
        os.close(fd)


def conserve(before, after_install, after_drain, queries, candidate_ack_events):
    for snapshot in (after_install, after_drain):
        require(all(snapshot[key] == before[key] for key in ('binding', 'node_id', 'generation', 'boot_id')),
                'candidate/Strand/boot substitution')
    require(all(before[key] == after_install[key] for key in ('ack', 'files', 'sources', 'cursors', 'logs')),
            'stopped install changed retained custody')
    pending = [source for source in before['sources'] if source['sequence'] > before['ack']]
    require(pending and after_drain['ack'] >= pending[-1]['sequence'], 'missing pending custody/drain ACK')
    require(before['logs'] == after_drain['logs'], 'source archive rewritten')
    retained = {source['sequence']: source['sha256'] for source in before['sources']}
    require(all(source['sequence'] not in retained or source['sha256'] == retained[source['sequence']]
        for source in after_drain['sources']), 'retained committed bytes changed')
    def sentinel(snapshot, body):
        rows = [row for source in snapshot['sources'] for row in source['rows'] if row['body'] == body]
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
    require(not any(row['body'] == queries['unread_body'] for source in before['sources'] for row in source['rows']),
            'unread sentinel already committed')
    require(before['cursors'][path]['offset'] == end
        and before['logs'][path]['bytes'][end:] == (queries['unread_body'] + '\n').encode(), 'unread suffix mismatch')
    unread_row, unread_path, start, unread_end = sentinel(after_drain, queries['unread_body'])
    require(unread_path == path and start == end and unread_row['sequence'] > before['sources'][-1]['sequence']
        and after_drain['cursors'][path]['offset'] == unread_end == after_drain['logs'][path]['size'], 'unread not drained')
    old = queries['before']
    require(len(old) == 3 and queries['pending_body'] != queries['unread_body']
        and not any(row['body'] in (queries['pending_body'], queries['unread_body']) for row in old), 'invalid old-row baseline')
    expected = old + [pending_row, unread_row]
    require(all(row in old + [pending_row] for source in pending for row in source['rows']), 'unexpected pending log row')
    keys = lambda rows: sorted((row['node_id'], row['sequence'], row['index']) for row in rows)
    encoded = lambda rows: sorted(json.dumps(row, sort_keys=True, allow_nan=False) for row in rows)
    require(len(set(keys(expected))) == len(expected), 'duplicate expected identity')
    for observed in (queries['after_drain'], queries['after_reboot']):
        require(keys(observed) == keys(expected) and encoded(observed) == encoded(expected), 'exact row conservation')
    require(queries['boot_before'] == before['boot_id'] and queries['boot_after']
        and queries['boot_after'] != queries['boot_before'], 'no actual reboot identity')
    process = after_drain['candidate_process']
    require(isinstance(process, dict) and all(process.get(key) == before['binding'][key] for key in BINDING)
        and process.get('unit') == UNITS[1] and type(process.get('pid')) is int and process['pid'] > 0
        and process.get('boot_id') == after_drain['boot_id'] and type(process.get('start_ns')) is int
        and type(process.get('end_ns')) is int and 0 <= process['start_ns'] <= process['end_ns'], 'candidate sender not bound')
    required = {source['sequence']: source['sha256'] for source in pending}
    required[unread_row['sequence']] = next(source['sha256'] for source in after_drain['sources']
                                          if source['sequence'] == unread_row['sequence'])
    witnessed = set()
    for event in candidate_ack_events:
        match = re.fullmatch(r'delivery sequence=([0-9]+) sha256=([0-9a-f]{64}) status=ack committed_through=([0-9]+) elapsed_us=([0-9]+)', event['message'])
        if not match:
            continue
        sequence, digest, through, _ = match.groups()
        sequence, through = int(sequence), int(through)
        require(all(event.get(key) == process[key] for key in ('unit', 'pid', 'boot_id'))
            and type(event.get('at_ns')) is int and process['start_ns'] <= event['at_ns'] <= process['end_ns'], 'ACK sender/window mismatch')
        if sequence in required:
            require(digest == required[sequence] and sequence <= through <= after_drain['ack'], 'ACK exact bytes/cursor mismatch')
            witnessed.add(sequence)
    require(witnessed == set(required), 'missing candidate exact-byte ACK')
