"""Run14's diagnostic inode transition must remain visible and narrowly admitted."""
import base64
import copy
import unittest

import query_oracle
from cross_family.producer import batch, message, metrics
from owner_evidence import (RESTORED_DIAGNOSTIC, RESTORED_GAP, SCOPED_GAP,
                            bounded_edge_records, grade_restored_prefix, restored_gap_witness,
                            without_gaps)
from release_query import project_records


def sources(inode=10, diagnostic=b'before\n'):
    return [dict(path=RESTORED_DIAGNOSTIC, device=1, inode=inode,
                 bytes=base64.b64encode(diagnostic).decode()),
            dict(path='/var/log/app.log', device=1, inode=20,
                 bytes=base64.b64encode(b'original application\n').decode())]


def record(sequence=1, *, gap=True, payload=True):
    raw = batch('a' * 32, sequence, metrics(1, 10, 20) if payload else b'')
    if gap:
        raw += message(8, RESTORED_GAP.encode())
    return dict(label='edge', received_ns=30 + sequence, bytes=base64.b64encode(raw).decode())


def answer(records):
    query = dict(kind='metrics', node='edge', name='cross.counter', from_ns=1, to_ns=100, limit=100)
    expected = query_oracle.expected(project_records([without_gaps(r) for r in records], query, {'edge'}), query)
    expected.pop('unavailable_nonempty')
    expected.update(unavailable=[], snapshot='opaque-native-shape', next_page=None,
                    gaps=[dict(node='edge', sequence=query_oracle.decode_batch(base64.b64decode(r['bytes']))['sequence'],
                               receive_ns=r['received_ns'], gap=SCOPED_GAP)
                          for r in records if query_oracle.decode_batch(base64.b64decode(r['bytes']))['gaps']])
    return query, [expected]


class RestoredGapTests(unittest.TestCase):
    def test_exact_restoration_gap_and_rows_are_both_checked(self):
        records = [record()]
        query, pages = answer(records)
        self.assertTrue(grade_restored_prefix(records, query, pages)['passed'])
        changed = copy.deepcopy(pages)
        changed[0]['rows'][0]['value'] += 1
        self.assertFalse(grade_restored_prefix(records, query, changed)['passed'])

    def test_gap_only_record_does_not_widen_signal_retention(self):
        records = [record(gap=False), record(2, payload=False)]
        query, pages = answer(records)
        self.assertEqual(pages[0]['retained_to_ns'], records[0]['received_ns'])
        self.assertTrue(grade_restored_prefix(records, query, pages)['passed'])

    def test_missing_altered_extra_or_foreign_gap_answers_fail(self):
        records = [record()]
        query, pages = answer(records)
        for key, value in [('gap', 'hidden'), ('node', 'foreign'), ('sequence', 9), ('receive_ns', 99)]:
            changed = copy.deepcopy(pages)
            changed[0]['gaps'][0][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                grade_restored_prefix(records, query, changed)
        for value in ([], pages[0]['gaps'] * 2):
            changed = copy.deepcopy(pages)
            changed[0]['gaps'] = value
            with self.assertRaises(ValueError):
                grade_restored_prefix(records, query, changed)

    def test_native_identity_and_original_bytes_are_required(self):
        before, after = sources(), sources(11, b'before\nafter\n')
        self.assertTrue(restored_gap_witness(before, after)['source_prefix_preserved'])
        for changed in (sources(), sources(11, b'changed\n'), after[:1], after + after[:1]):
            with self.assertRaises(ValueError):
                restored_gap_witness(before, changed)
        changed = copy.deepcopy(after)
        changed[1]['inode'] += 1
        with self.assertRaises(ValueError):
            restored_gap_witness(before, changed)

    def test_zero_gap_stages_unknown_gap_and_duplicate_gap_still_fail(self):
        before = dict(boot_id='literal', node_id='a' * 32, generation=1,
                      acked_through=1, started_ns=1, finished_ns=2)
        proof = dict(before=before, after=dict(before, next_sequence=2, started_ns=5),
                     attempted_ns=3, completed_ns=4)
        with self.assertRaises(ValueError):
            bounded_edge_records([record()], proof)
        witness = (sources(), sources(11, b'before\nafter\n'))
        self.assertEqual(bounded_edge_records([record()], proof, restored_sources=witness)[:2], (1, 1))
        for gap in ('log rotated; old tail unknown: /var/log/app.log', RESTORED_GAP):
            changed = record()
            changed['bytes'] = base64.b64encode(base64.b64decode(changed['bytes']) + message(8, gap.encode())).decode()
            with self.assertRaises(ValueError):
                bounded_edge_records([changed], proof, restored_sources=witness)
