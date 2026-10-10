import base64
import copy
import unittest
import grade
import producer
import query_oracle


class OracleControls(unittest.TestCase):
    def source(self):
        identity = {'node_id': 'a' * 32, 'generation': 1, 'sequence': 1}
        body = base64.b64encode(producer.batch(identity['node_id'], 1, producer.metrics(1, 10, 20))).decode()
        return {'type': 'source', **identity, 'bytes': body}

    def test_independent_custody_negative_requires_named_durable_rule(self):
        source = self.source()
        identity = {k: source[k] for k in ('node_id', 'generation', 'sequence')}
        transcript = [source, {'type': 'attempt', **identity, 'bytes': source['bytes'], 'injected_conflict': False},
                      {'type': 'response', **identity, 'kind': 'ack', 'committed_through': 1},
                      {'type': 'node_state', 'node_id': identity['node_id'], 'generation': 1,
                       'retained_sequences': [1], 'ack_cursor': 1}]
        recovered = [{**source, 'type': 'recovered'}]
        verdict = grade.custody_grade(transcript, recovered)
        self.assertTrue(verdict['passed'], verdict)
        self.assertTrue(verdict['drop_recovered_negative_detected'])
        self.assertFalse(grade.custody_grade(transcript, [])['passed'])

    def test_wrong_metric_value_is_content_defect_instead_of_shape_defect(self):
        source = self.source()
        records = [{'label': 'controlled', 'received_ns': 30, 'bytes': source['bytes']}]
        query = {'kind': 'metrics', 'node': 'controlled', 'name': 'cross.counter', 'from_ns': 0, 'to_ns': 100, 'limit': 100}
        expected = query_oracle.expected(records, query)
        page = {k: v for k, v in expected.items() if k != 'unavailable_nonempty'}
        page.update(snapshot='g1-1', next_page=None, unavailable=[])
        verdict = grade.query_grade(records, [(query, [page])])
        self.assertTrue(verdict['passed'], verdict)
        self.assertTrue(verdict['mismatched_row_negative_detected'])
        changed = copy.deepcopy(page)
        changed['rows'][0]['value'] += 1
        self.assertFalse(grade.query_grade(records, [(query, [changed])])['passed'])
