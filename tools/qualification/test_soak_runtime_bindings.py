"""Literal cross-job and phase controls independent of runtime capture."""
import copy
import unittest

from soak_runtime_bindings import validate_job, validate_phases

NS = 1_000_000_000


def fixture():
    anchor = NS
    phases, cursor = [], anchor
    for name, seconds in [('setup', 100), ('exposure', 7201), ('drain_seal', 10),
                          ('shutdown', 10), ('checkpoint', 10), ('closeout', 10)]:
        phases.append({'phase': name, 'start_ns': cursor, 'end_ns': cursor + seconds * NS,
                       'elapsed_ns': seconds * NS})
        cursor += seconds * NS
    runtime = {'phases': phases, 'outer_identity': {'unit': 'fabric-work-one',
               'cgroup': '/sys/fs/cgroup/fabric-work-one.service', 'activation_monotonic_ns': anchor},
               'output_identity': {'path': '/owned/result-one', 'device': 7, 'inode': 11},
               'profile': {'identities': 100, 'workers': 100, 'seed': 0xA11FA001,
                           'logs_per_second': 2, 'log_bytes': 512, 'metrics_per_15_seconds': 32},
               'input_sha256': {'simulator': '1' * 64, 'deb': '2' * 64, 'rpm': '3' * 64},
               'headroom_binding': {'candidate': 'frozen'}, 'offered_seconds': 7200,
               'native_interval': {'start_ns': phases[1]['start_ns'],
                                   'conservative_end_ns': phases[1]['start_ns'] + 7200 * NS}}
    outer = {'unit': 'fabric-work-one', 'command_receipt': {'cgroup': '/sys/fs/cgroup/fabric-work-one.service'},
             'activation_monotonic_ns': anchor}
    expected = {key: copy.deepcopy(runtime[key]) for key in
                ('output_identity', 'profile', 'input_sha256', 'headroom_binding')}
    return runtime, outer, expected


class BindingControls(unittest.TestCase):
    def test_exact_job_and_literal_six_phases(self):
        runtime, outer, expected = fixture()
        self.assertTrue(validate_job(runtime, outer, expected))
        self.assertTrue(validate_phases(runtime, outer['activation_monotonic_ns']))

    def test_otherwise_valid_receipts_cannot_mix_jobs(self):
        runtime, outer, expected = fixture()
        for key, value in [('unit', 'fabric-work-two'), ('activation_monotonic_ns', 2 * NS)]:
            bad = dict(outer, **{key: value})
            with self.subTest(key=key), self.assertRaises(ValueError):
                validate_job(runtime, bad, expected)
        bad = copy.deepcopy(outer)
        bad['command_receipt']['cgroup'] = '/sys/fs/cgroup/fabric-work-two.service'
        with self.assertRaises(ValueError):
            validate_job(runtime, bad, expected)

    def test_output_helper_and_rate_substitution_refused(self):
        for field, key, value in [('output_identity', 'inode', 12), ('output_identity', 'path', '/owned/other'),
                                  ('profile', 'workers', 99), ('profile', 'log_bytes', 511),
                                  ('input_sha256', 'simulator', '4' * 64)]:
            runtime, outer, expected = fixture()
            runtime[field][key] = value
            with self.subTest(field=field, key=key), self.assertRaises(ValueError):
                validate_job(runtime, outer, expected)

    def test_forged_elapsed_anchor_overlap_and_phase_borrowing_refused(self):
        for mutate in [lambda r: r['phases'][0].update(elapsed_ns=99 * NS),
                       lambda r: r['phases'][0].update(start_ns=NS + 1),
                       lambda r: r['phases'][1].update(start_ns=NS),
                       lambda r: r['phases'][2].update(end_ns=r['phases'][2]['start_ns'] + 61 * NS),
                       lambda r: r['phases'].pop(),
                       lambda r: r['native_interval'].update(conservative_end_ns=r['phases'][1]['end_ns'] + 1)]:
            runtime, outer, expected = fixture()
            mutate(runtime)
            with self.assertRaises(ValueError):
                validate_phases(runtime, outer['activation_monotonic_ns'])


if __name__ == '__main__':
    unittest.main()
