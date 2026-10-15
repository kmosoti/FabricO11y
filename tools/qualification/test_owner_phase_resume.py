"""Prospective accumulated-budget controls; synthetic clocks, no native claim.

Origin: the protocol table defines required observations, not command order.
The original transport wrongly rejected valid query/revoke phase reentry after
upgrade and prevented the guide's actual primary edge from surviving upgrade.
"""
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from owner_commands import Commands, PHASES


class PhaseResumeTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT'])
        self.root = Path(self.temporary.name)
        self.now = 10**9
        self.clock = patch('owner_commands.time.monotonic_ns', side_effect=lambda: self.now)
        self.clock.start()
        self.commands = Commands(self.root / 'commands')

    def tearDown(self):
        self.clock.stop()
        self.temporary.cleanup()

    def test_valid_query_reentry_after_upgrade(self):
        for name in ('boot', 'install', 'tls', 'first-owner', 'edge',
                     'queries-revoke-restart', 'backup-restore-recovery',
                     'upgrade', 'queries-revoke-restart', 'remove-reset-reenroll', 'archive-stop'):
            self.commands.phase(name)
            self.now += 10**9
        completed = self.commands.finish()
        self.assertEqual([item['name'] for item in completed].count('queries-revoke-restart'), 2)
        self.assertEqual({item['name'] for item in completed}, {name for name, _ in PHASES})

    def test_query_budget_accumulates_across_visits(self):
        self.commands.phase('boot')
        self.commands.phase('queries-revoke-restart')
        self.now += 80 * 10**9
        self.commands.phase('upgrade')
        self.now += 10**9
        self.commands.phase('queries-revoke-restart')
        self.assertEqual(self.commands.remaining(), 40)
        self.now += 41 * 10**9
        with self.assertRaises(TimeoutError): self.commands.remaining()

    def test_phase_omission_still_refuses_final_completion(self):
        self.commands.phase('boot')
        with self.assertRaisesRegex(ValueError, 'omitted'): self.commands.finish()

    def test_overall_deadline_cannot_be_evaded_by_phase_switching(self):
        self.commands.phase('boot')
        for name, seconds in PHASES:
            self.commands.phase(name)
            self.now += seconds * 10**9
        self.now += 1
        with self.assertRaises(TimeoutError): self.commands.remaining()


if __name__ == '__main__':
    unittest.main()
