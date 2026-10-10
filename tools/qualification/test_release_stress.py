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

    def test_start_uses_original_creation_not_later_generated_clock(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as root:
            path = Path(root)/'events.jsonl'
            events = [{'e': 'created', 'id': i, 'seq': 1, 't': stamp, 'generated_ns': 999*NS}
                      for i, stamp in [(0, 11*NS), (1, 10*NS)]]
            path.write_text(''.join(json.dumps(e)+'\n' for e in events)+'{"partial":')
            self.assertEqual(first_creation(path, threading.Event())['t'], 10*NS)


if __name__ == '__main__':
    unittest.main()
