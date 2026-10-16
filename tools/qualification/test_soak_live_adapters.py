"""Finite adapter joins; reviewed supervisor native evidence remains separate."""
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

from console_bridge import ConsoleBridge
from soak_r3 import Probes


class LiveAdapterControls(unittest.TestCase):
    def test_shared_fixture_default_and_explicit_browser_deadlines(self):
        from release_runtime import fixture_browser_deadline
        with mock.patch.dict(os.environ, {'FABRIC_RESOURCE_RUNTIME_SECONDS': '1800'}):
            self.assertEqual(fixture_browser_deadline(None, 100), 1900)
            self.assertEqual(fixture_browser_deadline(7500, 100), 7500)
        for runtime in ('0', '16001', 'invalid'):
            with mock.patch.dict(os.environ, {'FABRIC_RESOURCE_RUNTIME_SECONDS': runtime}):
                with self.assertRaises(ValueError):
                    fixture_browser_deadline(None, 100)
        with mock.patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(KeyError):
                fixture_browser_deadline(None, 100)
        for explicit in (100, float('inf'), True):
            with self.assertRaises(ValueError):
                fixture_browser_deadline(explicit, 100)

    def bridge(self, root):
        bridge = ConsoleBridge.__new__(ConsoleBridge)
        bridge.closed, bridge.session, bridge.log = False, None, None
        bridge.browser_group = bridge.browser_temporary = None
        bridge.work, bridge.out = root / 'profile', root / 'receipt'
        bridge.work.mkdir()
        bridge.out.mkdir()
        bridge.receipt = {}
        bridge.shutdown_deadline = 123.5
        bridge.supervisor = mock.Mock(cleanup_confirmed=True, receipt={'state': 'closed'})
        bridge.driver = bridge.supervisor
        return bridge

    def test_supervised_close_uses_exact_phase_deadline_and_no_legacy_pgid(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            bridge = self.bridge(Path(directory))
            with mock.patch('console_bridge.os.killpg', side_effect=AssertionError('legacy PGID cleanup')):
                bridge.close()
            bridge.supervisor.close.assert_called_once_with(deadline=123.5)
            self.assertFalse(bridge.work.exists())
            self.assertEqual(bridge.receipt['supervised_cleanup'], {'state': 'closed'})

    def test_failed_supervision_preserves_profile_and_aborts(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            bridge = self.bridge(Path(directory))
            bridge.supervisor.close.side_effect = RuntimeError('whole campaign stop required')
            with mock.patch('console_bridge.os.killpg', side_effect=AssertionError('legacy PGID cleanup')):
                with self.assertRaisesRegex(RuntimeError, 'whole campaign stop'):
                    bridge.close()
            self.assertTrue(bridge.work.exists())

    def test_supervisor_death_invalidates_live_probe_continuity(self):
        probes = Probes.__new__(Probes)
        probes.errors, probes.threads = [], [mock.Mock(is_alive=lambda: True)]
        probes.fixture = SimpleNamespace(bridge=SimpleNamespace(supervisor=mock.Mock(poll=lambda: 1)))
        with self.assertRaisesRegex(ValueError, 'supervisor died'):
            probes.check()


if __name__ == '__main__':
    unittest.main()
