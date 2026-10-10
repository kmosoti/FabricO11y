import unittest
import timing


class ClockJoins(unittest.TestCase):
    def test_clock_uncertainty_conservatively_expands_duration(self):
        self.assertEqual(timing.elapsed(timing.interval('a', 10, 20), timing.interval('a', 100, 130)),
                         {'lower_ns': 80, 'upper_ns': 120, 'uncertainty_ns': 40})

    def test_cross_guest_wall_samples_cannot_substitute_for_clock_join(self):
        with self.assertRaisesRegex(ValueError, 'different guest boots'):
            timing.elapsed(timing.interval('a', 10, 20), timing.interval('b', 100, 130))

    def test_lost_or_absent_clock_rejects_population(self):
        line = 'timing process_id=12 node_id=' + 'a' * 32 + ' generation=1 sequence=2 stage=answer_received unix_ns=unmeasured boot_monotonic_ns=100 monotonic_before_ns=5 monotonic_after_ns=15'
        event, = timing.events(line, 'boot')
        self.assertEqual((event['lower_ns'], event['upper_ns']), (90, 110))
        self.assertIsNone(event['unix_ns'])
        for defect in (line + '\ntiming dropped_events=1', line.replace('boot_monotonic_ns=100', 'boot_monotonic_ns=unmeasured')):
            with self.assertRaises(ValueError):
                timing.events(defect, 'boot')

    def test_missing_ack_is_censored_failure_instead_of_excluded_sample(self):
        with self.assertRaisesRegex(ValueError, 'required source'):
            timing.acknowledged_samples([], set(), {('a' * 32, 1, 2): timing.interval('boot', 1, 2)})
