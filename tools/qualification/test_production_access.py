"""Production adapters reject mixed modes and preserve registered storage."""
import argparse
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from production_access import ProductionAccess, options, validate_options


class ProductionAccessTests(unittest.TestCase):
    def test_partial_or_unselected_receipts_fail_closed(self):
        parser = argparse.ArgumentParser()
        options(parser)
        validate_options(parser.parse_args([]))
        complete = ['--production-access', '--deb-receipt', '/synthetic/deb',
                    '--rpm-receipt', '/synthetic/rpm', '--production-out', '/synthetic/out']
        validate_options(parser.parse_args(complete))
        for args in (complete[1:], ['--production-access'], complete[:-2],
                     ['--deb-receipt', '/synthetic/deb']):
            with self.assertRaises(ValueError):
                validate_options(parser.parse_args(args))

    def test_storage_adapter_restores_original_defaults_and_soak_file_size(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as work:
            config = Path(work) / 'server.conf'
            for soak in (False, True):
                config.write_text('access_origin=https://localhost:1\nself_spindle_ca=fixture-ca\n'
                                  'journal_bytes=4294967296\njournal_file_bytes=16777216\n'
                                  'retention_bytes=100000000000\nretention_s=86400\n')
                adapter = ProductionAccess.__new__(ProductionAccess)
                adapter.config, adapter.soak = config, soak
                with patch('release_runtime.CandidateFixture.start_server') as launch:
                    adapter.start_server()
                    launch.assert_called_once()
                actual = config.read_text()
                self.assertIn('access_origin=https://localhost:1\n', actual)
                self.assertIn('self_spindle_ca=fixture-ca\n', actual)
                self.assertNotIn('retention_', actual)
                if soak:
                    self.assertIn('journal_bytes=4294967296\n', actual)
                    self.assertIn('journal_file_bytes=67108864\n', actual)
                else:
                    self.assertNotIn('journal_', actual)

    def test_server_preserves_original_timing_observation_mode(self):
        adapter = ProductionAccess.__new__(ProductionAccess)
        with patch('release_runtime.CandidateFixture.spawn') as launch:
            adapter.spawn(['fabric-server', 'serve', 'fixture.conf', '--timing-events'], 'server-0', 'fixture-group', '0-1')
            launch.assert_called_once_with(['fabric-server', 'serve', 'fixture.conf'], 'server-0', 'fixture-group', '0-1')

    def test_unmeasured_enrollment_is_paced_without_denial_retries(self):
        from soak_tier import enroll_sources
        events = []
        class Admin:
            def call(self, method, body):
                events.append(('call', body['name']))
                if body['name'] == 'sim0002': raise RuntimeError('synthetic denied enrollment')
                return {'enrollment_id': body['name']}
        with patch('soak_tier.time.sleep', side_effect=lambda duration: events.append(('pace', duration))):
            with self.assertRaises(RuntimeError): enroll_sources(Admin(), 100)
        self.assertEqual(events, [('pace', .15), ('call', 'sim0000'), ('pace', .15), ('call', 'sim0001'),
                                  ('pace', .15), ('call', 'sim0002')])

    def test_browser_temporary_bytes_cannot_escape_combined_five_gib_limit(self):
        adapter = ProductionAccess.__new__(ProductionAccess)
        adapter.raw_root, adapter.work, adapter.out = (Path('/synthetic/raw'), Path('/synthetic/work'), Path('/synthetic/out'))
        class Bridge: browser_temporary = Path('/synthetic/browser-temp')
        adapter.bridge = Bridge()
        sizes = {adapter.raw_root: 3 * 1024**3, adapter.work: 2 * 1024**3 - 1,
                 adapter.out: 0, adapter.bridge.browser_temporary: 2}
        with patch('production_access.directory_bytes', side_effect=lambda path: sizes[path]), \
                patch('production_access.subprocess.check_output', return_value=b'0 synthetic\n'):
            with self.assertRaisesRegex(RuntimeError, '5 GiB'): adapter.sample_bound()
            sizes[adapter.bridge.browser_temporary] = 1
            adapter.sample_bound()

    def test_temporary_observation_error_still_closes_owned_fixture(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as work:
            root = Path(work)
            temporary = root / 'browser-temp'
            temporary.mkdir()
            adapter = ProductionAccess.__new__(ProductionAccess)
            adapter.closed, adapter.receipt, adapter.out = False, {}, root
            class Bridge: browser_temporary = temporary
            adapter.bridge = Bridge()
            def close():
                temporary.rmdir()
                adapter.closed = True
            with patch('production_access.directory_bytes', side_effect=OSError('synthetic counter failure')), \
                    patch('release_runtime.CandidateFixture.close', side_effect=close) as finish:
                with self.assertRaisesRegex(RuntimeError, 'after confirmed cleanup'): adapter.close()
                finish.assert_called_once()
            observation = adapter.receipt['browser_temporary_cleanup']
            self.assertTrue(observation['removed'])
            self.assertEqual(observation['observation_error'], 'OSError')

    def test_failed_ui_prerequisite_cannot_publish_pass_and_still_closes(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as work:
            adapter = ProductionAccess.__new__(ProductionAccess)
            adapter.closed, adapter.receipt, adapter.out = False, {'passed': True}, Path(work)
            class Bridge:
                def poll_health(self, **kwargs):
                    return dict(phases=1, attempts=3, completed=3, statusFailures=1,
                                transportFailures=0, inflight=0)
            adapter.bridge = Bridge()
            def close(): adapter.closed = True
            with patch('release_runtime.CandidateFixture.close', side_effect=close) as finish:
                with self.assertRaisesRegex(RuntimeError, 'after cleanup'): adapter.close()
                finish.assert_called_once()
            self.assertTrue(adapter.closed)
            self.assertFalse(adapter.receipt['passed'])
            self.assertFalse(adapter.receipt['ui_polling_prerequisite']['passed'])

    def test_generated_simulator_credentials_are_removed_but_ledgers_remain(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as work:
            root = Path(work)
            (root / 'sim').mkdir()
            token = root / 'sim/token-0'
            token.write_text('a' * 64 + '\n')
            ledger = root / 'sim/events.jsonl'
            ledger.write_text('synthetic independent oracle ledger\n')
            adapter = ProductionAccess.__new__(ProductionAccess)
            adapter.closed, adapter.receipt, adapter.out, adapter.raw_root = False, {}, root, root
            def close(): adapter.closed = True
            with patch('release_runtime.CandidateFixture.close', side_effect=close): adapter.close()
            self.assertFalse(token.exists())
            self.assertTrue(ledger.exists())
            self.assertEqual(adapter.receipt['simulator_credential_cleanup'][0]['bytes'], 65)

    def test_control_adapter_uses_only_console_nodes_and_rejects_denial(self):
        adapter = ProductionAccess.__new__(ProductionAccess)
        class Bridge:
            def request(self, path, method, body):
                self.observed = path, method, body
                return self.status, {'synthetic': True}
        adapter.bridge = Bridge()
        adapter.bridge.status = 200
        self.assertEqual(adapter.call('PUT', '/source/config', {'logs': []}), {'synthetic': True})
        self.assertEqual(adapter.bridge.observed, ('/v1/console/nodes/source/config', 'PUT', {'logs': []}))
        adapter.bridge.status = 403
        with self.assertRaises(RuntimeError):
            adapter.call('POST', body={'name': 'synthetic'})


if __name__ == '__main__':
    unittest.main()
