"""Retain the stopped owner's tunnel failure without extending boot budgets."""
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

import remote


class ForwardReadiness(unittest.TestCase):
    def test_mapping_refuses_non_ephemeral_and_untyped_ports(self):
        for port in (True, '40000', 7443, 32767, 61000, -1):
            with self.subTest(port=port), self.assertRaises(ValueError):
                remote.owner_forward_script(port)

    def test_live_tunnel_maps_only_disposable_guest_after_identity_check(self):
        with tempfile.TemporaryDirectory() as directory:
            tunnel = type('Running', (), {'poll': lambda self: None})()
            guest = self.guest(Path(directory), tunnel)
            guest.boot_id = guest.remote_boot_id = 'fixture-boot'
            guest.forward_data_port, guest.forward_listen_port = 7443, 40287
            guest.forward_rule = remote.owner_forward_script(40287)
            guest.host, guest.storage_root, guest.source_root = 'olo@olo-g15', Path('/fixture'), Path('/fixture/source')
            guest.manifest_sha256, guest.command, guest.forward_command = 'fixture-hash', ['worker'], ['ssh']
            result = subprocess.CompletedProcess([], 0, 'fixture rule listing', '')
            with patch.object(remote.Guest, 'ready'), patch.object(guest, 'script', return_value=result) as script, \
                    patch.object(guest, 'correlation', return_value={}):
                guest.ready()
            script.assert_called_once_with(guest.forward_rule)
            self.assertIn('ip daddr 192.0.2.2 tcp dport 7443 dnat to 192.0.2.2:40287', guest.forward_rule)
            self.assertNotIn('flush', guest.forward_rule)
            self.assertTrue((Path(directory) / 'edge-forward-rule.txt').exists())
            guest.remote_boot_id = 'wrong-boot'
            with patch.object(remote.Guest, 'ready'), patch.object(guest, 'script') as script:
                with self.assertRaisesRegex(ValueError, 'boot identity'):
                    guest.ready()
            script.assert_not_called()

    def guest(self, root, tunnel):
        guest = object.__new__(remote.RemoteGuest)
        guest.role, guest.results, guest.ssh = 'edge', root, ['fixture-ssh']
        guest.child = type('Running', (), {'poll': lambda self: None})()
        guest.tunnel = tunnel
        return guest

    def test_exited_tunnel_refuses_before_any_bootstrap_attempt(self):
        with tempfile.TemporaryDirectory() as directory:
            tunnel = type('Exited', (), {'poll': lambda self: 255})()
            guest = self.guest(Path(directory), tunnel)
            refused = subprocess.CompletedProcess([], 255, '', 'Connection refused')
            with patch.object(remote.Q, 'run', return_value=refused) as run, \
                    patch('time.sleep') as sleep:
                with self.assertRaisesRegex(RuntimeError, 'owned data/management forward exited'):
                    guest.ready(deadline=time.monotonic() + 0.01)
            run.assert_not_called()
            sleep.assert_not_called()

    def test_disconnect_is_checked_again_before_the_next_retry(self):
        with tempfile.TemporaryDirectory() as directory:
            tunnel = type('Tunnel', (), {})()
            tunnel.poll = unittest.mock.Mock(side_effect=[None, 255])
            guest = self.guest(Path(directory), tunnel)
            refused = subprocess.CompletedProcess([], 255, '', 'Connection refused')
            with patch.object(remote.Q, 'run', return_value=refused) as run, patch('time.sleep'):
                with self.assertRaisesRegex(RuntimeError, 'owned data/management forward exited'):
                    guest.ready(deadline=time.monotonic() + 0.01)
            self.assertEqual(run.call_count, 1)
            self.assertIn('Connection refused', (Path(directory) / 'edge-bootstrap.txt').read_text())


if __name__ == '__main__':
    unittest.main()
