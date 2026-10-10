#!/usr/bin/env python3
"""Independent encoded-producer fixtures and mutation controls for O6 companion."""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import projection_availability_oracle as po
import query_oracle as qo
import test_query_oracle as fixtures


def answer(records, query, availability):
    exp = po.expected(records, query, availability)
    limit = query.get('limit', max(1, len(exp['rows'])))
    chunks = [exp['rows'][i:i + limit] for i in range(0, len(exp['rows']), limit)] or [[]]
    return [{k: copy.deepcopy(exp[k]) for k in ('complete', 'retained_from_ns', 'retained_to_ns', 'freshness', 'gaps')}
            | {'unavailable': [] if exp['complete'] else [{'source': 'fixture'}],
               'snapshot': 'fixed', 'rows': chunk,
               'next_page': f'page-{i + 1}' if i + 1 < len(chunks) else None}
            for i, chunk in enumerate(chunks)]


class ProjectionAvailability(unittest.TestCase):
    def setUp(self):
        self.records = fixtures.build_log_fixture()
        self.query = dict(fixtures.LOG_QUERY)
        self.availability = {'raw_available': True, 'missing_projections': [
            {'projection': 'logs', 'node': 'nodeA', 'sequence': 1}]}
        self.pages = answer(self.records, self.query, self.availability)

    def rules(self, pages, availability=None):
        verdict = po.check(self.records, self.query, pages, availability or self.availability)
        return verdict, {v['rule'] for v in verdict['violations']}

    def test_partial_projection_preserves_global_metadata(self):
        verdict, _ = self.rules(self.pages)
        self.assertTrue(verdict['passed'])
        self.assertEqual(verdict['expected_rows'], 2)
        self.assertEqual(self.pages[0]['retained_from_ns'], 1000)
        self.assertEqual(self.pages[0]['retained_to_ns'], 1500)
        self.assertEqual(self.pages[0]['freshness'], {'nodeA': 300, 'nodeB': 400})
        self.assertEqual(self.pages[0]['gaps'], [{'node': 'nodeA', 'sequence': 1, 'receive_ns': 1000, 'gap': 'gap-a1'}])
        # Whole-record model is intentionally unchanged and cannot grade this case.
        self.assertFalse(qo.check(self.records, self.query, self.pages,
                                 [{'node': 'nodeA', 'sequence': 1}])['passed'])

    def test_raw_loss_keeps_all_projection_rows_and_metadata(self):
        availability = {'raw_available': False, 'missing_projections': []}
        pages = answer(self.records, self.query, availability)
        self.assertFalse(pages[0]['complete'])
        self.assertEqual(sum(len(p['rows']) for p in pages), 5)
        self.assertTrue(po.check(self.records, self.query, pages, availability)['passed'])
        pages[0]['complete'] = True
        self.assertIn('COMPLETE', self.rules(pages, availability)[1])

    def test_raw_loss_empty_and_nonmatching_are_explicitly_out_of_scope(self):
        raw_loss = {'raw_available': False, 'missing_projections': []}
        for records, query in [(self.records, dict(self.query, contains='absent')),
                               ([], self.query), (self.records, dict(self.query, node='absent'))]:
            verdict = po.check(records, query, self.pages, raw_loss)
            self.assertEqual(verdict['violations'][0]['rule'], 'MALFORMED')

    def test_other_projection_does_not_remove_log_rows(self):
        availability = {'raw_available': True, 'missing_projections': [
            {'projection': 'metrics', 'from_ns': 0, 'to_ns': 2000}]}
        pages = answer(self.records, self.query, availability)
        self.assertTrue(qo.check(self.records, self.query, pages)['passed'])
        self.assertTrue(po.check(self.records, self.query, pages, availability)['passed'])

    def test_metric_rate_and_span_projection_availability(self):
        for records, query, projection in [
            (fixtures.build_rate_fixture(), dict(fixtures.RATE_QUERY, kind='metrics', limit=2), 'metrics'),
            (fixtures.build_rate_fixture(), fixtures.RATE_QUERY, 'metrics'),
            (fixtures.build_span_fixture(), fixtures.SPAN_QUERY, 'spans'),
        ]:
            with self.subTest(kind=query['kind']):
                availability = {'raw_available': True, 'missing_projections': [
                    {'projection': projection, 'from_ns': 0, 'to_ns': 10000}]}
                pages = answer(records, query, availability)
                self.assertEqual(pages[0]['rows'], [])
                self.assertFalse(pages[0]['complete'])
                self.assertEqual(pages[0]['freshness'], qo.expected(records, query)['freshness'])
                self.assertTrue(po.check(records, query, pages, availability)['passed'])
                raw_loss = {'raw_available': False, 'missing_projections': []}
                pages = answer(records, query, raw_loss)
                self.assertGreater(sum(len(p['rows']) for p in pages), 0)
                self.assertFalse(pages[0]['complete'])
                self.assertTrue(po.check(records, query, pages, raw_loss)['passed'])

    def test_envelope_and_row_mutants_rejected(self):
        mutants = [('COMPLETE', lambda p: p[0].update(complete=True)),
                   ('RETAINED-WINDOW', lambda p: p[0].update(retained_from_ns=0, retained_to_ns=0)),
                   ('FRESHNESS', lambda p: p[0].update(freshness={})),
                   ('FRESHNESS', lambda p: p[0].update(freshness={'nodeA': 999999, 'nodeB': 400})),
                   ('GAPS', lambda p: p[0].update(gaps=[])),
                   ('ROW-DROPPED', lambda p: p[0]['rows'].pop()),
                   ('ROW-DUPLICATED', lambda p: p[0]['rows'].append(copy.deepcopy(p[0]['rows'][0]))),
                   ('UNAVAILABLE-SHAPE', lambda p: p[0].update(unavailable=[]))]
        for rule, mutate in mutants:
            with self.subTest(rule=rule):
                pages = copy.deepcopy(self.pages)
                mutate(pages)
                self.assertIn(rule, self.rules(pages)[1])

    def test_full_chain_controls(self):
        availability = {'raw_available': True, 'missing_projections': []}
        pages = answer(self.records, self.query, availability)
        self.assertTrue(po.check(self.records, self.query, pages, availability)['passed'])
        for rule, mutate in [
            ('PAGE-SNAPSHOT', lambda p: p[1].update(snapshot='different')),
            ('PAGE-LIMIT', lambda p: p[0].update(rows=p[0]['rows'] + p[1]['rows'])),
            ('ROW-ORDER', lambda p: p[0]['rows'].reverse()),
            ('ENVELOPE-CONSISTENT', lambda p: p[1].update(freshness={})),
        ]:
            changed = copy.deepcopy(pages)
            mutate(changed)
            self.assertIn(rule, {v['rule'] for v in po.check(self.records, self.query, changed, availability)['violations']})

    def test_malformed_inputs_rejected(self):
        invalid = [None, [], {}, {'raw_available': 1, 'missing_projections': []},
                   {'raw_available': True, 'missing_projections': {}, 'extra': 1},
                   {'raw_available': True, 'missing_projections': [{'projection': 'gaps', 'node': 'nodeA', 'sequence': 1}]},
                   {'raw_available': True, 'missing_projections': [{'projection': 'logs', 'node': 'nodeA', 'sequence': True}]},
                   {'raw_available': True, 'missing_projections': [{'projection': 'logs', 'from_ns': 3, 'to_ns': 2}]},
                   {'raw_available': True, 'missing_projections': self.availability['missing_projections'] * 2}]
        for availability in invalid:
            verdict = po.check(self.records, self.query, self.pages, availability)
            self.assertEqual(verdict['violations'][0]['rule'], 'MALFORMED')
        self.assertEqual(po.check(self.records * 2, self.query, self.pages, self.availability)['violations'][0]['rule'], 'MALFORMED')
        self.assertEqual(po.check(self.records, self.query, [], self.availability)['violations'][0]['rule'], 'MALFORMED')

    def test_cli_exit_codes_and_strict_json(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as temporary:
            root = Path(temporary)
            (root / 'records').write_text('\n'.join(json.dumps(r) for r in self.records))
            for name, value in [('query', self.query), ('answer', self.pages), ('availability', self.availability)]:
                (root / name).write_text(json.dumps(value))
            command = [sys.executable, '-B', str(Path(po.__file__)), '--records', str(root / 'records'),
                       '--query', str(root / 'query'), '--answer', str(root / 'answer'),
                       '--availability', str(root / 'availability')]
            self.assertEqual(subprocess.run(command, capture_output=True).returncode, 0)
            changed = copy.deepcopy(self.pages)
            changed[0]['complete'] = True
            (root / 'answer').write_text(json.dumps(changed))
            self.assertEqual(subprocess.run(command, capture_output=True).returncode, 1)
            (root / 'availability').write_text('{"raw_available":true,"raw_available":false,"missing_projections":[]}')
            self.assertEqual(subprocess.run(command, capture_output=True).returncode, 2)


if __name__ == '__main__':
    unittest.main()
