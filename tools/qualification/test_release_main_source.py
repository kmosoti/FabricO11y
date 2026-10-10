"""Independent source witness from dev-seed1-01's pre-send Rust transcript."""
import base64
import copy
import hashlib
import unittest

import query_oracle
from release_main_source import reconstruct, simulator_batch


class SourceWitness(unittest.TestCase):
    # Origin: release-main-dev-seed1-01, untouched original source/created
    # streams. Its revision-1 backlog result remains failed.
    event = {'e': 'created', 'id': 0, 'seq': 1, 't': 1791649283213725192,
             'generated_ns': 1791649283213814563}
    source = {'type': 'source', 'node_id': '932625421806c75c05e6325a9658c238',
              'generation': 1, 'sequence': 1, 'bytes': '5CkHtqTXAxAFzwCl/8pNP0gsoQxDuxkbsoDhVEICa2U='}

    def test_python_encoding_matches_an_independent_original_rust_hash(self):
        records, mapping = reconstruct(0xA11FA001, 1, 1, [self.event], [self.source])
        raw = base64.b64decode(records[0]['bytes'])
        self.assertEqual(hashlib.sha256(raw).digest(), base64.b64decode(self.source['bytes']))
        self.assertEqual(mapping[0], self.source['node_id'])
        model = query_oracle.materialize_record('sim0000', 1, raw)
        self.assertEqual(len(model.log_rows), 2)
        self.assertEqual(len(model.metric_points), 32)
        self.assertEqual({r['time_ns'] for r in model.metric_points}, {self.event['t']})

    def test_missing_duplicate_or_changed_independent_witness_fails(self):
        cases = [([], [self.source]), ([self.event, self.event], [self.source]),
                 ([self.event], [self.source, self.source])]
        event = dict(self.event, t=self.event['t']+1)
        cases.append(([event], [self.source]))
        changed = copy.deepcopy(self.source); changed['bytes'] = base64.b64encode(b'x'*32).decode()
        cases.append(([self.event], [changed]))
        for events, ledger in cases:
            with self.assertRaises(ValueError):
                reconstruct(0xA11FA001, 1, 1, events, ledger)

    def test_no_metrics_are_invented_between_registered_metric_ticks(self):
        raw = simulator_batch(0xA11FA001, 0, bytes.fromhex(self.source['node_id']), 2, self.event['t'])
        model = query_oracle.materialize_record('sim0000', 1, raw)
        self.assertEqual(len(model.log_rows), 2)
        self.assertEqual(model.metric_points, [])


if __name__ == '__main__':
    unittest.main()
