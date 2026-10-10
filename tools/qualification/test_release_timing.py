import base64
import hashlib
import os
from pathlib import Path
import tempfile
import unittest

from release_timing import native, population, NS


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


if __name__ == '__main__':
    unittest.main()
