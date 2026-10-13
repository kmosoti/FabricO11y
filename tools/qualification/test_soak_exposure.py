"""Prospective R3 literals, registered before the exposure mechanism.

Origin: C-062 / wiki 02f402a. Reference rules are test-only observations of
that registration, not Rust oracles or elapsed two-hour acceptance. Missing
mechanism is a named availability failure; no fallback masquerades as the SUT.
"""
import copy
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import types
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
MECHANISM = HERE / 'soak_exposure.py'
NS = 1_000_000_000
EDGES = (60, 853, 1646, 2440, 3233, 4026, 4820, 5613, 6406, 7200)
CEILINGS = {'setup': 180, 'exposure': 7200, 'drain_seal': 60,
            'shutdown': 20, 'checkpoint': 20, 'closeout': 20}


def reference_cohort(timestamp, began):
    offset = timestamp - began
    return next((i for i, (lo, hi) in enumerate(zip(EDGES, EDGES[1:]))
                 if lo * NS <= offset < hi * NS), None)


def reference_exposure(sim, interval):
    if (sim.get('seconds') != 7200 or sim.get('identities') != 100
            or sim.get('seed') != 0xA11FA001
            or interval.get('sim_began_unix_ns') != sim.get('began_unix_ns')
            or interval.get('end_ns', 0) - interval.get('start_ns', 0) < 7200 * NS
            or tuple(interval.get('cohort_edges_s', ())) != EDGES
            or interval.get('last_offered_second') != 7199
            or interval.get('probes_active') is not True):
        raise ValueError('exposure evidence')
    return True


def reference_remaining(phase, launcher_begin, phase_begin, now):
    if phase not in CEILINGS or not launcher_begin <= phase_begin <= now:
        raise ValueError('phase timestamps')
    remaining = min(CEILINGS[phase] * NS - (now - phase_begin),
                    7500 * NS - (now - launcher_begin))
    if remaining <= 0:
        raise ValueError('phase budget')
    return remaining


def reference_headroom(measured):
    if set(measured) != set(CEILINGS) - {'exposure'}:
        raise ValueError('phase headroom')
    for name, record in measured.items():
        maximum = record.get('maximum_ns')
        if (type(maximum) is not int or maximum <= 0
                or (5 * maximum + 3) // 4 > CEILINGS[name] * NS
                or record.get('kind') not in ('native-r2', 'full-rate-smoke')
                or record.get('stop_confirmed') is not True
                or len(record.get('source_sha256', '')) != 64
                or any(c not in '0123456789abcdef' for c in record['source_sha256'])):
            raise ValueError('phase headroom')
    return True


def reference_checkpoint(measurement, runtime, outer, expected):
    refs = {'measurement': measurement, 'runtime': runtime, 'outer': outer}
    if any(ref['sha256'] != expected['receipt_sha256'][name]
           for name, ref in refs.items()):
        raise ValueError('checkpoint binding')
    m, r, o = (ref['value'] for ref in (measurement, runtime, outer))
    child = o.get('command_receipt', {})
    if (m.get('state') != 'pending_acceptance' or 'passed' in m
            or r.get('state') != 'closed_pending_acceptance' or 'passed' in r
            or m.get('binding') != expected['binding']
            or r.get('binding') != expected['binding']
            or r.get('cleanup_ok') is not True or r.get('immutable_inputs') is not True
            or r.get('production_exits') != {'server': 0, 'simulator': 0}
            or r.get('forced_shutdown') is not False
            or o.get('unit') != expected['unit'] or o.get('stop_confirmed') is not True
            or o.get('command') != expected['command'] or o.get('runtime_max_seconds') != 7500
            or o.get('exit') != 0 or o.get('wrapper_exit') != 0
            or child.get('state') != 'completed' or child.get('exit') != 0
            or child.get('cgroup') != expected['cgroup']):
        raise ValueError('checkpoint binding')
    return True


def reference_parent(groups, stopped=False):
    root = groups.get('.', {})
    limits = {'memory.high': '5368709120', 'memory.max': '6442450944',
              'memory.swap.max': '0', 'pids.max': '1536'}
    if any(root.get(name) != value for name, value in limits.items()):
        raise ValueError('whole-parent containment')
    events = dict(line.split() for line in root.get('memory.events', '').splitlines())
    if (events.get('oom') != '0' or events.get('oom_kill') != '0'
            or root.get('memory.swap.current') != '0'
            or not 0 <= int(root.get('memory.peak', -1)) <= 6442450944
            or not 0 <= int(root.get('pids.peak', -1)) <= 1536
            or 'cgroup.events' not in root or 'cgroup.procs' not in root
            or (stopped and (root['cgroup.events'] != 'populated 0'
                             or root['cgroup.procs'].strip()))):
        raise ValueError('whole-parent containment')
    return True


REFERENCE = types.SimpleNamespace(cohort_ns=reference_cohort,
    validate_exposure=reference_exposure, remaining_ns=reference_remaining,
    validate_phase_headroom=reference_headroom, validate_checkpoint=reference_checkpoint,
    validate_parent=reference_parent)


def checkpoint_records():
    binding = {'candidate': '8fb6f413179b094288316e25845c137440ba23fa',
        'source': '1' * 64, 'deb_receipt': '2' * 64, 'rpm_receipt': '3' * 64,
        'helpers': '4' * 64, 'console': '5' * 64, 'freeze_manifest': '6' * 64,
        'raw': {'events.jsonl': 'a' * 64, 'sim-summary.json': 'b' * 64,
                'server-state': 'c' * 64, 'spool-state': 'd' * 64}}
    expected = {'unit': 'fabric-work-literal', 'cgroup': '/sys/fs/cgroup/fabric-work-literal.service',
        'command': ['runner.py', '--duration-s', '7500', 'soak_tier.py'], 'binding': binding,
        'receipt_sha256': {'measurement': '7' * 64, 'runtime': '8' * 64, 'outer': '9' * 64}}
    measurement = {'sha256': '7' * 64, 'value': {'state': 'pending_acceptance', 'binding': copy.deepcopy(binding)}}
    runtime = {'sha256': '8' * 64, 'value': {'state': 'closed_pending_acceptance',
        'binding': copy.deepcopy(binding), 'cleanup_ok': True, 'immutable_inputs': True,
        'production_exits': {'server': 0, 'simulator': 0}, 'forced_shutdown': False}}
    outer = {'sha256': '9' * 64, 'value': {'unit': expected['unit'], 'stop_confirmed': True,
        'runtime_max_seconds': 7500, 'command': list(expected['command']), 'exit': 0, 'wrapper_exit': 0,
        'command_receipt': {'state': 'completed', 'exit': 0, 'cgroup': expected['cgroup']}}}
    return measurement, runtime, outer, expected


def parent_snapshot():
    return {'.': {'memory.high': '5368709120', 'memory.max': '6442450944',
        'memory.swap.max': '0', 'pids.max': '1536', 'memory.events': 'oom 0\noom_kill 0',
        'memory.peak': '1000', 'memory.current': '0', 'memory.swap.current': '0',
        'pids.current': '0', 'pids.peak': '7', 'cgroup.events': 'populated 0',
        'cgroup.procs': '', 'cpu.max': 'max 100000', 'cpu.stat': 'usage_usec 4',
        'io.stat': ''}}


class LiteralExposureControls(unittest.TestCase):
    subject = REFERENCE

    def test_literal_half_open_edges_plus_minus_one_ns(self):
        began = 1_791_840_000_000_000_000
        for index, second in enumerate(EDGES):
            wanted = (index - 1 if index else None, index if index < 9 else None,
                      index if index < 9 else None)
            for delta, expected in zip((-1, 0, 1), wanted):
                with self.subTest(second=second, delta_ns=delta):
                    self.assertEqual(self.subject.cohort_ns(began + second * NS + delta, began), expected)
        for second, expected in ((59, None), (659, 0), (660, 0), (852, 0), (6406, 8), (7199, 8)):
            self.assertEqual(self.subject.cohort_ns(began + second * NS, began), expected)

    def test_old_profile_mislabeled_and_early_exposure_are_refused(self):
        sim = {'seconds': 7200, 'identities': 100, 'seed': 0xA11FA001, 'began_unix_ns': 1234}
        interval = {'start_ns': 9000, 'end_ns': 7200 * NS + 9000, 'cohort_edges_s': list(EDGES),
                    'sim_began_unix_ns': 1234, 'last_offered_second': 7199, 'probes_active': True}
        self.assertIs(self.subject.validate_exposure(sim, interval), True)
        cases = (({'seconds': 5460}, {}), ({'identities': 99}, {}), ({'seed': 17}, {}),
                 ({}, {'end_ns': interval['end_ns'] - 1}), ({}, {'last_offered_second': 5459}),
                 ({}, {'sim_began_unix_ns': 1235}), ({}, {'probes_active': False}),
                 ({}, {'cohort_edges_s': list(EDGES[:-1])}),
                 ({}, {'cohort_edges_s': [60, 853, 853, 2440, 3233, 4026, 4820, 5613, 6406, 7200]}),
                 ({}, {'cohort_edges_s': [60, 660, 1260, 1860, 2460, 3060, 3660, 4260, 4860, 5460]}))
        for source_change, interval_change in cases:
            with self.subTest(source=source_change, interval=interval_change):
                with self.assertRaisesRegex(ValueError, 'exposure evidence'):
                    self.subject.validate_exposure(sim | source_change, interval | interval_change)

    def test_each_phase_ceiling_and_global_remainder_are_hard(self):
        for phase, seconds in CEILINGS.items():
            with self.subTest(phase=phase):
                self.assertEqual(self.subject.remaining_ns(phase, 0, 0, seconds * NS - 1), 1)
                for overrun in (0, 1):
                    with self.assertRaisesRegex(ValueError, 'phase budget'):
                        self.subject.remaining_ns(phase, 0, 0, seconds * NS + overrun)
                self.assertEqual(self.subject.remaining_ns(phase, 0, 7500 * NS - 3, 7500 * NS - 1), 1)
                with self.assertRaisesRegex(ValueError, 'phase budget'):
                    self.subject.remaining_ns(phase, 0, 7500 * NS - 3, 7500 * NS)
        for args in (('setup', 4, 3, 5), ('setup', 0, 5, 4), ('borrow', 0, 0, 0)):
            with self.assertRaisesRegex(ValueError, 'phase timestamps'):
                self.subject.remaining_ns(*args)

    def test_each_real_phase_requires_25_percent_headroom(self):
        maxima = {'setup': 144, 'drain_seal': 48, 'shutdown': 16, 'checkpoint': 16, 'closeout': 16}
        measured = {name: {'maximum_ns': seconds * NS, 'kind': 'full-rate-smoke',
                           'source_sha256': 'a' * 64, 'stop_confirmed': True}
                    for name, seconds in maxima.items()}
        self.assertIs(self.subject.validate_phase_headroom(measured), True)
        for phase in maxima:
            for change in ({'maximum_ns': measured[phase]['maximum_ns'] + 1},
                           {'kind': 'synthetic'}, {'stop_confirmed': False},
                           {'source_sha256': ''}, {'source_sha256': 'q' * 64}):
                bad = copy.deepcopy(measured); bad[phase].update(change)
                with self.subTest(phase=phase, change=change):
                    with self.assertRaisesRegex(ValueError, 'phase headroom'):
                        self.subject.validate_phase_headroom(bad)
            missing = copy.deepcopy(measured); del missing[phase]
            with self.assertRaisesRegex(ValueError, 'phase headroom'):
                self.subject.validate_phase_headroom(missing)

    def test_pending_runtime_and_exact_stopped_bindings_are_immutable(self):
        records = checkpoint_records(); before = copy.deepcopy(records)
        self.assertIs(self.subject.validate_checkpoint(*records), True)
        self.assertEqual(records, before)
        paths = ((0, ('sha256',), '0' * 64), (1, ('sha256',), '0' * 64), (2, ('sha256',), '0' * 64),
            (0, ('value', 'state'), 'passed'), (0, ('value', 'passed'), True),
            (1, ('value', 'state'), 'passed'), (1, ('value', 'passed'), True),
            (1, ('value', 'cleanup_ok'), False), (1, ('value', 'forced_shutdown'), True),
            (1, ('value', 'immutable_inputs'), False),
            (1, ('value', 'production_exits', 'server'), -15),
            (2, ('value', 'unit'), 'fabric-work-forged'), (2, ('value', 'stop_confirmed'), False),
            (2, ('value', 'runtime_max_seconds'), 7200), (2, ('value', 'exit'), 1),
            (2, ('value', 'wrapper_exit'), 1),
            (2, ('value', 'command'), ['forged']),
            (2, ('value', 'command_receipt', 'state'), 'running'),
            (2, ('value', 'command_receipt', 'exit'), 1),
            (2, ('value', 'command_receipt', 'cgroup'), '/sys/fs/cgroup/forged.service'))
        for index, path, replacement in paths:
            bad = copy.deepcopy(records); cursor = bad[index]
            for part in path[:-1]: cursor = cursor[part]
            cursor[path[-1]] = replacement
            with self.subTest(index=index, path=path):
                with self.assertRaisesRegex(ValueError, 'checkpoint binding'):
                    self.subject.validate_checkpoint(*bad)
        for name in before[3]['binding']:
            for index in (0, 1):
                bad = copy.deepcopy(records); bad[index]['value']['binding'][name] = 'changed'
                with self.subTest(index=index, binding=name):
                    with self.assertRaisesRegex(ValueError, 'checkpoint binding'):
                        self.subject.validate_checkpoint(*bad)

    def test_actual_parent_limits_and_stopped_population_are_required(self):
        groups = parent_snapshot()
        self.assertIs(self.subject.validate_parent(groups, stopped=True), True)
        for key, value in (('memory.high', '17179869184'), ('memory.max', '21474836480'),
            ('memory.swap.max', 'max'), ('pids.max', '1024'), ('memory.swap.current', '1'),
            ('memory.events', 'oom 1\noom_kill 0'), ('memory.events', 'oom 0\noom_kill 1'),
            ('memory.peak', '6442450945'), ('pids.peak', '1537'),
            ('cgroup.events', 'populated 1'), ('cgroup.procs', '123')):
            bad = copy.deepcopy(groups); bad['.'][key] = value
            with self.subTest(key=key, value=value):
                with self.assertRaisesRegex(ValueError, 'whole-parent containment'):
                    self.subject.validate_parent(bad, stopped=True)
        for key in groups['.']:
            if key in ('memory.current', 'pids.current', 'cpu.max', 'cpu.stat', 'io.stat'): continue
            bad = copy.deepcopy(groups); del bad['.'][key]
            with self.subTest(missing=key):
                with self.assertRaisesRegex(ValueError, 'whole-parent containment'):
                    self.subject.validate_parent(bad, stopped=True)
        with self.assertRaisesRegex(ValueError, 'whole-parent containment'):
            self.subject.validate_parent({'server': groups['.']}, stopped=True)


class ExposureMechanismAvailability(unittest.TestCase):
    def test_registered_mechanism_is_available(self):
        self.assertTrue(MECHANISM.is_file(), 'R3 soak_exposure.py mechanism unavailable; not a product failure')


@unittest.skipUnless(MECHANISM.is_file(), 'R3 mechanism absent; availability assertion remains red')
class ExposureMechanismControls(LiteralExposureControls):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location('soak_exposure', MECHANISM)
        cls.subject = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.subject)

    def test_stopped_checkpoint_renames_without_copy_and_refuses_live_or_cross_device(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root = Path(directory); source = root / 'source-parent/raw'; destination = root / 'destination-parent/stopped'
            source.parent.mkdir(); destination.parent.mkdir()
            source.mkdir(); (source / 'one').write_bytes(b'complete stopped bytes')
            identity = (source.stat().st_dev, source.stat().st_ino)
            with mock.patch('shutil.copytree', side_effect=AssertionError('checkpoint copied raw bytes')):
                with self.assertRaisesRegex(ValueError, 'stopped checkpoint'):
                    self.subject.checkpoint_tree(source, destination, processes_stopped=False)
                self.assertTrue(source.is_dir()); self.assertFalse(destination.exists())
                destination.mkdir(); old_inode = destination.stat().st_ino
                with self.assertRaisesRegex(ValueError, 'fresh checkpoint'):
                    self.subject.checkpoint_tree(source, destination, processes_stopped=True)
                self.assertTrue(source.is_dir()); self.assertEqual(destination.stat().st_ino, old_inode)
                destination.rmdir()
                real_stat = Path.stat
                def other_device(path, *args, **kwargs):
                    info = real_stat(path, *args, **kwargs)
                    if path == destination.parent:
                        values = list(info); values[2] += 1
                        return os.stat_result(values)
                    return info
                with mock.patch.object(Path, 'stat', other_device):
                    with self.assertRaisesRegex(ValueError, 'same-filesystem checkpoint'):
                        self.subject.checkpoint_tree(source, destination, processes_stopped=True)
                self.assertTrue(source.is_dir()); self.assertFalse(destination.exists())
                self.subject.checkpoint_tree(source, destination, processes_stopped=True)
            self.assertFalse(source.exists())
            self.assertEqual((destination.stat().st_dev, destination.stat().st_ino), identity)
            self.assertEqual((destination / 'one').read_bytes(), b'complete stopped bytes')


class ExistingSoakSourceControls(unittest.TestCase):
    def test_registered_default_exposure_is_not_the_5460_second_profile(self):
        import soak_tier
        self.assertEqual(soak_tier.SECONDS, 7200, 'existing 5460-second command cannot establish R3 exposure')

    def test_original_ten_gate_names_remain_exact(self):
        from soak_tier import evaluate
        from test_soak_tier import clean
        self.assertEqual(set(evaluate(clean())), {'oracle', 'exits', 'ack_p99_le_1s_every_window',
            'no_growing_backlog', 'server_rss_le_2gib', 'no_rss_growth', 'queries_complete',
            'query_p99_le_2s', 'management_ok', 'sealing_caught_up'})

    def test_kernel_snapshot_includes_actual_parent_and_population(self):
        support = HERE.parent / 'bench/labs/completion/cgroups.py'
        spec = importlib.util.spec_from_file_location('soak_cgroup_controls', support)
        cgroups = importlib.util.module_from_spec(spec); spec.loader.exec_module(cgroups)
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root = Path(directory)
            for group in (root, root / 'server'):
                group.mkdir(exist_ok=True)
                for name, value in parent_snapshot()['.'].items(): (group / name).write_text(value)
            observed = cgroups.snapshot(root)
            self.assertIn('.', observed, 'actual parent omitted from kernel snapshot')
            for name, value in parent_snapshot()['.'].items(): self.assertEqual(observed['.'][name], value)

    def test_same_device_mount_refuses_before_inventory_without_deletion(self):
        import preserve_soak_r2 as preserve
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root = Path(directory); real_read = Path.read_text
            for suffix in ('nested space', 'nested\tname', 'nested\nname', 'nested\\name'):
                nested = root / suffix; nested.mkdir()
                encoded = str(nested).replace('\\', '\\134').replace(' ', '\\040').replace('\t', '\\011').replace('\n', '\\012')
                device = root.stat().st_dev
                mount = f'77 1 {os.major(device)}:{os.minor(device)} / {encoded} rw - ext4 /dev/literal rw\n'
                def read_text(path, *args, **kwargs):
                    return mount if str(path) == '/proc/self/mountinfo' else real_read(path, *args, **kwargs)
                with self.subTest(mount=suffix), mock.patch.object(Path, 'read_text', read_text), \
                        mock.patch.object(preserve.os, 'walk', side_effect=AssertionError('inventory entered mount')) as walk, \
                        mock.patch.object(preserve.shutil, 'rmtree') as delete:
                    with self.assertRaisesRegex(ValueError, 'mount'):
                        preserve.tree_manifest(root)
                    walk.assert_not_called(); delete.assert_not_called()

    def test_new_same_device_mount_refuses_immediately_before_first_unlink(self):
        import preserve_soak_r2 as preserve
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            base = Path(directory); owned = base / 'owned'; results = base / 'results'
            owned.mkdir(); results.mkdir(); (owned / 'raw').write_bytes(b'raw failure retained')
            archive, members, receipt, cleanup = (results / name for name in ('archive', 'members', 'receipt', 'cleanup'))
            for path in (archive, members, receipt): path.write_bytes(b'fixture'); path.chmod(0o600)
            expected = preserve.tree_manifest(owned)
            digest = preserve.hashlib.sha256(json.dumps(expected, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
            archive_sha = preserve.sha256_path(archive)
            members.write_text(json.dumps({'freeze': str(owned), 'archive_sha256': archive_sha,
                'members_sha256': digest, 'members': expected}))
            receipt.write_text(json.dumps({'state': 'preserved_and_verified', 'archive_sha256': archive_sha,
                'members_sha256': digest}))
            mounted = [False]; real_tree = preserve.tree_manifest; real_read = Path.read_text
            device = owned.stat().st_dev
            def inventory(path):
                rows = real_tree(path); mounted[0] = True; return rows
            def read_text(path, *args, **kwargs):
                if str(path) == '/proc/self/mountinfo':
                    return f'77 1 {os.major(device)}:{os.minor(device)} / {owned}/raw rw - ext4 /dev/literal rw\n' if mounted[0] else ''
                return real_read(path, *args, **kwargs)
            globals_ = dict(RESULTS=results, ARCHIVE=archive, MEMBERS=members,
                            PRESERVE_RECEIPT=receipt, CLEANUP_RECEIPT=cleanup)
            stop = (owned, {'invocation_id': 'literal', 'exit_code': 0},
                    {'unit': 'fabric-work-literal', 'exit': 0}, {'manifest_sha256': 'f' * 64})
            with mock.patch.multiple(preserve, **globals_), mock.patch.object(Path, 'read_text', read_text), \
                    mock.patch.object(preserve, 'ownership_checks', return_value=stop), \
                    mock.patch.object(preserve, 'tagged_processes', return_value=[]), \
                    mock.patch.object(preserve, 'verify_archive', return_value={}), \
                    mock.patch.object(preserve, 'tree_manifest', inventory), \
                    mock.patch.object(preserve.shutil, 'rmtree') as delete:
                try:
                    with self.assertRaisesRegex(ValueError, 'mount'):
                        preserve.cleanup(base / 'postcheck', base / 'outer')
                finally:
                    delete.assert_not_called()
            self.assertEqual((owned / 'raw').read_bytes(), b'raw failure retained')


if __name__ == '__main__':
    unittest.main()
