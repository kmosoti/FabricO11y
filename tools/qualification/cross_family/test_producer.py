"""Published protobuf semantics checked with unchanged independent oracle."""
import base64
import importlib.util
from pathlib import Path
import sys
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import query_oracle
import workload
SPEC = importlib.util.spec_from_file_location('cross_producer', HERE / 'producer.py')
P = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(P)


class ProducerTests(unittest.TestCase):
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
