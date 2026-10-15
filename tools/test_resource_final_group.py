"""R3 final-parent finite controls; native source stays independently observed."""
import copy
import unittest

from resource_final_group import options, validate_empty


def groups():
    return {'.': {'memory.high': '5368709120', 'memory.max': '6442450944', 'memory.swap.max': '0',
        'pids.max': '1536', 'memory.events': 'oom 0\noom_kill 0', 'memory.peak': '1000',
        'memory.current': '500', 'memory.swap.current': '0', 'pids.current': '0', 'pids.peak': '100',
        'cgroup.events': 'populated 0\nfrozen 0', 'cgroup.procs': '', 'cpu.max': 'max 100000',
        'cpu.stat': 'usage_usec 100', 'io.stat': ''}}


class FinalGroupControls(unittest.TestCase):
    def test_optional_mode_does_not_change_default_argument_parser(self):
        self.assertEqual(options(['--', 'true']), (False, ['--', 'true']))
        self.assertEqual(options(['--observe-final-group', '--delegate', '--runtime-seconds', '7500', '--', 'true']),
                         (True, ['--delegate', '--runtime-seconds', '7500', '--', 'true']))

    def test_complete_empty_parent_and_subgroup(self):
        observed = groups()
        observed['supervisor'] = dict(observed['.'])
        self.assertTrue(validate_empty(observed))

    def test_literal_missing_field_population_caps_swap_and_oom(self):
        for name, value in [('memory.high', '17179869184'), ('memory.max', '21474836480'),
                            ('memory.swap.max', 'max'), ('pids.max', '4096'),
                            ('memory.swap.current', '1'), ('memory.events', 'oom 0\noom_kill 1'),
                            ('cgroup.events', 'populated 1'), ('cgroup.procs', '123'),
                            ('pids.current', '1'), ('memory.peak', '6442450945')]:
            bad = groups()
            bad['.'][name] = value
            with self.subTest(name=name), self.assertRaises(ValueError):
                validate_empty(bad)
        for name in groups()['.']:
            bad = groups()
            del bad['.'][name]
            with self.subTest(missing=name), self.assertRaises(ValueError):
                validate_empty(bad)
        with self.assertRaises(ValueError):
            validate_empty({})

    def test_empty_parent_cannot_hide_populated_or_incomplete_subgroup(self):
        for child in [{'cgroup.events': 'populated 0'}, dict(groups()['.'], **{'pids.current': '1'})]:
            bad = groups()
            bad['supervisor'] = child
            with self.assertRaises(ValueError):
                validate_empty(bad)


if __name__ == '__main__':
    unittest.main()
