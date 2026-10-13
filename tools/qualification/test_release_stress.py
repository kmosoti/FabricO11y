"""Pressure verifier controls; never execute a full candidate cell here."""
import base64
import hashlib
import json
import os
from pathlib import Path
import tempfile
import threading
from types import SimpleNamespace
import urllib.error
import unittest
from unittest.mock import ANY, MagicMock, Mock, call, patch

from release_stress import first_creation, missing_management_slots, perform, verify_offers
from release_main_source import simulator_batch
from release_runtime import WorkloadReader
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

    def test_owner_login_precedes_visible_polling_and_simulator_launch(self):
        order = Mock()
        order.login.return_value = {'fresh_until_unix_s': 400}
        order.spawn.side_effect = RuntimeError('stop before simulator launch')
        summary = {}
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as root:
            fixture = SimpleNamespace(
                work=Path(root), helpers=Path(root), origin='https://fixture.invalid',
                enroll=lambda name: {'enrollment_id': name, 'token': 'synthetic-fixture-only'},
                reader=Mock(return_value=object()), spawn=order.spawn,
                bridge=SimpleNamespace(request=Mock(return_value=(200, {})),
                                       login=order.login, poll_ui=order.poll_ui))
            with patch('release_stress.time.sleep'), \
                    patch('release_stress.time.time_ns', side_effect=[100*NS, 101*NS]):
                with self.assertRaisesRegex(RuntimeError, 'stop before simulator launch'):
                    perform(fixture, SimpleNamespace(smoke=True, seed=SEEDS[0]), summary)
        self.assertEqual(summary['owner_login'], {
            'started_unix_ns': 100*NS, 'finished_unix_ns': 101*NS, 'fresh_until_unix_s': 400})
        self.assertEqual(order.mock_calls, [call.login(), call.poll_ui(True), call.spawn(ANY, 'sim')])

    def test_start_uses_original_creation_not_later_generated_clock(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as root:
            path = Path(root)/'events.jsonl'
            events = [{'e': 'created', 'id': i, 'seq': 1, 't': stamp, 'generated_ns': 999*NS}
                      for i, stamp in [(0, 11*NS), (1, 10*NS)]]
            path.write_text(''.join(json.dumps(e)+'\n' for e in events)+'{"partial":')
            self.assertEqual(first_creation(path, threading.Event())['t'], 10*NS)

    def test_page_two_refusal_retains_first_body_and_failed_request_evidence(self):
        """Origin: #62 P2 / C-023; r2 requires every page, including aborted rounds."""
        first = {'complete': True, 'next_page': 'fixture-page-two',
                 'rows': [{'message': 'retained first-page body'}]}
        raw = json.dumps(first).encode()
        response = MagicMock(status=200)
        response.__enter__.return_value = response
        response.read.return_value = raw
        refusal = urllib.error.HTTPError('https://fixture.invalid/v1/console/query',
                                         429, 'synthetic page-two refusal', {}, None)
        reader = WorkloadReader('https://fixture.invalid', None, 'synthetic-reader-secret')
        stop = Mock()
        stop.is_set.side_effect = [False, True]
        stop.wait.return_value = False
        def thread(target, daemon):
            result = Mock()
            if target.__name__ == 'manager':
                result.start.side_effect = target
            result.is_alive.return_value = False
            return result
        summary = {}
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as root:
            fixture = SimpleNamespace(work=Path(root), helpers=Path(root), origin=reader.origin,
                enroll=lambda name: {'enrollment_id': name, 'token': 'synthetic-fixture-only'},
                reader=lambda _: reader, spawn=Mock(return_value=Mock()),
                bridge=SimpleNamespace(request=Mock(return_value=(200, {})),
                    login=Mock(return_value={'fresh_until_unix_s': 400}), poll_ui=Mock()),
                resource_sample=Mock(side_effect=RuntimeError('stop before recovery inspection')))
            with patch('release_stress.time.sleep'), \
                    patch('release_stress.threading.Event', return_value=stop), \
                    patch('release_stress.threading.Thread', side_effect=thread), \
                    patch('release_runtime.urllib.request.urlopen', side_effect=[response, refusal]) as opened:
                with self.assertRaisesRegex(RuntimeError, 'stop before recovery inspection'):
                    perform(fixture, SimpleNamespace(smoke=True, seed=SEEDS[0]), summary)
        self.assertEqual(opened.call_count, 2)
        self.assertEqual(json.loads(opened.call_args_list[1].args[0].data)['page'], 'fixture-page-two')
        self.assertEqual(len(summary['management']), 1)
        row = summary['management'][0]
        observations = row['query_observations']
        self.assertEqual((row.get('query_pages'), len(observations), observations[-1].get('status')),
                         ([first], 2, 429))
        self.assertFalse(row['complete'])
        self.assertIn('HTTPError', row['exception'])
        self.assertTrue(any('management:' in failure and '429' in failure for failure in summary['failures']))
        self.assertEqual(observations[0]['bytes'], len(raw))
        self.assertEqual(observations[-1]['path'], '/v1/console/query')
        self.assertIn('HTTPError', observations[-1]['error'])
        self.assertLessEqual(observations[-1]['started_ns'], observations[-1]['finished_ns'])
        self.assertNotIn(reader.token, json.dumps(summary))

    def test_page_two_invalid_200_retains_status_and_first_body(self):
        """Origin: #62 P2 / C-032; r2 records status even when body integrity fails."""
        first = {'complete': True, 'next_page': 'fixture-page-two', 'rows': []}
        for raw, exception in ((b'{', json.JSONDecodeError),
                               (b'x' * (8 * 1024**2 + 1), RuntimeError)):
            with self.subTest(exception=exception.__name__):
                response = MagicMock(status=200)
                response.__enter__.return_value = response
                response.read.side_effect = [json.dumps(first).encode(), raw]
                reader = WorkloadReader('https://fixture.invalid', None, 'synthetic-reader-secret')
                with patch('release_runtime.urllib.request.urlopen', return_value=response), \
                        self.assertRaises(exception) as caught:
                    reader.pages({'kind': 'logs'})
                self.assertEqual(caught.exception.pages, [first])
                self.assertEqual(len(reader.observations), 2)
                failed = reader.observations[-1]
                self.assertEqual(failed.get('status'), 200)
                self.assertEqual(failed['path'], '/v1/console/query')
                self.assertIn(exception.__name__, failed['error'])
                self.assertLessEqual(failed['started_ns'], failed['finished_ns'])
                self.assertNotIn(reader.token, json.dumps(reader.observations))


if __name__ == '__main__':
    unittest.main()
