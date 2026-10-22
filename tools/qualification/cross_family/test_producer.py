"""Published protobuf semantics checked with unchanged independent oracle."""
import base64
import importlib.util
from pathlib import Path
import sys
import unittest
import http.server
import json
import queue
import tempfile
import threading
import time
import urllib.request
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import query_oracle
import workload
SPEC = importlib.util.spec_from_file_location('cross_producer', HERE / 'producer.py')
P = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(P)


class ProducerTests(unittest.TestCase):
    def test_guard_preserves_every_offered_coordinate_and_full_outage_work(self):
        times = [P.due_ns(0, tick) for tick in range(P.SECONDS * 10)]
        self.assertEqual(len(times), 1950)
        self.assertEqual(times[1349], 134900000000)
        self.assertEqual(times[1350], 140000000000)
        self.assertEqual(times[-1], 199900000000)
        self.assertEqual(sum(tick % 5 == 0 for tick in range(len(times))), 390)
        self.assertEqual(sum(tick % 150 == 0 for tick in range(len(times))), 13)
        self.assertEqual(sum(t >= 140000000000 for t in times), 600)

    def test_original_short_deadline_fails_but_bounded_sender_retains_all_outcomes(self):
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                self.rfile.read(int(self.headers['content-length']))
                time.sleep(.15)
                try:
                    self.send_response(200)
                    self.send_header('Content-Length', '2')
                    self.end_headers()
                    self.wfile.write(b'{}')
                except OSError:
                    pass

        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=lambda: server.serve_forever(poll_interval=.05))
        thread.start()
        try:
            url = f'http://127.0.0.1:{server.server_port}/v1/traces'
            with self.assertRaises(OSError):
                urllib.request.urlopen(urllib.request.Request(url, data=b'original'), timeout=.08)
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / 'transport.jsonl'
                journal = P.Journal(path)
                failures = []
                sender = P.TraceSender(journal, failures, time.monotonic()+5, url)
                self.assertEqual((len(sender.workers), sender.pending.maxsize), (8, 256))
                for tick in range(16):
                    sender.offer(tick, b'original', time.monotonic_ns())
                sender.finish()
                journal.stream.close()
                self.assertFalse(failures)
                self.assertFalse(any(t.is_alive() for t in sender.workers))
                rows = [json.loads(line) for line in path.read_text().splitlines()]
                self.assertEqual({r['tick'] for r in rows}, set(range(16)))
                self.assertEqual(len(rows), 16)
                self.assertTrue(all(r['type'] == 'trace_admitted' and r['status'] == 200 for r in rows))
        finally:
            server.shutdown()
            thread.join(timeout=2)
            server.server_close()
            self.assertFalse(thread.is_alive())

    def test_queue_overflow_and_worker_exception_are_explicit_failures(self):
        with tempfile.TemporaryDirectory() as directory:
            journal = P.Journal(Path(directory) / 'transport.jsonl')
            sender = P.TraceSender.__new__(P.TraceSender)
            sender.pending, sender.journal, sender.failures = queue.Queue(maxsize=1), journal, []
            sender.offer(0, b'x', 1)
            sender.offer(1, b'x', 2)
            self.assertEqual(sender.failures, ['trace transport queue overflow'])
            with patch.object(P.urllib.request, 'urlopen', side_effect=ValueError('injected worker defect')):
                failures = []
                pool = P.TraceSender(journal, failures, time.monotonic()+5)
                pool.offer(2, b'x', 3)
                pool.finish()
                self.assertTrue(any('injected worker defect' in error for error in failures))
                self.assertFalse(any(t.is_alive() for t in pool.workers))
            journal.stream.close()

    def test_exact_32_cumulative_monotonic_points(self):
        metric, = query_oracle.decode_metrics_request(P.metrics(3, 100, 200))
        self.assertEqual(metric['name'], 'cross.counter')
        self.assertEqual(metric['kind'], 'sum')
        self.assertTrue(metric['monotonic'])
        self.assertEqual(len(metric['points']), 32)
        for p, point in enumerate(metric['points']):
            self.assertEqual(point, {'start_ns': 100, 'time_ns': 200,
                                    'attributes': {'point_id': str(p)}, 'value': 3 * (p + 1)})
        fields = query_oracle.parse_fields(P.metrics(3, 100, 200))
        resource = query_oracle.parse_fields(fields[1][0][1])
        scope = query_oracle.parse_fields(resource[2][0][1])
        metric_fields = query_oracle.parse_fields(scope[2][0][1])
        total = query_oracle.parse_fields(metric_fields[7][0][1])
        self.assertEqual(total[2], [(0, 2)])  # OTLP cumulative temporality.

    def test_trace_identity_parentage_and_population(self):
        seen = set()
        for tick in range(20):
            payload, rows = P.trace(tick, 1000)
            decoded = query_oracle.decode_traces_request(payload)
            self.assertEqual(len(decoded), 3)
            self.assertEqual({r['trace_id'] for r in decoded}, {rows[0]['trace_id']})
            self.assertNotIn(rows[0]['trace_id'], seen)
            seen.add(rows[0]['trace_id'])
            for row, expected in zip(decoded, rows):
                for key, value in expected.items():
                    self.assertEqual(row[key], value)

    def test_log_entropy_and_envelope_are_registered(self):
        for tick in (1, 17, 300):
            self.assertEqual(P.entropy_body(tick), workload.entropy_body(P.SEED, 0, tick))
            self.assertEqual(len(P.entropy_body(tick)), 512)
        node = '1234567890abcdef1234567890abcdef'
        raw = P.batch(node, 7, P.metrics(3, 100, 200))
        decoded = query_oracle.decode_batch(raw)
        self.assertEqual(decoded['node_id'], bytes.fromhex(node))
        self.assertEqual(decoded['generation'], 1)
        self.assertEqual(decoded['sequence'], 7)


if __name__ == '__main__':
    unittest.main()
