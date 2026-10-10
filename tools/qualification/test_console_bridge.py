"""Campaign bridge boundary controls; browser/WebAuthn smoke is separate."""
import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch

from console_bridge import ConsoleBridge, protected_secret, validate_route


class BridgeBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=os.environ["FABRIC_SCRATCH_ROOT"])
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def test_production_route_only_and_canonical_encoding(self):
        validate_route("/v1/console/query", "POST")
        validate_route("/v1/console/nodes/source-1/config", "PUT")
        for route in ("/v1/admin/query", "https://localhost/v1/console/query",
                      "/v1/console/../query", "/v1/console/%2e%2e/query",
                      "/v1/console//query", "/v1/console/query?token=synthetic"):
            with self.assertRaises(ValueError):
                validate_route(route, "POST")

    def test_bootstrap_requires_protected_regular_owned_scratch_file(self):
        secret = self.root / "bootstrap"
        secret.write_text("synthetic-once-only-fixture\n")
        secret.chmod(0o600)
        self.assertEqual(protected_secret(secret), "synthetic-once-only-fixture")
        secret.chmod(0o644)
        with self.assertRaises(ValueError):
            protected_secret(secret)
        secret.chmod(0o600)
        link = self.root / "link"
        link.symlink_to(secret)
        with self.assertRaises(ValueError):
            protected_secret(link)
        hard = self.root / "hard"
        os.link(secret, hard)
        with self.assertRaises(ValueError):
            protected_secret(secret)

    def test_oversized_secret_is_not_consumed(self):
        secret = self.root / "oversized"
        secret.write_text("x" * 257)
        secret.chmod(0o600)
        with self.assertRaises(ValueError):
            protected_secret(secret)

    def test_uv_refresh_pauses_existing_poll_before_replacing_session_then_restores_it(self):
        bridge = ConsoleBridge.__new__(ConsoleBridge)
        bridge.lock, bridge.principal_id, bridge.ui_polling = threading.RLock(), 'synthetic-principal', True
        events = []
        def poll(enabled):
            events.append(('poll', enabled))
            bridge.ui_polling = enabled
        def ceremony(action, body):
            events.append(('ceremony', action))
            self.assertFalse(bridge.ui_polling)
            return {'csrf': 'synthetic-new-csrf'}
        with patch.object(bridge, 'poll_ui', side_effect=poll), \
                patch.object(bridge, '_ceremony', side_effect=ceremony), \
                patch.object(bridge, '_js', side_effect=lambda script: events.append(('witness', script))):
            self.assertEqual(bridge.login(), {'csrf': 'synthetic-new-csrf'})
        self.assertEqual(events[:3], [('poll', False), ('ceremony', 'login'), ('poll', True)])
        self.assertTrue(bridge.ui_polling)

    def test_failed_uv_refresh_keeps_poll_paused_and_has_no_retry(self):
        bridge = ConsoleBridge.__new__(ConsoleBridge)
        bridge.lock, bridge.principal_id, bridge.ui_polling = threading.RLock(), 'synthetic-principal', True
        def poll(enabled): bridge.ui_polling = enabled
        with patch.object(bridge, 'poll_ui', side_effect=poll) as pause, \
                patch.object(bridge, '_ceremony', side_effect=RuntimeError('synthetic rejected ceremony')) as ceremony:
            with self.assertRaises(RuntimeError): bridge.login()
            pause.assert_called_once_with(False)
            ceremony.assert_called_once()
        self.assertFalse(bridge.ui_polling)

    def test_owner_success_cannot_substitute_for_actual_ui_progress_or_status(self):
        bridge = ConsoleBridge.__new__(ConsoleBridge)
        bridge.lock, bridge.ui_polling, bridge.ui_samples, bridge.receipt = threading.RLock(), True, [], {}
        baseline = dict(nowMs=0, attempts=2, completed=2)
        good = dict(nowMs=60000, attempts=14, completed=14, statusFailures=0,
                    transportFailures=0, inflight=0, visibility='visible', connected=True, pauseVisible=True)
        for changed in ({'connected': False}, {'statusFailures': 1}, {'transportFailures': 1},
                        {'attempts': 2, 'completed': 2}, {'inflight': 2}, {'visibility': 'hidden'}):
            bridge.ui_previous = baseline
            with patch.object(bridge, '_js', return_value=good | changed):
                with self.assertRaises(RuntimeError): bridge.poll_health()
        bridge.ui_previous = baseline
        with patch.object(bridge, '_js', return_value=good.copy()):
            self.assertEqual(bridge.poll_health()['completed'], 14)

    def test_readiness_requires_new_actual_body_completion_within_registered_deadline(self):
        bridge = ConsoleBridge.__new__(ConsoleBridge)
        bridge.lock, bridge.ui_polling, bridge.receipt = threading.RLock(), True, {}
        bridge.ui_phase_baseline = {'attempts': 7, 'completed': 7}
        sample = {'attempts': 8, 'completed': 8, 'phases': 2}
        with patch.object(bridge, 'poll_health', return_value=sample), \
                patch('console_bridge.time.monotonic', side_effect=[0, 5]):
            self.assertEqual(bridge.await_poll_ready()['completed'], 8)
        witness = bridge.receipt['ui_ready_phases'][0]
        self.assertEqual(witness['baseline_completed'], 7)
        self.assertEqual(witness['ready_completed'], 8)
        self.assertEqual(witness['elapsed_s'], 5)
        for invalid in ({'attempts': 8, 'completed': 7, 'phases': 2},
                        {'attempts': 7, 'completed': 7, 'phases': 2}):
            with patch.object(bridge, 'poll_health', return_value=invalid), \
                    patch('console_bridge.time.monotonic', side_effect=[0, 20]):
                with self.assertRaisesRegex(RuntimeError, 'twenty seconds'): bridge.await_poll_ready()
        with patch.object(bridge, 'poll_health', side_effect=RuntimeError('synthetic failed status')):
            with self.assertRaisesRegex(RuntimeError, 'failed status'): bridge.await_poll_ready()


if __name__ == "__main__":
    unittest.main()
