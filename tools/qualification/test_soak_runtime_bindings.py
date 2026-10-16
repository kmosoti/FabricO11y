"""Literal cross-job and phase controls independent of runtime capture."""
import copy
import unittest

from soak_runtime_bindings import validate_job, validate_phases, validate_final_parent

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
    def test_final_parent_must_match_admitted_inode_and_subgroups(self):
        from test_resource_guardian import fixture as guardian_fixture
        runtime, outer, _ = fixture()
        runtime['parent_identity'] = {'device': 7, 'inode': 9}
        runtime['workload_identity'] = {'device': 7, 'inode': 10}
        observation = guardian_fixture()
        observation.update(control_only=False, unit=outer['unit'],
            outer_cgroup=runtime['outer_identity']['cgroup'], activation_monotonic_ns=NS,
            before_ns=runtime['phases'][-1]['end_ns'] + 1, after_ns=runtime['phases'][-1]['end_ns'] + 2)
        observation['outer'].update(device=7, inode=9)
        observation['workload'].update(device=7, inode=10)
        observation['workload_admission'] = {'device': 7, 'inode': 10}
        runtime['workload_cgroup'] = observation['workload_cgroup']
        runtime['wrapper_identity'] = copy.deepcopy(observation['wrapper_identity'])
        runtime['parent_observations'] = [{'groups': copy.deepcopy(observation['outer']['groups'])}]
        observation['command_receipt']['command'] = ['actual-helper']
        outer.update(final_kernel_observation=observation, stop_confirmed=True, exit=0, wrapper_exit=0,
                     command=['actual-helper'], command_receipt=copy.deepcopy(observation['command_receipt']))
        runtime['clock_identity'] = {'clock': 'CLOCK_MONOTONIC', 'boot_id': 'native-boot'}
        outer.update(clock_identity=copy.deepcopy(runtime['clock_identity']),
                     stop_bracket={'before_ns': observation['after_ns'] + 1, 'after_ns': observation['after_ns'] + 2})
        self.assertTrue(validate_final_parent(runtime, outer))
        for mutate in [lambda o: o['outer'].update(inode=10), lambda o: o['outer']['groups'].pop('control-plane'),
                       lambda o: o.update(control_only=True), lambda o: o.update(unit='other'),
                       lambda o: o['wrapper_identity'].update(start_ticks=999)]:
            bad = copy.deepcopy(outer)
            mutate(bad['final_kernel_observation'])
            with self.assertRaises(ValueError):
                validate_final_parent(runtime, bad)
        for mutate in [lambda o: o['clock_identity'].update(boot_id='other-boot'),
                       lambda o: o['stop_bracket'].update(after_ns=runtime['phases'][-1]['start_ns'] + 20 * NS + 1),
                       lambda o: o['stop_bracket'].update(after_ns=o['stop_bracket']['before_ns'] - 1)]:
            bad = copy.deepcopy(outer)
            mutate(bad)
            with self.assertRaises(ValueError):
                validate_final_parent(runtime, bad)

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
