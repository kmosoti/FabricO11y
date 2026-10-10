import copy
import unittest

from release_main import grade_visibility
from release_fixture import traces, NS


class VisibilityControls(unittest.TestCase):
    def test_original_parent_is_required_even_when_trace_id_and_time_match(self):
        _, rows = traces(0xA11FA001, 0, 10 * NS)
        offers = {'logs': [], 'traces': [{'offered_mono_ns': NS, 'rows': rows}]}
        query = {'second': 20, 'kind': 'spans', 'node': 'edge', 'received_mono_ns': NS + 10,
                 'answer': {'rows': [dict(rows[0])]}}
        answer = grade_visibility([query], offers, 0xA11FA001, 10, {}, 15, '/fixture.log')
        self.assertEqual(answer['edge_traces']['samples'], 1)
        self.assertFalse(answer['edge_traces']['passed'])  # one sample is insufficient
        corrupted = copy.deepcopy(query)
        corrupted['answer']['rows'][0]['parent_span_id'] = 'ff' * 8
        with self.assertRaisesRegex(ValueError, 'independent offer'):
            grade_visibility([corrupted], offers, 0xA11FA001, 10, {}, 15, '/fixture.log')

    def test_missing_probe_and_changed_source_body_are_not_success(self):
        offers = {'logs': [{'offset': 0, 'body': 'R' * 512, 'offered_mono_ns': NS}], 'traces': []}
        query = {'second': 20, 'kind': 'logs', 'node': 'edge', 'received_mono_ns': NS + 10,
                 'answer': {'rows': []}}
        answer = grade_visibility([query], offers, 0xA11FA001, 10, {}, 15, '/fixture.log')
        self.assertEqual(answer['edge_logs']['missing_probes'], 1)
        self.assertFalse(answer['edge_logs']['passed'])
        query['answer']['rows'] = [{'body': 'X' * 512, 'attributes': {'log.file.path': '/fixture.log',
                                                                   'log.file.offset.start': '0'}}]
        with self.assertRaisesRegex(ValueError, 'independent offer'):
            grade_visibility([query], offers, 0xA11FA001, 10, {}, 15, '/fixture.log')


if __name__ == '__main__':
    unittest.main()
