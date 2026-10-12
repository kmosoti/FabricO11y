"""Pressure verifier controls; never execute a full candidate cell here."""
import base64
import hashlib
import json
import os
from pathlib import Path
import tempfile
import threading
import unittest

from release_stress import first_creation, missing_management_slots, verify_offers
from release_main_source import simulator_batch
from release_timing import NS
from test_console_admission import refused_requests
from workload import SEEDS


class StressControls(unittest.TestCase):
    def population(self):
        node = bytes(range(16))
        events = [{'e': 'created', 'id': 0, 'seq': s, 't': NS*s} for s in (1, 2)]
        ledger = [{'type': 'source', 'node_id': node.hex(), 'generation': 1, 'sequence': s,
                   'bytes': base64.b64encode(hashlib.sha256(simulator_batch(SEEDS[0], 0, node, s, NS*s)).digest()).decode()}
                  for s in (1, 2)]
        return events, ledger

    def test_complete_baseline_and_dropped_offer(self):
        events, ledger = self.population()
        verify_offers(SEEDS[0], 1, 2, (3, 4), events, ledger)
        with self.assertRaisesRegex(ValueError, 'offer population'):
            verify_offers(SEEDS[0], 1, 2, (3, 4), events[:-1], ledger)

    def test_missing_burst_bytes_and_changed_source_rejected(self):
        events, ledger = self.population()
        with self.assertRaisesRegex(ValueError, 'burst workload'):
            verify_offers(SEEDS[0], 1, 2, (1, 2), events, ledger)
        ledger[1]['bytes'] = base64.b64encode(b'x'*32).decode()
        with self.assertRaisesRegex(ValueError, 'burst workload'):
            verify_offers(SEEDS[0], 1, 2, (3, 4), events, ledger)

    def test_drain_iterations_do_not_fill_missing_measured_window(self):
        began = 100*NS
        rows = [{'started_unix_ns': began+offset} for offset in (0, NS//2, NS, 2*NS, 3*NS)]
        self.assertEqual(missing_management_slots(rows, began, 2), [3])

    def test_registered_one_second_coverage_and_missing_round_controls(self):
        began = 100*NS
        for starts, missing in [((.25, 1.25), []), ((.25,), [1]),
                                ((.25, 2.25), [1]), ((.25, .75), [1]),
                                ((-.25, 1.25), [0])]:
            with self.subTest(starts=starts):
                rows = [{'started_unix_ns': began+round(offset*NS), 'complete': True,
                         'inventory_status': 200, 'configuration_status': 200}
                        for offset in starts]
                self.assertEqual(missing_management_slots(rows, began, 2, interval_ns=NS), missing)
        for defect in ({'complete': False}, {'inventory_status': 429},
                       {'configuration_status': 429}, {'exception': 'partial', 'complete': False}):
            rows = [{'started_unix_ns': began+offset, 'complete': True,
                     'inventory_status': 200, 'configuration_status': 200}
                    for offset in (NS//4, NS+NS//4)]
            rows[0].update(defect)
            with self.subTest(defect=defect):
                self.assertEqual(missing_management_slots(rows, began, 2, interval_ns=NS), [0])

    def test_literal_pressure_request_schedule_and_legacy_overrate_control(self):
        one_second = [.10, .25, .25, .75, .75, .75,
                      1.25, 1.25, 1.75, 1.75, 1.75, 2.25, 2.25, 2.75, 2.75, 2.75]
        self.assertEqual(len(one_second), 16)
        self.assertEqual(refused_requests(one_second, 8, 8), 0)
        legacy = [.10] + [stamp for stamp in (.25, .75, 1.25, 1.75, 2.25, 2.75)
                          for _ in range(5)]
        self.assertGreater(refused_requests(legacy, 8, 8), 0)
        first_refusal = next(stamp for count, stamp in enumerate(legacy, 1)
                             if refused_requests(legacy[:count], 8, 8))
        self.assertEqual(first_refusal, 2.25)

    def test_start_uses_original_creation_not_later_generated_clock(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as root:
            path = Path(root)/'events.jsonl'
            events = [{'e': 'created', 'id': i, 'seq': 1, 't': stamp, 'generated_ns': 999*NS}
                      for i, stamp in [(0, 11*NS), (1, 10*NS)]]
            path.write_text(''.join(json.dumps(e)+'\n' for e in events)+'{"partial":')
            self.assertEqual(first_creation(path, threading.Event())['t'], 10*NS)


if __name__ == '__main__':
    unittest.main()
