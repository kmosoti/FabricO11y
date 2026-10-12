"""Real decoder/oracle controls for independently bounded all-reader snapshots."""
import base64
import copy
import json
from pathlib import Path
import unittest

import query_oracle as oracle
from query_corpus import COMPANION_LABEL, SnapshotCorpus, join_companion
from release_fixture import message
from release_query import grade, prepare_projections, project_records
from test_release_fixture import START, page, record

CASE = json.loads((Path(__file__).resolve().parents[2] /
    'docs/experiments/formal/fixtures/query-reader-corpus.json').read_text())['snapshot_control']


def source(original):
    batch = oracle.decode_batch(base64.b64decode(original['bytes']))
    return {'node_id': batch['node_id'].hex(), 'generation': batch['generation'],
            'sequence': batch['sequence'], 'bytes': original['bytes'], 'type': 'source'}


class CorpusTests(unittest.TestCase):
    def setUp(self):
        self.fixture = [record('A')]
        self.companion = [record(COMPANION_LABEL, 99, sequence=i,
            receive=START+i*100, ) for i in (1, 2, 3)]
        self.sources = list(map(source, self.companion))
        self.timing = [{'key': [s['node_id'], s['generation'], s['sequence']],
            'created_ns': accepted, 'ack_ns': ack} for s, accepted, ack in
            zip(self.sources, CASE['source_accepted_ns'], CASE['source_ack_ns'])]
        self.corpus = SnapshotCorpus(self.fixture, self.companion, self.timing, {'A'}, prepare_projections)
        self.query = {'kind': 'metrics', 'name': 'release.counter',
            'from_ns': START, 'to_ns': START+1, 'limit': 1000}

    def answer(self, prefix, query=None):
        query = self.query if query is None else query
        projected = project_records(self.fixture+self.companion[:prefix], query, {'A', COMPANION_LABEL})
        return [page(oracle.expected(projected, query))]

    def evaluate(self, pages, query=None):
        return self.corpus.grade(self.query if query is None else query, pages,
            CASE['first_page_attempt_ns'], CASE['first_page_completed_ns'], grade)

    def test_fixed_and_concurrent_prefix_are_oracle_exact(self):
        self.assertEqual(self.corpus.bounds(100, 200), tuple((1, 2)))
        for prefix in CASE['admitted_prefixes']:
            result = self.evaluate(self.answer(prefix))
            self.assertTrue(result['passed'])
            self.assertEqual(result['snapshot_prefix']['selected'], prefix)

    def test_omitted_acked_source_and_future_source_are_rejected(self):
        for prefix in (0, 3):
            self.assertFalse(self.evaluate(self.answer(prefix))['passed'])

    def test_forged_freshness_and_retention_are_rejected(self):
        for field in ('freshness', 'retained_from_ns', 'retained_to_ns'):
            changed = copy.deepcopy(self.answer(2))
            if field == 'freshness':
                changed[0][field][COMPANION_LABEL] += 1
            else:
                changed[0][field] += 1
            self.assertFalse(self.evaluate(changed)['passed'])

    def test_one_source_filter_still_excludes_other_sources(self):
        query = dict(self.query, node='A')
        self.assertTrue(self.evaluate(self.answer(2, query), query)['passed'])
        changed = self.answer(2, query)
        changed[0]['freshness'][COMPANION_LABEL] = 123
        self.assertFalse(self.evaluate(changed, query)['passed'])

    def test_receive_join_rejects_changed_bytes_label_and_missing_prefix(self):
        joined = join_companion(self.sources, self.companion)
        self.assertEqual(joined, self.companion)
        changed = copy.deepcopy(self.companion)
        raw = base64.b64decode(changed[0]['bytes']) + message(8, b'changed independent witness')
        changed[0]['bytes'] = base64.b64encode(raw).decode()
        wrong_label = copy.deepcopy(self.companion)
        wrong_label[0]['label'] = 'unknown-source'
        for mutant in (changed, wrong_label, self.companion[1:], self.companion+[self.companion[0]]):
            with self.assertRaises(ValueError):
                join_companion(self.sources, mutant)

    def test_invalid_or_unbounded_snapshot_timing_fails_closed(self):
        with self.assertRaises(ValueError):
            self.corpus.bounds(200, 100)
        changed = copy.deepcopy(self.timing)
        changed[1]['created_ns'] = 1
        with self.assertRaises(ValueError):
            SnapshotCorpus(self.fixture, self.companion, changed, {'A'}, prepare_projections)


if __name__ == '__main__':
    unittest.main()
