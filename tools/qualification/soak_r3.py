"""R3 measurement lifecycle. Pending evidence is graded only after outer stop.

The caller's independently pinned admission is authority, never a runtime's own
assertions. Missing native publication or browser clock binders fail closed.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import random
import signal
import stat
import subprocess
import sys
import threading
import time
from types import SimpleNamespace

import soak_exposure as exposure

NS = exposure.NS
PHASES = tuple(exposure.CEILINGS)
SEED = 0xA11FA001
PROFILE = {'identities': 100, 'workers': 100, 'seed': SEED, 'logs_per_second': 2,
           'log_bytes': 512, 'metrics_per_15_seconds': 32}


def require(value, message):
    if not value:
        raise ValueError(message)


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def pinned(path, expected):
    path = Path(path)
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1
            and info.st_uid == os.getuid() and exposure.sha256(expected), 'regular owned pinned input required')
    raw = path.read_bytes()
    require(hashlib.sha256(raw).hexdigest() == expected, 'pinned input changed')
    return json.loads(raw)


def write_new(path, value):
    with Path(path).open('x', encoding='utf-8') as stream:
        stream.write(json.dumps(value, sort_keys=True) + '\n')
        stream.flush()
        os.fsync(stream.fileno())


class PhaseClock:
    """One main-thread alarm also interrupts progressive reads and joins."""
    def __init__(self, launcher_begin, now=time.monotonic_ns):
        self.launcher_begin, self.now = launcher_begin, now
        self.records, self.phase, self.started = [], None, None
        self.boundary_hook = None

    def remaining(self):
        return exposure.remaining_ns(self.phase, self.launcher_begin, self.started, self.now())

    def enter(self, phase):
        require(phase in PHASES and PHASES.index(phase) == len(self.records) + (self.phase is not None),
                'ordered disjoint phases required')
        if self.phase is not None:
            self.finish()
        self.phase, self.started = phase, self.launcher_begin if phase == 'setup' else self.now()
        self.arm()
        if self.boundary_hook:
            self.call(self.boundary_hook, phase + '-begin')

    def arm(self):
        signal.setitimer(signal.ITIMER_REAL, self.remaining() / NS)

    def call(self, function, *args, **kwargs):
        self.arm()
        result = function(*args, **kwargs)
        self.remaining()
        return result

    def finish(self):
        self.remaining()
        if self.boundary_hook:
            self.call(self.boundary_hook, self.phase + '-end')
        ended = self.now()
        self.records.append({'phase': self.phase, 'start_ns': self.started,
                             'end_ns': ended, 'elapsed_ns': ended - self.started})
        self.phase = None
        signal.setitimer(signal.ITIMER_REAL, 0)


def expired(_signum, _frame):
    raise TimeoutError('R3 phase or global deadline exhausted')


def launcher_begin(parent):
    require(os.environ.get('FABRIC_RESOURCE_RUNTIME_SECONDS') == '7500', 'R3 requires exact 7500-second launcher')
    retained = subprocess.check_output(['systemctl', '--user', 'show', parent.name,
        '--property=RemainAfterExit', '--value'], timeout=2).strip()
    require(retained == b'yes', 'R3 requires retained service for actual final parent observation')
    raw = subprocess.check_output(['systemctl', '--user', 'show', parent.name,
        '--property=ActiveEnterTimestampMonotonic', '--value'], timeout=2).strip()
    require(raw.isdigit() and int(raw) > 0, 'native launcher monotonic activation missing')
    return int(raw) * 1000


def admission(path, expected):
    value = pinned(path, expected)
    require(value.get('protocol') == 'R3' and value.get('seed') == SEED
            and value.get('identities') == value.get('workers') == 100, 'registered R3 parameters required')
    require(type(value.get('smoke')) is bool, 'explicit smoke/full admission required')
    seconds = value.get('seconds')
    require(type(seconds) is int and (seconds == 7200 if not value['smoke'] else 60 < seconds < 7200),
            'offered seconds do not match admission')
    for record in value['inputs'].values():
        require(digest(record['path']) == record['sha256'], 'admitted input changed')
    # These are separately certified capabilities, not evidence of this future
    # run. Actual native/UI evidence is still required at closeout and grading.
    for kind in ('native_publication', 'browser_continuity'):
        reference = value.get(kind)
        require(isinstance(reference, dict), 'missing certified ' + kind + ' prerequisite')
        certificate = pinned(reference['path'], reference['sha256'])
        require(certificate.get('state') == 'certified' and certificate.get('kind') == kind
                and certificate.get('source_sha256') == value['inputs'][kind]['sha256'],
                'uncertified ' + kind + ' prerequisite')
    if not value['smoke']:
        validate_headroom_sources(value)
    return value


def stopped_outer(actual, expected, seconds):
    child = actual.get('command_receipt', {})
    require(actual.get('unit') == expected['unit'] and bool(expected['unit'])
            and actual.get('command') == expected['command'] and bool(expected['command'])
            and child.get('cgroup') == expected['cgroup'] and bool(expected['cgroup'])
            and actual.get('runtime_max_seconds') == seconds
            and actual.get('stop_confirmed') is True and child.get('state') == 'completed'
            and all(type(exit_code) is int and exit_code == 0 for exit_code in
                    (actual.get('exit'), actual.get('wrapper_exit'), child.get('exit'))),
            'exact stopped launcher binding missing')


def validate_headroom_sources(value):
    from soak_runtime_bindings import validate_job, validate_phases, validate_final_parent
    measurements = value.get('phase_headroom', {})
    exposure.validate_phase_headroom(measurements)
    for phase, record in measurements.items():
        source = pinned(record['source_receipt'], record['source_sha256'])
        stopped_outer(source, record['outer_expected'], record['outer_runtime_seconds'])
        runtime = pinned(record['runtime_path'], record['runtime_sha256'])
        validate_job(runtime, source, record['runtime_expected'])
        validate_phases(runtime, source['activation_monotonic_ns'])
        validate_final_parent(runtime, source)
        require(runtime.get('state') == 'closed_pending_acceptance'
                and runtime.get('cleanup_ok') is True and runtime.get('forced_shutdown') is False
                and runtime.get('immutable_inputs') is True and runtime.get('profile') == PROFILE
                and runtime.get('headroom_binding') == value['headroom_binding']
                and bool(value['headroom_binding'])
                and runtime.get('production_exits') == {'simulator': 0, 'server': 0},
                'S1 source runtime binding missing')
        measurement_path = Path(record['runtime_path']).parent / 'measurement.json'
        measured = pinned(measurement_path, runtime['measurement_sha256'])
        checkpoint = measurement_path.parent
        require(measured['admission_sha256'] == runtime['admission_sha256']
                and measured['binding'] == runtime['binding'], 'S1 runtime/measurement mismatch')
        for label, members in measured['members'].items():
            require(inventory(checkpoint / label) == members, 'S1 stopped source changed')
        sim = json.loads((checkpoint / 'raw/sim/sim-summary.json').read_text())
        require(sim['seconds'] == runtime['offered_seconds'] and sim['identities'] == sim['workers'] == 100
                and sim['seed'] == SEED and sim['undelivered'] == 0,
                'S1 actual simulator profile mismatch')
        offered = bytearray(100 * sim['seconds'])
        for event in rows(checkpoint / 'raw/sim/events.jsonl'):
            if event.get('e') != 'created':
                continue
            identity, seq = event['id'], event['seq']
            require(type(identity) is int and type(seq) is int and 0 <= identity < 100
                    and 1 <= seq <= sim['seconds'], 'S1 malformed native offer')
            slot = identity * sim['seconds'] + seq - 1
            require(not offered[slot], 'S1 duplicate native offer')
            offered[slot] = 1
        require(all(offered), 'S1 missing native worker offers')
        require(runtime.get('smoke') is True if record['kind'] == 'full-rate-smoke'
                else runtime.get('protocol') == 'R2', 'S1 source kind mismatch')
        durations = {}
        previous = None
        for item in runtime['phases']:
            name, began, ended = item['phase'], item['start_ns'], item['end_ns']
            require(name not in durations and type(began) is int and type(ended) is int
                    and 0 <= began < ended and (previous is None or previous <= began)
                    and ended - began == item['elapsed_ns'], 'S1 source phase timestamps invalid')
            durations[name], previous = ended - began, ended
        if phase == 'exposure_transition':
            actual = durations['exposure'] - runtime['offered_seconds'] * NS
        else:
            actual = durations[phase]
        require(actual == record['maximum_ns'], 'S1 supplied maximum differs from raw phase')
    return True


def inventory(root):
    root = exposure.canonical_directory(root)
    exposure.require_no_mounts(root)
    records = {}
    for path in sorted(root.rglob('*')):
        info = path.lstat()
        require(info.st_uid == os.getuid() and info.st_dev == root.stat().st_dev,
                'foreign checkpoint member')
        require(stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode), 'linked or special checkpoint member')
        if path.is_file():
            require(info.st_nlink == 1, 'linked checkpoint file')
            records[str(path.relative_to(root))] = {'bytes': info.st_size, 'sha256': digest(path)}
    return records


def process_identity(pid):
    root = Path('/proc') / str(pid)
    info = (root / 'exe').stat()
    # comm may contain spaces or ')'; the final ')' terminates field two.
    fields = (root / 'stat').read_text().rsplit(')', 1)[1].split()
    return {'pid': pid, 'start_ticks': int(fields[19]),
        'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
        'exe_device': info.st_dev, 'exe_inode': info.st_ino}


class Probes:
    """Prelaunch readiness and complete append-only per-consumer raw ledgers."""
    def __init__(self, fixture, raw, clock):
        self.fixture, self.raw, self.clock = fixture, raw, clock
        self.stop = threading.Event()
        self.ready = {name: threading.Event() for name in ('query', 'management', 'sampler')}
        self.started, self.ready_ns, self.ended, self.errors, self.threads = {}, {}, {}, [], []
        self.inflight, self.cadences = {}, {}
        self.outputs = {name: (raw / (name + '.jsonl')).open('x') for name in self.ready}
        for name in ('query-http', 'management-http'):
            self.outputs[name] = (raw / (name + '.jsonl')).open('x')
        def reader_raw(record):
            record = dict(record)
            record['raw_base64'] = base64.b64encode(record.pop('raw')).decode()
            self.append('query-http', record)
        fixture.query_reader.raw_observer = reader_raw
        fixture.bridge.raw_observer = lambda record: self.append('management-http', record)

    def append(self, name, value):
        self.outputs[name].write(json.dumps(value, sort_keys=True) + '\n')
        self.outputs[name].flush()

    def cycle(self, name, cadence, operation):
        self.started[name] = time.monotonic_ns()
        target = self.started[name]
        try:
            while not self.stop.is_set():
                began = time.monotonic_ns()
                self.inflight[name] = began
                require(self.fixture.server.poll() is None, 'production server exited during probes')
                operation(began)
                self.ready_ns.setdefault(name, time.monotonic_ns())
                self.inflight[name] = None
                self.ready[name].set()
                target += cadence * NS
                require(time.monotonic_ns() <= target, name + ' missed cadence')
                self.stop.wait(max(0, target - time.monotonic_ns()) / NS)
        except Exception as error:
            self.append(name, {'error': repr(error), 'at_ns': time.monotonic_ns()})
            self.errors.append(name + ': ' + repr(error))
            self.stop.set()
        finally:
            self.ended[name] = time.monotonic_ns()

    def start(self):
        randomizer = random.Random(SEED)
        def query(began):
            now = time.time_ns()
            body = {'kind': 'logs', 'node': 'sim%04d' % randomizer.randrange(100),
                    'from_ns': now - 30 * NS, 'to_ns': now, 'limit': 100}
            for index in range(1000):
                answer = self.fixture.query_call('POST', body)
                self.append('query', {'started_ns': began, 'ended_ns': time.monotonic_ns(),
                    'page': index, 'request': body, 'answer': answer})
                require(answer.get('complete') is True, 'incomplete query page')
                if not answer.get('next_page'):
                    return
                body = dict(body, page=answer['next_page'])
            raise ValueError('query pagination exceeds finite page bound')
        def management(began):
            for method, suffix, body in [('GET', '', None), ('PUT', '/sim0000/config',
                    {'logs': [], 'metric_interval_s': 15})]:
                answer = self.fixture.call(method, suffix, body)
                self.append('management', {'started_ns': began, 'ended_ns': time.monotonic_ns(),
                    'method': method, 'suffix': suffix, 'answer': answer})
        def sample(began):
            from fleet_tier import cpu_s, status_kib
            import cgroups
            self.fixture.sample_bound()
            groups = cgroups.snapshot(self.fixture.parent)
            exposure.validate_parent(groups)
            # Native browser-clock/session/document witness is provided by the
            # separately certified bridge. Counter-only health is insufficient.
            ui = self.fixture.bridge.continuity_snapshot()
            before = time.monotonic_ns()
            wall = time.time_ns()
            after = time.monotonic_ns()
            self.append('sampler', {'started_ns': began, 'ended_ns': time.monotonic_ns(),
                'mono_before_ns': before, 'wall_ns': wall, 'mono_after_ns': after,
                'rss_kib': status_kib(self.fixture.server.pid, 'VmRSS'),
                'cpu_s': cpu_s(self.fixture.server.pid), 'groups': groups, 'ui': ui})
        for name, cadence, operation in [('query', 10, query), ('management', 60, management), ('sampler', 5, sample)]:
            self.cadences[name] = cadence
            thread = threading.Thread(target=self.cycle, args=(name, cadence, operation), daemon=True)
            self.threads.append(thread)
            thread.start()
        for event in self.ready.values():
            while not event.wait(min(.05, self.clock.remaining() / NS)):
                require(not self.errors, 'probe readiness failed: ' + repr(self.errors))

    def check(self):
        require(not self.errors and all(thread.is_alive() for thread in self.threads),
                'probe continuity lost: ' + repr(self.errors))
        supervisor = self.fixture.bridge.supervisor
        require(supervisor is not None and supervisor.poll() is None,
                'browser supervisor died; whole campaign stop required')
        now = time.monotonic_ns()
        require(all(start is None or now - start <= self.cadences[name] * NS
                    for name, start in self.inflight.items()), 'probe blocked beyond its cadence')

    def close(self):
        self.stop.set()
        for thread in self.threads:
            thread.join(timeout=self.clock.remaining() / NS)
            require(not thread.is_alive(), 'unconfirmed probe stop')
        for stream in self.outputs.values():
            stream.flush()
            os.fsync(stream.fileno())
            stream.close()
        self.fixture.query_reader.raw_observer = None
        self.fixture.bridge.raw_observer = None


def require_empty(groups):
    for group in groups:
        require(not (group / 'cgroup.procs').read_text().strip()
                and 'populated 0' in (group / 'cgroup.events').read_text().splitlines(),
                'production/browser/simulator group remains populated')


def measure(value, admission_sha):
    from production_access import ProductionAccess
    from console_bridge import ConsoleBridge
    from resource_group import require_limits
    require(callable(getattr(ConsoleBridge, 'continuity_snapshot', None)),
            'native browser clock/session/document binder unavailable')
    try:
        from simulator_native_trace import NativeTrace
    except ImportError as error:
        raise ValueError('native publication trace observer unavailable') from error
    require(getattr(ConsoleBridge, 'supervised_cleanup', False),
            'isolated supervised browser lifecycle unavailable')
    parent = require_limits()
    clock = PhaseClock(launcher_begin(parent))
    old_handler = signal.signal(signal.SIGALRM, expired)
    raw = Path(value['raw'])
    output = Path(value['output'])
    require(not raw.exists() and not output.exists(), 'fresh exclusive R3 destinations required')
    raw.parent.resolve(strict=True).relative_to(Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True))
    output.parent.resolve(strict=True).relative_to(Path(os.environ['FABRIC_STORAGE_ROOT']).resolve(strict=True) / 'results')
    raw.mkdir(mode=0o700)
    output.mkdir(mode=0o700)
    fixture = probes = sim = None
    parent_observations = []
    try:
        clock.enter('setup')
        clock.call(subprocess.run, ['systemctl', '--user', 'set-property', '--runtime', parent.name,
            'MemoryHigh=5G', 'MemoryMax=6G', 'MemorySwapMax=0', 'TasksMax=1536'], check=True,
            timeout=clock.remaining() / NS)
        import cgroups
        def parent_boundary(label):
            groups = cgroups.snapshot(parent)
            exposure.validate_parent(groups)
            parent_observations.append({'boundary': label, 'at_ns': time.monotonic_ns(), 'groups': groups})
        clock.boundary_hook = parent_boundary
        clock.call(parent_boundary, 'admission')
        args = SimpleNamespace(production_access=True, production_server_cpus='0-1',
            deb_receipt=Path(value['inputs']['deb']['path']), rpm_receipt=Path(value['inputs']['rpm']['path']),
            production_out=Path(value['fixture_output']), r3_browser_deadline=clock.launcher_begin / NS + 7500)
        fixture = clock.call(ProductionAccess, args, raw, soak=True)
        require(fixture.bridge.receipt.get('supervised_browser') is True and fixture.bridge.supervisor is not None,
                'R3 actual browser supervisor was not selected')
        fixture.bridge.continuous_polling = True
        fixture.bridge.ui_sample_limit = 2048
        fixture.bridge.r3_control_session = True
        from soak_tier import enroll_sources
        enrolled = clock.call(enroll_sources, fixture, 100)
        clock.call(fixture.issue_reader, [item['enrollment_id'] for item in enrolled], 7800)
        session = clock.call(fixture.bridge.session_view)
        require(type(session.get('expires_unix_s')) is int
                and session['expires_unix_s'] > time.time() + value['seconds'] + 90,
                'actual owner session cannot cover offered interval')
        tokens = raw / 'tokens'
        tokens.write_text('\n'.join(item['token'] for item in enrolled) + '\n')
        tokens.chmod(0o600)
        token_stamp, token_sha = tokens.stat(), digest(tokens)
        clock.call(fixture.bridge.poll_ui, True)
        probes = Probes(fixture, raw, clock)
        clock.call(probes.start)
        exposure.validate_parent(cgroups.snapshot(fixture.parent))
        clock.enter('exposure')
        helper = Path(value['inputs']['simulator']['path'])
        descriptor = os.open(helper, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
        try:
            require(digest('/proc/self/fd/' + str(descriptor)) == value['inputs']['simulator']['sha256'],
                    'held simulator ELF changed')
            command = ['taskset', '-c', '2-3', sys.executable, '-B',
                str(Path(__file__).parents[1] / 'bench/labs/completion/enter_group.py'),
                str(fixture.parent / 'supervisor'), '/proc/self/fd/' + str(descriptor),
                '--server-url', fixture.origin, '--ca', str(fixture.work / 'ca.pem'),
                '--tokens', str(tokens), '--seed', hex(SEED), '--seconds', str(value['seconds']),
                '--workers', '100', '--out', str(raw / 'sim'), '--begin-witness', str(raw / 'begin.json')]
            sim = clock.call(NativeTrace.start, command, descriptor, raw / 'native-trace',
                strace=Path(value['inputs']['strace']['path']),
                strace_sha256=value['inputs']['strace']['sha256'],
                deadline=clock.launcher_begin / NS + 7500)
            while not (raw / 'begin.json').exists():
                probes.check()
                require(sim.poll() is None, 'simulator exited before native begin')
                clock.call(time.sleep, .01)
            begin_sha = digest(raw / 'begin.json')
            begin = pinned(raw / 'begin.json', begin_sha)
            require(begin['process'] == process_identity(sim.target_pid)
                    and digest('/proc/%d/exe' % sim.target_pid) == value['inputs']['simulator']['sha256'],
                    'loaded native simulator identity changed')
            lower, upper = begin['mono_before_ns'], begin['mono_after_ns']
            require(type(lower) is int and type(upper) is int and 0 <= upper - lower <= 1_000_000,
                    'native begin clock bracket invalid')
            require(all(start <= lower for start in probes.ready_ns.values()), 'probes became ready after native exposure')
            boundary = upper + value['seconds'] * NS
            while time.monotonic_ns() < boundary:
                probes.check()
                if sim.poll() is not None:
                    require(time.monotonic_ns() >= boundary, 'native simulator exited before conservative exposure end')
                    break
                clock.call(time.sleep, min(.05, (boundary - time.monotonic_ns()) / NS))
            observed_end = time.monotonic_ns()
            probes.check()
            require(digest(raw / 'begin.json') == begin_sha, 'native begin bytes changed')
        finally:
            os.close(descriptor)
        clock.enter('drain_seal')
        sim_exit = clock.call(sim.wait, timeout=clock.remaining() / NS)
        require(sim_exit == 0, 'native simulator failed')
        native_observation = clock.call(sim.finish, raw / 'begin.json')
        require(sim.tracer_exit == 0, 'native tracer failed or remained alive')
        write_new(raw / 'native-observation.json', native_observation)
        journal = fixture.state_root / 'journal'
        while any(journal.glob('sealed-*.faj')):
            probes.check()
            clock.call(time.sleep, min(.1, clock.remaining() / NS))
        clock.call(probes.close)
        write_new(raw / 'ui-final.json', clock.call(fixture.bridge.continuity_snapshot))
        summary = json.loads((raw / 'sim/sim-summary.json').read_text())
        require(summary['seconds'] == value['seconds'] and summary['identities'] == 100
                and summary['seed'] == SEED and summary.get('workers') == 100
                and summary.get('undelivered') == 0
                and summary.get('elapsed_ns', 0) >= value['seconds'] * NS
                and summary.get('ended_monotonic_ns', 0) >= boundary,
                'native summary does not establish exposure')
        clock.enter('shutdown')
        fixture.bridge.shutdown_deadline = (clock.started + 20 * NS) / NS
        from fleet_tier import status_kib
        server_hwm = status_kib(fixture.server.pid, 'VmHWM')
        clock.call(fixture.bridge.poll_ui, False)
        fixture.server.send_signal(signal.SIGTERM)
        server_exit = clock.call(fixture.server.wait, timeout=clock.remaining() / NS)
        require(server_exit == 0, 'production shutdown failed')
        clock.call(fixture.close)
        require_empty(fixture.groups)
        require(fixture.receipt['cleanup_confirmed'] and fixture.receipt['immutable_inputs'],
                'production/browser cleanup not confirmed')
        current_token = tokens.lstat()
        require(stat.S_ISREG(current_token.st_mode) and current_token.st_nlink == 1
                and (current_token.st_dev, current_token.st_ino, current_token.st_uid) ==
                    (token_stamp.st_dev, token_stamp.st_ino, os.getuid())
                and digest(tokens) == token_sha, 'owned simulator credential changed')
        tokens.unlink()
        clock.enter('checkpoint')
        # Every rename keeps the stopped inode. No fallback copy or recursive
        # deletion can make a partially checkpointed run successful.
        moves = {}
        for label, source in [('raw', raw), ('work', fixture.work), ('fixture', fixture.out)]:
            destination = output / label
            moves[label] = clock.call(exposure.checkpoint_tree, source, destination, processes_stopped=True)
        members = {label: clock.call(inventory, output / label) for label in moves}
        record = {'state': 'pending_acceptance', 'protocol': 'R3', 'smoke': value['smoke'],
            'admission_sha256': admission_sha, 'binding': value['binding'], 'checkpoint': moves,
            'members': members, 'begin_sha256': begin_sha, 'native_begin': begin,
            'parent_observations': parent_observations,
            'server_hwm_kib': server_hwm,
            'credential_cleanup': {'tokens_removed': not tokens.exists(), 'sha256': token_sha},
            'exposure': {'start_ns': lower, 'end_ns': observed_end,
                         'conservative_end_ns': boundary, 'probes_started': probes.started,
                         'probes_ready': probes.ready_ns,
                         'probes_ended': probes.ended},
            'simulator_command': command, 'production_exits': {'simulator': sim_exit, 'server': server_exit}}
        clock.call(write_new, output / 'measurement.json', record)
        clock.enter('closeout')
        require_empty(fixture.groups)
        exposure.validate_parent(cgroups.snapshot(parent))
        closeout_start = clock.started
        clock.finish()
        # Keep final receipt I/O inside the same closeout/global budget.
        signal.setitimer(signal.ITIMER_REAL, exposure.remaining_ns('closeout', clock.launcher_begin,
            closeout_start, time.monotonic_ns()) / NS)
        write_new(output / 'runtime.json', {'state': 'closed_pending_acceptance',
            'admission_sha256': admission_sha, 'binding': value['binding'], 'phases': clock.records,
            'cleanup_ok': True, 'immutable_inputs': True, 'forced_shutdown': False,
            'parent_observations': parent_observations,
            'profile': PROFILE, 'smoke': value['smoke'], 'protocol': 'R3', 'offered_seconds': value['seconds'],
            'headroom_binding': value['headroom_binding'],
            'input_sha256': {name: ref['sha256'] for name, ref in value['inputs'].items()},
            'outer_identity': {'unit': parent.name.removesuffix('.service'), 'cgroup': str(parent),
                               'activation_monotonic_ns': clock.launcher_begin},
            'parent_identity': {'device': parent.stat().st_dev, 'inode': parent.stat().st_ino},
            'output_identity': {'path': str(output), 'device': output.stat().st_dev, 'inode': output.stat().st_ino},
            'native_interval': {'start_ns': lower, 'conservative_end_ns': boundary},
            'production_exits': record['production_exits'], 'measurement_sha256': digest(output / 'measurement.json')})
        exposure.remaining_ns('closeout', clock.launcher_begin, closeout_start, time.monotonic_ns())
        return 0
    except BaseException as error:
        # Preserve all partial trees and failures. The outer launcher is the
        # final bounded descendant-tree stop authority after an interrupted
        # phase; an interrupted cleanup never becomes a clean measurement.
        signal.setitimer(signal.ITIMER_REAL, 0)
        write_new(output / 'failure.json', {'state': 'failed', 'error': repr(error),
            'phase': clock.phase, 'phases': clock.records, 'raw': str(raw),
            'fixture_work': str(fixture.work) if fixture else None,
            'cleanup_confirmed': False})
        raise
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, old_handler)


def rows(path):
    with Path(path).open() as stream:
        for line in stream:
            yield json.loads(line)


def measurements(raw, begin, interval):
    """Integer native cohorts; only wholly exposed probes feed the ten gates."""
    from soak_tier import _count_le
    began = begin['began_unix_ns']
    lo, hi = begin['mono_before_ns'], begin['mono_after_ns']
    start, end = hi, lo + 7200 * NS
    require(interval['start_ns'] <= lo and interval['end_ns'] >= hi + 7200 * NS,
            'one or more native exposure nanoseconds missing')
    created, acked, offers = {}, {}, bytearray(100 * 7200)
    ack_windows, rss_windows = [[] for _ in range(9)], [[] for _ in range(9)]
    for item in rows(raw / 'sim/events.jsonl'):
        if item['e'] == 'created':
            identity, sequence = item['id'], item['seq']
            require(type(identity) is int and type(sequence) is int and 0 <= identity < 100
                    and 1 <= sequence <= 7200, 'invalid native worker offer identity')
            slot = identity * 7200 + sequence - 1
            require(not offers[slot], 'duplicate native worker offer')
            offers[slot] = 1
            created[(identity, sequence)] = item['t']
        elif item['e'] == 'attempt' and item['kind'] == 'ack':
            key = item['id'], item['seq']
            require(item['start'] <= item['end'], 'reversed actual attempt latency')
            acked.setdefault(key, item['end'])
            index = exposure.cohort_ns(item['start'], began)
            if index is not None:
                ack_windows[index].append((item['end'] - item['start']) / 1_000_000)
    require(all(offers), 'missing worker offers')
    require(set(acked) <= set(created), 'ACK without original creation')
    made, done = sorted(created.values()), sorted(acked.get(key, 1 << 127) for key in created)
    backlog_seconds = list(range(60, 7195, 60)) + [7195]
    backlog = [_count_le(made, began + second * NS) - _count_le(done, began + second * NS)
               for second in backlog_seconds]
    query_times, query_incomplete, query_failed = [], 0, 0
    pages, answers = list(rows(raw / 'query.jsonl')), list(rows(raw / 'query-http.jsonl'))
    require(len(pages) == len(answers), 'missing raw query answer')
    query_groups = {}
    for page, answer in zip(pages, answers):
        require(answer.get('status') == 200 and 'error' not in page and 'error' not in answer,
                'query refusal or failure')
        require(json.loads(base64.b64decode(answer['raw_base64'], validate=True)) == page['answer'],
                'raw query answer differs from consumed page')
        group = query_groups.setdefault(page['started_ns'], [])
        group.append(page)
    for began_probe, group in query_groups.items():
        finished = group[-1]['ended_ns']
        if start <= began_probe <= finished <= end:
            query_times.append((finished - began_probe) / NS)
            query_incomplete += sum(page['answer'].get('complete') is not True for page in group)
            require(not group[-1]['answer'].get('next_page'), 'incomplete raw query pagination')
    management_ok, management_failed = 0, 0
    for item in rows(raw / 'management.jsonl'):
        require('error' not in item, 'management failure')
        if start <= item['started_ns'] <= item['ended_ns'] <= end and item['method'] == 'PUT':
            management_ok += 1
    snapshots = []
    for sample in rows(raw / 'sampler.jsonl'):
        require('error' not in sample, 'resource/UI sampling failure')
        exposure.validate_parent(sample['groups'])
        snapshots.append(sample['ui'])
        before, after, wall = sample['mono_before_ns'], sample['mono_after_ns'], sample['wall_ns']
        require(before - hi <= wall - began <= after - lo, 'native/wall clock conversion changed')
        if start <= sample['started_ns'] <= sample['ended_ns'] <= end:
            index = exposure.cohort_ns(wall, began)
            if index is not None:
                rss_windows[index].append(sample['rss_kib'])
    snapshots.append(json.loads((raw / 'ui-final.json').read_text()))
    from soak_ui_continuity import validate
    ui = validate(snapshots, lo, hi + 7200 * NS)
    return {'ack_ms_by_window': ack_windows, 'rss_kib_by_window': rss_windows,
        'backlog': backlog, 'query_s': query_times, 'query_incomplete': query_incomplete,
        'query_failed': query_failed, 'management_ok': management_ok, 'management_failed': management_failed}, ui


def grade(value, admission_sha, expected_path, expected_sha, destination):
    """Separate stopped-input grading cell; never rewrites pending receipts."""
    from resource_group import require_limits
    require_limits()
    require(os.environ.get('FABRIC_RESOURCE_RUNTIME_SECONDS') == '1800', 'grading needs default 1800-second cell')
    expected = pinned(expected_path, expected_sha)
    require(expected['binding'] == value['binding'] and expected['admission_sha256'] == admission_sha,
            'owner grading binding changed')
    root, destination = Path(value['output']), Path(destination)
    require(set(path.name for path in root.iterdir()) == {'raw', 'work', 'fixture', 'measurement.json', 'runtime.json'},
            'failed or incomplete measurement root')
    require(not destination.exists() and not destination.is_relative_to(root), 'fresh separate grading output required')
    measurement = pinned(root / 'measurement.json', expected['receipt_sha256']['measurement'])
    runtime = pinned(root / 'runtime.json', expected['receipt_sha256']['runtime'])
    outer = pinned(expected['outer_path'], expected['receipt_sha256']['outer'])
    exposure.validate_checkpoint(
        {'sha256': digest(root / 'measurement.json'), 'value': measurement},
        {'sha256': digest(root / 'runtime.json'), 'value': runtime},
        {'sha256': digest(expected['outer_path']), 'value': outer}, expected)
    from soak_runtime_bindings import validate_job, validate_phases, validate_final_parent
    validate_job(runtime, outer, expected['runtime_expected'])
    validate_phases(runtime, outer['activation_monotonic_ns'])
    validate_final_parent(runtime, outer)
    require(not measurement['smoke'], 'short smoke is not full R3 acceptance')
    for label, members in measurement['members'].items():
        require(inventory(root / label) == members, 'stopped raw input changed')
    for sample in runtime['parent_observations']:
        exposure.validate_parent(sample['groups'])
    require(runtime['measurement_sha256'] == expected['receipt_sha256']['measurement'], 'pending measurement changed')
    # The concrete observer re-parses actual syscall evidence. A runtime's
    # JSON facts or a preexisting control certificate cannot replace this join.
    from simulator_native_trace import validate_stopped
    native = validate_stopped(root / 'raw/native-trace', root / 'raw/begin.json', expected['native'])
    require(native['target_exit'] == native['tracer_exit'] == 0, 'native target/tracer stop missing')
    metrics, ui = measurements(root / 'raw', measurement['native_begin'], measurement['exposure'])
    destination.parent.resolve(strict=True).relative_to(Path(os.environ['FABRIC_STORAGE_ROOT']).resolve(strict=True) / 'results')
    destination.mkdir(mode=0o700)
    # Diagnostic configuration is a separate, recorded relocation of the
    # stopped config; original bytes in the checkpoint remain immutable.
    original = measurement['checkpoint']['work']['source']
    work = root / 'work'
    config = destination / 'server.conf'
    config.write_text((work / 'server.conf').read_text().replace(original, str(work)))
    node_config = destination / 'node.conf'
    node_config.write_text((work / 'state/self-spindle/node.conf').read_text().replace(original, str(work)))
    from soak_companion import dump_stopped_spool, validate_recovery, project
    observation = dump_stopped_spool(work / 'helpers/spool_dump', node_config,
                                    destination / 'custody', processes_stopped=True)
    recovered, transcript = [], destination / 'transcript.jsonl'
    with transcript.open('x') as output:
        for item in rows(root / 'raw/sim/transcript.jsonl'):
            require(item.get('type') not in ('end', 'recovered'), 'unexpected measured transcript terminator/recovery')
            output.write(json.dumps(item) + '\n')
        for item in observation['projected_sources']:
            output.write(json.dumps(item) + '\n')
        command = [str(work / 'helpers/server_dump'), str(config)]
        with (destination / 'dump.jsonl').open('x') as dump, (destination / 'dump.stderr').open('xb') as error:
            child = subprocess.Popen(command, stdout=dump, stderr=error)
            try:
                require(child.wait(timeout=1200) == 0, 'server dump failed')
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=5)
        require((destination / 'dump.stderr').stat().st_size == 0, 'server dump reported uncertainty')
        for item in rows(destination / 'dump.jsonl'):
            item = project(item)
            if (item['node_id'], item['generation']) == tuple(observation['stream']):
                recovered.append(item)
            output.write(json.dumps(item) + '\n')
        output.write('{"type":"end"}\n')
    custody = validate_recovery(observation, recovered)
    import delivery_oracle
    with transcript.open() as source:
        verdict = delivery_oracle.check(source)
    metrics.update(oracle_passed=verdict.passed and custody['passed'],
        sim_exit=runtime['production_exits']['simulator'], server_exit=runtime['production_exits']['server'],
        server_vmhwm_kib=measurement['server_hwm_kib'], sealed_left=len(list((work / 'state/journal').glob('sealed-*.faj'))),
        segments=len(list((work / 'state/segments').glob('seg-*'))))
    from soak_tier import evaluate
    gates = evaluate(metrics)
    # Actual grading launcher stop/receipt is necessarily future evidence.
    # Root can close this result only after independently confirming that stop.
    write_new(destination / 'grading.json', {'state': 'pending_grader_stop', 'gates': gates,
        'all_gates': all(gates.values()), 'binding': value['binding'], 'ui': ui,
        'native': native, 'custody': custody, 'violations': verdict.violations[:5],
        'expected_sha256': expected_sha, 'admission_sha256': admission_sha,
        'input_receipt_sha256': expected['receipt_sha256'], 'metrics': metrics})
    require(all(inventory(root / label) == members for label, members in measurement['members'].items()),
            'raw input changed during grading')
    return 0 if all(gates.values()) else 1


def close_grading(value, admission_sha, expected_path, expected_sha, destination):
    """Close only an already stopped grading cell, preserving every input."""
    from resource_group import require_limits
    require_limits()
    require(os.environ.get('FABRIC_RESOURCE_RUNTIME_SECONDS') == '1800', 'closeout needs default contained cell')
    expected = pinned(expected_path, expected_sha)
    result = pinned(expected['grading_path'], expected['grading_sha256'])
    outer = pinned(expected['outer_path'], expected['outer_sha256'])
    stopped_outer(outer, expected['outer_expected'], 1800)
    require(result.get('state') == 'pending_grader_stop' and result['binding'] == value['binding']
            and result['admission_sha256'] == admission_sha, 'grading result/admission mismatch')
    original = pinned(expected['grading_expected_path'], result['expected_sha256'])
    root = Path(value['output'])
    require(set(path.name for path in root.iterdir()) == {'raw', 'work', 'fixture', 'measurement.json', 'runtime.json'},
            'failed or incomplete measurement root')
    measurement = pinned(root / 'measurement.json', original['receipt_sha256']['measurement'])
    runtime = pinned(root / 'runtime.json', original['receipt_sha256']['runtime'])
    measurement_outer = pinned(original['outer_path'], original['receipt_sha256']['outer'])
    exposure.validate_checkpoint(
        {'sha256': digest(root / 'measurement.json'), 'value': measurement},
        {'sha256': digest(root / 'runtime.json'), 'value': runtime},
        {'sha256': digest(original['outer_path']), 'value': measurement_outer}, original)
    from soak_runtime_bindings import validate_job, validate_phases, validate_final_parent
    validate_job(runtime, measurement_outer, original['runtime_expected'])
    validate_phases(runtime, measurement_outer['activation_monotonic_ns'])
    validate_final_parent(runtime, measurement_outer)
    require(result['input_receipt_sha256'] == original['receipt_sha256'] and not measurement['smoke'],
            'grading input substitution')
    for label, members in measurement['members'].items():
        require(inventory(root / label) == members, 'stopped raw input changed after grading')
    from soak_tier import evaluate
    gates = evaluate(result['metrics'])
    require(gates == result['gates'] and result['all_gates'] is True and all(gates.values()),
            'not all original ten gates succeeded')
    target = Path(destination)
    require(not target.exists() and not target.is_relative_to(root)
            and target.resolve(strict=False) != Path(expected['grading_path']).resolve(strict=True),
            'fresh separate acceptance receipt required')
    target.parent.resolve(strict=True).relative_to(Path(os.environ['FABRIC_STORAGE_ROOT']).resolve(strict=True) / 'results')
    write_new(target, {'state': 'pending_closeout_stop', 'binding': value['binding'],
        'gates': gates, 'admission_sha256': admission_sha, 'closeout_expected_sha256': expected_sha,
        'grading_sha256': expected['grading_sha256'], 'grading_outer_sha256': expected['outer_sha256'],
        'input_receipt_sha256': original['receipt_sha256'],
        'limits': 'one finite same-host loopback trial; no deployment or universal stability qualification'})
    return 0


def main(arguments=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--admission', type=Path, required=True)
    parser.add_argument('--admission-sha256', required=True)
    parser.add_argument('--grade', action='store_true')
    parser.add_argument('--close-grading', action='store_true')
    parser.add_argument('--expected', type=Path)
    parser.add_argument('--expected-sha256')
    parser.add_argument('--grading-output', type=Path)
    args = parser.parse_args(arguments)
    value = admission(args.admission, args.admission_sha256)
    if args.close_grading:
        require(not args.grade and args.expected and args.expected_sha256 and args.grading_output,
                'closeout requires separately pinned stopped grader expectations')
        return close_grading(value, args.admission_sha256, args.expected, args.expected_sha256, args.grading_output)
    if args.grade:
        require(args.expected and args.expected_sha256 and args.grading_output, 'grading requires independently pinned stopped expectations')
        return grade(value, args.admission_sha256, args.expected, args.expected_sha256, args.grading_output)
    return measure(value, args.admission_sha256)


if __name__ == '__main__':
    raise SystemExit(main())
