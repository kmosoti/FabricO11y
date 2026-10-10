import base64
import hashlib
import os
from pathlib import Path
import tempfile
import unittest

from release_timing import clearing, native, population, NS


class TimingControls(unittest.TestCase):
    def fixture(self):
        return [{'created_ns': i * NS + NS // 4, 'ack_ns': i * NS + NS // 2, 'bytes': 700}
                for i in range(135)]

    def test_late_censored_and_invalid_clocks_cannot_pass(self):
        entries = self.fixture()
        self.assertTrue(population(entries, 15 * NS, 135 * NS)['passed'])
        for index in (30, 31):
            entries[index]['ack_ns'] += 2 * NS
        self.assertFalse(population(entries, 15 * NS, 135 * NS)['passed'])
        entries = self.fixture(); entries[31]['ack_ns'] = None
        self.assertEqual(population(entries, 15 * NS, 135 * NS)['censored'], 1)
        self.assertFalse(population(entries, 15 * NS, 135 * NS)['passed'])
        entries = self.fixture(); entries[31]['ack_ns'] = entries[31]['created_ns'] - 1
        self.assertFalse(population(entries, 15 * NS, 135 * NS)['valid'])

    def test_late_sampled_backlog_cannot_hide_behind_fast_acks(self):
        entries = self.fixture()
        for i in range(105, 135, 5):
            entries.append({'created_ns': i * NS - NS // 100, 'ack_ns': i * NS + NS // 100, 'bytes': 1700})
        answer = population(entries, 15 * NS, 135 * NS)
        self.assertLess(answer['creation_to_ack_p99_s'], 1)
        self.assertEqual(answer['final_unacked'], 0)
        self.assertFalse(answer['passed'])
        self.assertGreater(answer['backlog_means']['bytes']['last'], answer['backlog_means']['bytes']['first'])

    def test_native_timing_requires_complete_matching_source_and_attempt(self):
        source = {'type': 'source', 'node_id': '01' * 16, 'generation': 1, 'sequence': 1,
                  'bytes': base64.b64encode(b'independent-source').decode()}
        lines = []
        for stage, stamp in [('sources_accepted', 10), ('spool_committed', 20),
                             ('send_started', 30), ('answer_received', 40)]:
            lines.append(f'timing process_id=7 node_id={source["node_id"]} generation=1 sequence=1 stage={stage} unix_ns=100 boot_monotonic_ns={stamp} monotonic_before_ns=1 monotonic_after_ns=2')
        lines.append('delivery sequence=1 sha256=' + hashlib.sha256(b'independent-source').hexdigest() + ' status=ack committed_through=1 elapsed_us=1')
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT'], prefix='timing-controls-') as root:
            path = Path(root) / 'native.out'
            path.write_text('\n'.join(lines))
            self.assertEqual(native(path, [source])[0]['ack_ns'], 40)
            for mutant in [lines[:-1], [*lines, 'timing dropped_events=1'],
                           [line for line in lines if 'stage=spool_committed ' not in line],
                           [line.replace('boot_monotonic_ns=40', 'boot_monotonic_ns=unmeasured') for line in lines],
                           [line.replace(hashlib.sha256(b'independent-source').hexdigest(), '0' * 64) for line in lines]]:
                path.write_text('\n'.join(mutant))
                with self.assertRaises(ValueError):
                    native(path, [source])

    def test_periodic_phase_changes_only_the_old_sampled_decision(self):
        # Same 10 Hz source and 20 ms service in both windows, with a 50 ms
        # arrival-phase shift. Sampling catches only the final window's work.
        entries = []
        for tick in range(1500):
            created = tick * NS // 10 + (40_000_000 if tick < 1000 else -10_000_000)
            entries.append({'created_ns': created, 'ack_ns': created+20_000_000, 'bytes': 500})
        old = population(entries, 15*NS, 135*NS)
        new = population(entries, 15*NS, 135*NS, rule='clearing-v2')
        self.assertFalse(old['passed'])
        self.assertFalse(new['sampled_v1_passed'])
        self.assertTrue(new['passed'])
        self.assertEqual(new['clearing']['max_busy_ns'], 20_000_000)

    def test_fast_individual_acks_do_not_hide_continuous_debt(self):
        entries = [{'created_ns': i*NS//10, 'ack_ns': i*NS//10+NS//5, 'bytes': 500}
                   for i in range(70)]
        result = population(entries, 0, 10*NS, rule='clearing-v2')
        self.assertLess(result['creation_to_ack_p99_s'], 1)
        self.assertEqual(result['final_unacked'], 0)
        self.assertFalse(result['passed'])
        # Selecting the window first would erase the long warmup/drain chain.
        self.assertGreater(clearing(entries, 3*NS, 4*NS)['max_busy_ns'], 5*NS)
        self.assertFalse(clearing(entries, 3*NS, 4*NS)['passed'])

    def test_adjacency_exact_boundary_zero_duration_and_censoring(self):
        entries = [{'created_ns': i*NS//2, 'ack_ns': (i+1)*NS//2, 'bytes': 50}
                   for i in range(10)]
        boundary = clearing(entries, 0, 10*NS)
        self.assertEqual(boundary['max_busy_ns'], 5*NS)
        self.assertEqual(boundary['windows'][0]['empty_ns'], 0)
        self.assertFalse(boundary['passed'])
        self.assertTrue(clearing([{'created_ns': 2*NS, 'ack_ns': 2*NS, 'bytes': 1}], 0, 10*NS)['passed'])
        for ack in (None, -1, 0.5):
            self.assertFalse(clearing([{'created_ns': 0, 'ack_ns': ack, 'bytes': 1}], 0, NS)['passed'])

    def test_rising_peaks_are_reported_when_work_still_clears(self):
        entries = [{'created_ns': i*NS+NS//4, 'ack_ns': i*NS+NS//2, 'bytes': 500}
                   for i in range(120) for _ in range(1 if i < 60 else 4)]
        result = clearing(entries, 0, 120*NS)
        self.assertTrue(result['passed'])
        self.assertEqual(result['max_outstanding_count'], 4)
        self.assertEqual(result['max_outstanding_bytes'], 2000)
        self.assertGreater(result['continuous_first_last_mean'][1]['bytes'],
                           result['continuous_first_last_mean'][0]['bytes'])


if __name__ == '__main__':
    unittest.main()
