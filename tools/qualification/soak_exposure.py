"""Registered R3 observation checks; callers supply independently admitted inputs.

No clock is read by the six validators. Their success is not a native run,
receipt authentication or permission to delete evidence.
"""
import ctypes
import errno
import os
from pathlib import Path
import re
import stat

NS = 1_000_000_000
EDGES = (60, 853, 1646, 2440, 3233, 4026, 4820, 5613, 6406, 7200)
CEILINGS = {'setup': 170, 'exposure': 7210, 'drain_seal': 60,
            'shutdown': 20, 'checkpoint': 20, 'closeout': 20}
HEADROOM = {name: seconds for name, seconds in CEILINGS.items() if name != 'exposure'}
HEADROOM['exposure_transition'] = 10
PARENT_FIELDS = ('memory.high', 'memory.max', 'memory.swap.max', 'pids.max',
    'memory.events', 'memory.peak', 'memory.current', 'memory.swap.current',
    'pids.current', 'pids.peak', 'cgroup.events', 'cgroup.procs', 'cpu.max', 'cpu.stat', 'io.stat')


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def sha256(value):
    return isinstance(value, str) and re.fullmatch('[0-9a-f]{64}', value) is not None


def cohort_ns(timestamp, began):
    require(type(timestamp) is int and type(began) is int, 'cohort timestamps')
    offset = timestamp - began
    return next((index for index, (lo, hi) in enumerate(zip(EDGES, EDGES[1:]))
                 if lo * NS <= offset < hi * NS), None)


def validate_exposure(sim, interval):
    require(isinstance(sim, dict) and isinstance(interval, dict), 'exposure evidence')
    require(all(type(sim.get(key)) is int for key in ('seconds', 'identities', 'seed', 'began_unix_ns'))
        and all(type(interval.get(key)) is int for key in ('start_ns', 'end_ns', 'sim_began_unix_ns', 'last_offered_second')),
        'exposure evidence')
    edges = interval.get('cohort_edges_s')
    require(sim['seconds'] == 7200 and sim['identities'] == 100 and sim['seed'] == 0xA11FA001
        and sim['began_unix_ns'] >= 0 and interval['sim_began_unix_ns'] == sim['began_unix_ns']
        and 0 <= interval['start_ns'] <= interval['end_ns'] - 7200 * NS
        and isinstance(edges, (list, tuple)) and all(type(value) is int for value in edges) and tuple(edges) == EDGES
        and interval['last_offered_second'] == 7199 and interval.get('probes_active') is True,
        'exposure evidence')
    return True


def remaining_ns(phase, launcher_begin, phase_begin, now):
    require(phase in CEILINGS and all(type(value) is int for value in (launcher_begin, phase_begin, now))
        and 0 <= launcher_begin <= phase_begin <= now, 'phase timestamps')
    remaining = min(CEILINGS[phase] * NS - (now - phase_begin), 7500 * NS - (now - launcher_begin))
    require(remaining > 0, 'phase budget')
    return remaining


def validate_phase_headroom(measurements):
    require(isinstance(measurements, dict) and set(measurements) == set(HEADROOM), 'phase headroom')
    for phase, record in measurements.items():
        require(isinstance(record, dict), 'phase headroom')
        maximum = record.get('maximum_ns')
        require(type(maximum) is int and maximum > 0
            and (5 * maximum + 3) // 4 <= HEADROOM[phase] * NS
            and record.get('kind') in ('native-r2', 'full-rate-smoke')
            and record.get('stop_confirmed') is True and sha256(record.get('source_sha256')), 'phase headroom')
    return True


def validate_checkpoint(measurement, runtime, outer, expected):
    refs = {'measurement': measurement, 'runtime': runtime, 'outer': outer}
    require(isinstance(expected, dict) and isinstance(expected.get('receipt_sha256'), dict)
        and all(isinstance(ref, dict) for ref in refs.values()), 'checkpoint binding')
    require(all(sha256(ref.get('sha256')) and ref['sha256'] == expected['receipt_sha256'].get(name)
        and isinstance(ref.get('value'), dict) for name, ref in refs.items()), 'checkpoint binding')
    m, r, o = (ref['value'] for ref in (measurement, runtime, outer))
    child = o.get('command_receipt', {})
    require(isinstance(child, dict) and bool(expected.get('unit')) and bool(expected.get('cgroup')),
            'checkpoint binding')
    exits = r.get('production_exits')
    require(bool(expected.get('binding')) and m.get('state') == 'pending_acceptance' and 'passed' not in m
        and r.get('state') == 'closed_pending_acceptance' and 'passed' not in r
        and m.get('binding') == r.get('binding') == expected['binding']
        and r.get('cleanup_ok') is True and r.get('immutable_inputs') is True
        and isinstance(exits, dict) and set(exits) == {'server', 'simulator'}
        and all(type(value) is int and value == 0 for value in exits.values())
        and r.get('forced_shutdown') is False
        and o.get('unit') == expected.get('unit') and o.get('stop_confirmed') is True
        and bool(expected.get('command')) and o.get('command') == expected['command']
        and type(o.get('runtime_max_seconds')) is int and o['runtime_max_seconds'] == 7500
        and all(type(value) is int and value == 0 for value in (o.get('exit'), o.get('wrapper_exit'), child.get('exit')))
        and child.get('state') == 'completed' and child.get('cgroup') == expected.get('cgroup'), 'checkpoint binding')
    return True


def validate_parent(groups, stopped=False):
    require(isinstance(groups, dict), 'whole-parent containment')
    root = groups.get('.', {})
    require(isinstance(root, dict) and all(name in root and isinstance(root[name], str) for name in PARENT_FIELDS),
            'whole-parent containment')
    require(all(root[name] == value for name, value in {'memory.high': '5368709120',
        'memory.max': '6442450944', 'memory.swap.max': '0', 'pids.max': '1536'}.items()), 'whole-parent containment')
    try:
        events = dict(line.split() for line in root['memory.events'].splitlines())
        population = dict(line.split() for line in root['cgroup.events'].splitlines())
        require(events.get('oom') == events.get('oom_kill') == '0'
            and root['memory.swap.current'] == '0'
            and all(0 <= int(root[name]) <= cap for name, cap in
                (('memory.peak', 6442450944), ('memory.current', 6442450944),
                 ('pids.peak', 1536), ('pids.current', 1536)))
            and population.get('populated') in ('0', '1')
            and (not stopped or (population['populated'] == '0' and not root['cgroup.procs'].strip())),
            'whole-parent containment')
    except (ValueError, TypeError) as error:
        raise ValueError('whole-parent containment') from error
    return True


def require_no_mounts(root):
    root = Path(root).resolve(strict=True)
    for line in Path('/proc/self/mountinfo').read_text(errors='surrogateescape').splitlines():
        fields = line.split()
        require(len(fields) >= 10 and '-' in fields[6:], 'malformed mountinfo')
        encoded = fields[4]
        require(re.search(r'\\(?!040|011|012|134)', encoded) is None, 'malformed mountinfo escape')
        mount = Path(re.sub(r'\\(040|011|012|134)', lambda match: chr(int(match[1], 8)), encoded))
        require(mount.is_absolute() and '..' not in mount.parts, 'malformed mountinfo path')
        require(not mount.is_relative_to(root), 'mount at or below owned evidence root')


def canonical_directory(path):
    path = Path(path)
    require(path.is_absolute() and '..' not in path.parts and path.resolve(strict=True) == path,
            'stopped checkpoint needs canonical directories')
    cursor = Path('/')
    for part in path.parts[1:]:
        cursor /= part
        require(stat.S_ISDIR(cursor.lstat().st_mode), 'stopped checkpoint has linked or non-directory parent')
    require(path.stat().st_uid == os.getuid(), 'stopped checkpoint directory is not owned')
    return path


def checkpoint_tree(source, destination, *, processes_stopped=True):
    require(processes_stopped is True, 'stopped checkpoint requires confirmed processes')
    source, destination = Path(source), Path(destination)
    require(destination.is_absolute() and '..' not in destination.parts
        and not destination.exists() and not destination.is_symlink(), 'fresh checkpoint destination required')
    source = canonical_directory(source)
    source_parent, destination_parent = canonical_directory(source.parent), canonical_directory(destination.parent)
    require(not destination.is_relative_to(source), 'fresh checkpoint cannot be inside its source')
    before = source.stat()
    require(before.st_dev == destination_parent.stat().st_dev, 'same-filesystem checkpoint required')
    require_no_mounts(source)
    require_no_mounts(destination_parent)
    descriptors = []
    try:
        for path in (source_parent, destination_parent, source):
            descriptors.append(os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC))
        parent_fd, destination_fd, source_fd = descriptors
        identity = lambda info: (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_gid)
        source_stamp = lambda info: (*identity(info), info.st_nlink, info.st_size, info.st_mtime_ns, info.st_ctime_ns)
        require(source_stamp(before) == source_stamp(os.fstat(source_fd)) == source_stamp(source.lstat()),
                'stopped checkpoint source changed')
        require(all(identity(path.lstat()) == identity(os.fstat(fd)) for path, fd in
            ((source_parent, parent_fd), (destination_parent, destination_fd))), 'stopped checkpoint parent changed')
        require(os.fstat(source_fd).st_dev == os.fstat(destination_fd).st_dev, 'same-filesystem checkpoint required')
        libc = ctypes.CDLL(None, use_errno=True)
        rename = getattr(libc, 'renameat2', None)
        require(rename is not None, 'stopped checkpoint requires no-replace rename')
        rename.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint)
        rename.restype = ctypes.c_int
        if rename(parent_fd, os.fsencode(source.name), destination_fd, os.fsencode(destination.name), 1) != 0:
            code = ctypes.get_errno()
            if code == errno.EEXIST:
                raise ValueError('fresh checkpoint destination already exists')
            if code == errno.EXDEV:
                raise ValueError('same-filesystem checkpoint required')
            raise OSError(code, os.strerror(code))
        require(not source.exists() and identity(destination.lstat()) == identity(os.fstat(source_fd)), 'stopped checkpoint rename identity changed')
        os.fsync(parent_fd)
        os.fsync(destination_fd)
        return {'method': 'rename', 'source': str(source), 'destination': str(destination),
                'device': before.st_dev, 'inode': before.st_ino}
    finally:
        for fd in reversed(descriptors):
            os.close(fd)
