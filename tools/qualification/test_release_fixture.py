"""Independent decoder/oracle checks for R3 construction and scoped projection."""
import base64
import copy
import unittest

import query_oracle as oracle
from release_fixture import NS, SEEDS, identity, message, query_batch
from release_query import grade, project_records, query_plan, join_verified_recovery, prepare_projections

START = 9_007_199_254_741_123


def record(label, node=0, sequence=1, receive=START):
    return {'label': label, 'received_ns': receive,
            'bytes': base64.b64encode(query_batch(SEEDS[0], node, sequence, START)).decode()}


def page(expected):
    return {key: expected[key] for key in ('rows', 'complete', 'retained_from_ns',
            'retained_to_ns', 'freshness', 'gaps')} | {
                'unavailable': [], 'snapshot': 'independent-test-snapshot', 'next_page': None}


class FixtureTests(unittest.TestCase):
    def test_fixed_plan_counts_bounds_and_reproducibility(self):
        for seed in SEEDS:
            plan = query_plan(seed, START)
            self.assertEqual(plan, query_plan(seed, START))
            self.assertEqual(len(plan), 100)
            self.assertEqual({kind: sum(item['kind']==kind for item in plan)
                              for kind in {item['kind'] for item in plan}},
                             dict.fromkeys(('source_logs','rare_text','source_counter','fleet_counter','trace'),20))
            for item in plan:
                query=item['query']
                self.assertTrue(START <= query['from_ns'] < query['to_ns'] <= START+500*NS)
                self.assertEqual(query['limit'],1000)
                oracle.validate_query(query)

    def test_recovery_join_uses_original_bytes_and_rejects_mutants(self):
        originals=[record('A'),record('B',1)]
        recovered=copy.deepcopy(originals)
        recovered[0]['received_ns']=START+77
        joined,extra=join_verified_recovery(originals,recovered)
        self.assertEqual(joined[0]['bytes'],originals[0]['bytes'])
        self.assertEqual(joined[0]['received_ns'],START+77)
        self.assertFalse(extra)
        mutants=[recovered[:1],recovered+[recovered[0]],copy.deepcopy(recovered),copy.deepcopy(recovered)]
        mutants[2][0]['label']='forbidden'
        raw=base64.b64decode(mutants[3][0]['bytes'])
        mutants[3][0]['bytes']=base64.b64encode(raw+message(8,b'changed source hash')).decode()
        for mutant in mutants:
            with self.assertRaises(ValueError):
                join_verified_recovery(originals,mutant)

    def test_independent_decoder_matches_boundary_coordinates_and_counter_series(self):
        for seed in SEEDS:
            for node in (0, 99):
                for sequence in (1, 2, 100):
                    batch = oracle.decode_batch(query_batch(seed, node, sequence, START))
                    self.assertEqual((batch['version'], batch['generation'], batch['sequence']), (1, 1, sequence))
                    logs = oracle.decode_logs_request(batch['logs_bytes'])
                    metric = oracle.decode_metrics_request(batch['metrics_bytes'])
                    self.assertEqual(len(logs), 50)
                    self.assertEqual(len(metric), 1)
                    self.assertEqual((metric[0]['name'], metric[0]['kind'], metric[0]['monotonic']),
                                     ('release.counter', 'sum', True))
                    self.assertEqual(len(metric[0]['points']), 50)
                    for index, (log, point) in enumerate(zip(logs, metric[0]['points'])):
                        coordinate = (sequence-1)*50+index
                        self.assertEqual(log['observed_time_unix_nano'], START+coordinate*100_000_000)
                        self.assertEqual(len(log['body'].encode('ascii')), 512)
                        if coordinate % 2 == 0:
                            self.assertEqual(log['body'], 'R'*512)
                        self.assertEqual(point['attributes'], {'series': str(coordinate%5)})
                        self.assertEqual(point['value'], coordinate//5+1)
                        self.assertEqual(point['time_ns'], log['observed_time_unix_nano'])
                        self.assertEqual(point['start_ns'], START)
                    spans = oracle.decode_traces_request(batch['traces_bytes'])
                    self.assertEqual(len(spans), 3 if sequence <= 10 else 0)
                    if spans:
                        self.assertEqual(spans[0]['parent_span_id'], '')
                        self.assertTrue(all(span['parent_span_id']==spans[0]['span_id'] for span in spans[1:]))
                        self.assertEqual(len({span['span_id'] for span in spans}), 3)

    def test_identity_and_trace_populations_are_distinct(self):
        for seed in SEEDS:
            self.assertEqual(len({identity(seed, node) for node in range(100)}), 100)
            trace_ids = set()
            for node in range(100):
                for sequence in range(1, 11):
                    batch = oracle.decode_batch(query_batch(seed,node,sequence,START))
                    trace_ids.add(oracle.decode_traces_request(batch['traces_bytes'])[0]['trace_id'])
            self.assertEqual(len(trace_ids),1000)

    def test_projection_preserves_payload_integers_indices_and_scope(self):
        records = [record('A'), record('B',1,receive=START+100)]
        query = {'kind':'logs','node':'A','from_ns':START,'to_ns':START+1,'limit':1000,'page':None}
        projected = project_records(records,query,{'A'})
        self.assertEqual(len(projected),1)
        original = oracle.decode_batch(base64.b64decode(records[0]['bytes']))
        changed = oracle.decode_batch(base64.b64decode(projected[0]['bytes']))
        self.assertEqual(changed['logs_bytes'],original['logs_bytes'])
        self.assertEqual(changed['metrics_bytes'],b'')
        self.assertEqual(changed['traces_bytes'],b'')
        q = {key:value for key,value in query.items() if value is not None}
        expected = oracle.expected(projected,q)
        self.assertEqual(expected['rows'][0]['observed_ns'],START)
        self.assertEqual(expected['rows'][0]['index'],0)
        self.assertEqual(expected['freshness'],{'A':START+4_900_000_000})
        answer = page(expected)
        self.assertTrue(grade(records,query,[answer],{'A'})['passed'])
        self.assertEqual(grade(records,query,[answer],{'A'}),
                         grade(records,query,[answer],{'A'},prepared=prepare_projections(records,{'A'})))
        wrong_value=copy.deepcopy(answer)
        wrong_value['rows'][0]['body']='independently altered response value'
        rejected=grade(records,query,[wrong_value],{'A'})
        self.assertFalse(rejected['passed'])
        self.assertIn('ROW-CONTENT',{item['rule'] for item in rejected['violations']})
        for mutant_freshness in ({'A':START+4_900_000_000,'B':START+100},
                                 {'A':START+99_000_000_000}):
            mutated=copy.deepcopy(answer);mutated['freshness']=mutant_freshness
            verdict=grade(records,query,[mutated],{'A'})
            self.assertFalse(verdict['passed'])
            self.assertIn('FRESHNESS',{item['rule'] for item in verdict['violations']})
        broad = dict(query,to_ns=START+5*NS)
        rows=oracle.expected(project_records(records,broad,{'A'}),{k:v for k,v in broad.items() if v is not None})['rows']
        self.assertEqual([row['index'] for row in rows],list(range(50)))

    def test_span_parent_response_mutation_is_rejected(self):
        records=[record('A')]
        query={'kind':'spans','from_ns':START,'to_ns':START+NS,'limit':1000}
        expected=oracle.expected(project_records(records,query,{'A'}),query)
        answer=page(expected)
        self.assertTrue(grade(records,query,[answer],{'A'})['passed'])
        answer['rows'][1]['parent_span_id']='ffffffffffffffff'
        rejected=grade(records,query,[answer],{'A'})
        self.assertFalse(rejected['passed'])
        self.assertIn('ROW-CONTENT',{item['rule'] for item in rejected['violations']})

    def test_gap_admission_fails_before_query_label_selection(self):
        gap=record('B',1)
        raw=base64.b64decode(gap['bytes'])+message(8,b'independently injected gap')
        gap['bytes']=base64.b64encode(raw).decode()
        query={'kind':'logs','node':'A','from_ns':START,'to_ns':START+NS,'limit':1000}
        with self.assertRaisesRegex(ValueError,'zero-gap'):
            project_records([record('A'),gap],query,{'A','B'})
        verdict=grade([record('A')],query,[{'gaps':[{'gap':'unexpected'}]}],{'A'})
        self.assertFalse(verdict['passed'])
        self.assertEqual(verdict['violations'][0]['rule'],'UNEXPECTED-FIXTURE-GAP')


if __name__ == '__main__':
    unittest.main()
