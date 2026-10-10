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
