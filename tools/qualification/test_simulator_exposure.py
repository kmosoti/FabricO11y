"""C-081 prospective literals: receipt semantics, not native timing evidence.

The small hand-written compilation map and two-second/two-identity records
are synthetic controls. No Rust code, timer, subprocess or native helper runs.
"""
import copy
import hashlib
import importlib.util
from pathlib import Path
import types
import unittest

ROOT = Path(__file__).resolve().parents[2]
MECHANISM = Path(__file__).with_name('simulator_exposure.py')
BASE = 'e078ba23b543126011a2136dd1f7e98ca65370eb'
HELPER = 'examples/spindle_sim.rs'
NS = 1_000_000_000


def source_records():
    original = {HELPER: '2b2828fed73cfe654fadc6739e31c51b9004a42a5174f51be2cb08b161385264',
                'Cargo.toml': '2' * 64, 'Cargo.lock': '3' * 64,
                '.cargo/config.toml': '4' * 64, 'src/spindle/runtime.rs': '5' * 64,
                'vendor/pinned/src/lib.rs': '6' * 64}
    canonical = {'candidate': BASE, 'source_commit': '1' * 40,
        'source_manifest_sha256': '7' * 64, 'source_bundle_sha256': '8' * 64,
        'helper_manifest_sha256': '9' * 64,
        'helpers': {'spool_dump': 'a' * 64, 'server_dump': 'b' * 64, 'spindle_sim': 'c' * 64}}
    source = {'base_commit': BASE, 'commit': '2' * 40, 'tree': '3' * 40,
        'manifest_sha256': 'd' * 64, 'bundle_sha256': 'e' * 64,
        'compilation_inputs': original | {HELPER: 'f' * 64}}
    build = {'sha256': 'a' * 64, 'value': {'state': 'built', 'exit': 0,
        'stop_confirmed': True, 'source': copy.deepcopy(source), 'binary_sha256': 'b' * 64,
        'command': ['cargo', 'build', '--offline', '--locked', '--release', '--example', 'spindle_sim'],
        'configuration': {'rustc': '1.99.0', 'target': 'x86_64-unknown-linux-gnu',
                          'profile': 'release', 'features': [], 'rustflags': []}}}
    expected = {'canonical': copy.deepcopy(canonical), 'canonical_inputs': original,
                'source': copy.deepcopy(source), 'build': copy.deepcopy(build)}
    return source, build, canonical, expected


def native_records():
    process = {'pid': 55, 'start_ticks': 123, 'boot_id': 'literal-boot',
               'exe_device': 7, 'exe_inode': 9}
    parameters = {'seconds': 2, 'identities': 2, 'workers': 2, 'seed': 0xA11FA001}
    identity = {'process': process, 'binary_sha256': 'b' * 64,
                'source_commit': '2' * 40, 'build_receipt_sha256': 'a' * 64}
    expected = {'identity': copy.deepcopy(identity), 'parameters': parameters,
                'begin_sha256': 'c' * 64, 'max_clock_bracket_ns': 1_000_000}
    begin = {'sha256': 'c' * 64, 'value': {'process': copy.deepcopy(process),
        'binary_sha256': 'b' * 64, 'parameters': copy.deepcopy(parameters),
        'mono_before_ns': 10 * NS, 'mono_after_ns': 10 * NS + 100,
        'began_unix_ns': 1_791_840_000_000_000_000}}
    summary = {'seconds': 2, 'identities': 2, 'workers': 2, 'seed': 0xA11FA001,
        'began_unix_ns': begin['value']['began_unix_ns'], 'begin_witness_sha256': 'c' * 64,
        'process': copy.deepcopy(process), 'elapsed_ns': 2 * NS,
        'ended_monotonic_ns': 12 * NS + 100, 'undelivered': 0}
    loaded = identity | {'exit': 0, 'stop_confirmed': True,
        'publication': {'atomic': True, 'exclusive': True, 'before_first_offer': True,
                        'regular': True, 'nlink': 1, 'mode': 0o400, 'unchanged_sha256': 'c' * 64}}
    offers = [{'e': 'created', 'id': identity, 'seq': second + 1,
        't': begin['value']['began_unix_ns'] + second * NS,
        'generated_ns': begin['value']['began_unix_ns'] + second * NS}
        for identity in (0, 1) for second in (0, 1)]
    return begin, summary, offers, loaded, expected


def reference_source_build(source, build, canonical, expected):
    if canonical != expected['canonical']:
        raise ValueError('canonical helper authority')
    actual, original = source['compilation_inputs'], expected['canonical_inputs']
    if (set(actual) != set(original) or actual[HELPER] == original[HELPER]
            or any(actual[name] != original[name] for name in original if name != HELPER)):
        raise ValueError('compilation inputs')
    if (source != expected['source'] or source['base_commit'] != BASE
            or build != expected['build'] or build['value']['source'] != source
            or build['value']['state'] != 'built' or type(build['value']['exit']) is not int
            or build['value']['exit'] != 0 or build['value']['stop_confirmed'] is not True):
        raise ValueError('separate simulator source/build')
    return True


def reference_native(begin, summary, offers, loaded, expected):
    publication = loaded['publication']
    if (not isinstance(begin, dict) or begin.get('sha256') != expected['begin_sha256']
            or publication.get('unchanged_sha256') != expected['begin_sha256']
            or any(publication.get(key) is not True for key in
                   ('atomic', 'exclusive', 'before_first_offer', 'regular'))
            or publication.get('nlink') != 1 or publication.get('mode') != 0o400):
        raise ValueError('begin witness')
    b, identity, parameters = begin['value'], expected['identity'], expected['parameters']
    lo, hi = b['mono_before_ns'], b['mono_after_ns']
    if (type(lo) is not int or type(hi) is not int
            or not 0 <= lo <= hi <= lo + expected['max_clock_bracket_ns']
            or type(b['began_unix_ns']) is not int or b['began_unix_ns'] < 0):
        raise ValueError('clock brackets')
    if (any(loaded.get(key) != value for key, value in identity.items())
            or b['process'] != identity['process'] or b['binary_sha256'] != identity['binary_sha256']
            or b['parameters'] != parameters or summary['process'] != identity['process']
            or any(summary.get(key) != value for key, value in parameters.items())
            or summary['began_unix_ns'] != b['began_unix_ns']
            or summary['begin_witness_sha256'] != begin['sha256']):
        raise ValueError('summary/identity binding')
    duration = parameters['seconds'] * NS
    if (type(summary['elapsed_ns']) is not int or summary['elapsed_ns'] < duration
            or type(summary['ended_monotonic_ns']) is not int
            or summary['ended_monotonic_ns'] < hi + duration
            or type(loaded['exit']) is not int or loaded['exit'] != 0
            or loaded['stop_confirmed'] is not True or summary['undelivered'] != 0):
        raise ValueError('native lifetime')
    created = [item for item in offers if item.get('e') == 'created']
    wanted = {(identity, second + 1) for identity in range(parameters['identities'])
              for second in range(parameters['seconds'])}
    if (len(created) != len(wanted) or {(item['id'], item['seq']) for item in created} != wanted
            or any(type(item[key]) is not int for item in created for key in ('id', 'seq', 't', 'generated_ns'))
            or any(not b['began_unix_ns'] <= item['t'] <= item['generated_ns'] for item in created)):
        raise ValueError('worker offers')
    return True


REFERENCE = types.SimpleNamespace(validate_source_build=reference_source_build,
                                  validate_native=reference_native)


class LiteralSimulatorControls(unittest.TestCase):
    subject = REFERENCE

    def test_exact_native_boundary_and_one_ns_early_refusal(self):
        records = native_records()
        self.assertIs(self.subject.validate_native(*records), True)
        for key in ('elapsed_ns', 'ended_monotonic_ns'):
            bad = copy.deepcopy(records); bad[1][key] -= 1
            with self.subTest(boundary=key), self.assertRaisesRegex(ValueError, 'native lifetime'):
                self.subject.validate_native(*bad)

    def test_missing_changed_and_nonexclusive_begin_refusals(self):
        records = native_records()
        variants = [(None, records[3]), (records[0] | {'sha256': 'd' * 64}, records[3])]
        for key, value in (('exclusive', False), ('atomic', False), ('before_first_offer', False),
                           ('regular', False), ('nlink', 2), ('mode', 0o600), ('unchanged_sha256', 'd' * 64)):
            loaded = copy.deepcopy(records[3]); loaded['publication'][key] = value
            variants.append((records[0], loaded))
        for index, (begin, loaded) in enumerate(variants):
            with self.subTest(witness=index), self.assertRaisesRegex(ValueError, 'begin witness'):
                self.subject.validate_native(begin, records[1], records[2], loaded, records[4])

    def test_reversed_uncertain_and_invalid_clock_refusals(self):
        records = native_records(); b = records[0]['value']
        for change in ({'mono_after_ns': b['mono_before_ns'] - 1},
                       {'mono_after_ns': b['mono_before_ns'] + 1_000_001},
                       {'mono_before_ns': -1}, {'mono_after_ns': True}, {'began_unix_ns': -1}):
            bad = copy.deepcopy(records); bad[0]['value'].update(change)
            with self.subTest(clock=change), self.assertRaisesRegex(ValueError, 'clock brackets'):
                self.subject.validate_native(*bad)

    def test_summary_witness_loaded_binary_source_and_build_refusals(self):
        records = native_records()
        changes = ((0, ('value', 'process', 'pid'), 56),
            (0, ('value', 'process', 'exe_inode'), 10), (0, ('value', 'binary_sha256'), 'd' * 64),
            (1, ('seconds',), 1), (1, ('workers',), 1), (1, ('seed',), 17),
            (1, ('began_unix_ns',), records[1]['began_unix_ns'] + 1),
            (1, ('begin_witness_sha256',), 'd' * 64), (1, ('process', 'start_ticks'), 124),
            (3, ('process', 'boot_id'), 'other-boot'), (3, ('process', 'exe_device'), 8),
            (3, ('binary_sha256',), 'd' * 64), (3, ('source_commit',), '4' * 40),
            (3, ('build_receipt_sha256',), 'd' * 64))
        for index, path, value in changes:
            bad = copy.deepcopy(records); target = bad[index]
            for key in path[:-1]: target = target[key]
            target[path[-1]] = value
            with self.subTest(identity=path), self.assertRaisesRegex(ValueError, 'summary/identity binding'):
                self.subject.validate_native(*bad)

    def test_missing_worker_duplicate_and_final_second_offer_refusals(self):
        records = native_records(); offers = records[2]
        variants = (offers[:-1], offers[:2], offers + [offers[0]],
                    [item | {'seq': 3} if index == 3 else item for index, item in enumerate(offers)],
                    [offers[0] | {'t': records[1]['began_unix_ns'] - 1}] + offers[1:])
        for index, observed in enumerate(variants):
            with self.subTest(offers=index), self.assertRaisesRegex(ValueError, 'worker offers'):
                self.subject.validate_native(records[0], records[1], observed, records[3], records[4])
        for change in ({'exit': 1}, {'stop_confirmed': False}):
            with self.assertRaisesRegex(ValueError, 'native lifetime'):
                self.subject.validate_native(*records[:3], records[3] | change, records[4])

    def test_separate_source_build_and_unchanged_canonical_authority(self):
        records = source_records()
        self.assertIs(self.subject.validate_source_build(*records), True)
        for name in ('spool_dump', 'server_dump', 'spindle_sim'):
            bad = copy.deepcopy(records); bad[2]['helpers'][name] = 'f' * 64
            with self.subTest(canonical=name), self.assertRaisesRegex(ValueError, 'canonical helper authority'):
                self.subject.validate_source_build(*bad)
        for index, path, value in ((0, ('commit',), '4' * 40), (0, ('base_commit',), '5' * 40),
            (0, ('manifest_sha256',), 'f' * 64), (0, ('bundle_sha256',), 'f' * 64),
            (1, ('sha256',), 'f' * 64), (1, ('value', 'binary_sha256'), 'f' * 64),
            (1, ('value', 'exit'), 1), (1, ('value', 'stop_confirmed'), False),
            (1, ('value', 'configuration', 'rustc'), 'other-toolchain')):
            bad = copy.deepcopy(records); target = bad[index]
            for key in path[:-1]: target = target[key]
            target[path[-1]] = value
            with self.subTest(source=path), self.assertRaisesRegex(ValueError, 'separate simulator source/build'):
                self.subject.validate_source_build(*bad)

    def test_only_one_changed_rust_compilation_input_is_admitted(self):
        records = source_records()
        for name in records[3]['canonical_inputs']:
            bad = copy.deepcopy(records)
            bad[0]['compilation_inputs'][name] = ('0' * 64 if name != HELPER
                                                else records[3]['canonical_inputs'][HELPER])
            with self.subTest(input=name), self.assertRaisesRegex(ValueError, 'compilation inputs'):
                self.subject.validate_source_build(*bad)
        for added in (True, False):
            bad = copy.deepcopy(records)
            if added: bad[0]['compilation_inputs']['vendor/extra.rs'] = '0' * 64
            else: del bad[0]['compilation_inputs']['Cargo.lock']
            with self.subTest(added=added), self.assertRaisesRegex(ValueError, 'compilation inputs'):
                self.subject.validate_source_build(*bad)


class SimulatorMechanismAvailability(unittest.TestCase):
    def test_separate_simulator_admission_mechanism_is_available(self):
        self.assertTrue(MECHANISM.is_file(), 'separate simulator_exposure.py unavailable; no native defect claimed')


@unittest.skipUnless(MECHANISM.is_file(), 'admission mechanism absent; native runtime remains unrun')
class SimulatorMechanismControls(LiteralSimulatorControls):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location('simulator_exposure', MECHANISM)
        cls.subject = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.subject)


class CanonicalSourceControls(unittest.TestCase):
    def test_canonical_input_guard_remains_original(self):
        expected = {'tools/qualification/cross_family/inputs.py':
            '3dfd336bd132fa7d904aea783c81db94f6832bea304474d86250cb2329dbec44'}
        for path, digest in expected.items():
            with self.subTest(source=path):
                self.assertEqual(hashlib.sha256((ROOT / path).read_bytes()).hexdigest(), digest)


if __name__ == '__main__':
    unittest.main()
