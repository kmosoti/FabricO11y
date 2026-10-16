"""Regress the actual d537 hosted receipt/checker mismatch and unsafe joins."""
import copy
import importlib.util
import json
from pathlib import Path
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('browser_defect', HERE / 'check-browser-defect.py')
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class BrowserDefectTests(unittest.TestCase):
    def setUp(self):
        self.good = json.loads((HERE / 'fixtures/browser-api-cache-d537.json').read_text())

    def test_observed_hosted_defect_receipt(self):
        self.assertTrue(checker.detected(1, self.good))

    def test_cleanup_corruptions_refuse_the_same_named_defect(self):
        changes = [
            lambda r: r.pop('own_children_empty'),
            lambda r: r.update(own_children_empty=False),
            lambda r: r.update(cleanup='owned TLS/server/browser fixtures removed'),
            lambda r: r.update(cleanup_error='unfinished'),
            lambda r: r.update(group_stops=[]),
            lambda r: r['group_stops'][0].update(kernel_empty=False),
            lambda r: r['group_stops'][0].pop('children_reaped'),
            lambda r: r['group_stops'][0].update(children_reaped=1),
            lambda r: r['group_stops'][0].update(stop_error='timeout'),
            lambda r: r['group_stops'][0].update(pid=0),
            lambda r: r['group_stops'][0].update(start_ticks=False),
            lambda r: r['group_stops'].__setitem__(1, r['group_stops'][0]),
            lambda r: r['process_exits'].pop('server'),
            lambda r: r['process_exits'].update(server=None),
        ]
        for index, change in enumerate(changes):
            with self.subTest(index=index):
                receipt = copy.deepcopy(self.good)
                change(receipt)
                self.assertFalse(checker.detected(1, receipt))

    def test_unrelated_failure_or_missing_prerequisite_refuses(self):
        for status in (0, 2, -15):
            self.assertFalse(checker.detected(status, self.good))
        for field, value in [('error','timeout'), ('exit',0), ('injected_api_cache_defect',False), ('checks',[])]:
            with self.subTest(field=field):
                self.assertFalse(checker.detected(1, {**self.good, field:value}))

    def test_nonresident_control_also_requires_structured_cleanup(self):
        receipt = {**self.good, 'injected_nonresident_enrollment':True,
                   'error':'owner enrollment stores actual resident credentials for the fixture RP',
                   'checks':['first owner consumed protected one-time bootstrap',
                             'server enrollment requires resident credentials and user verification'],
                   'resident_credential_witnesses':[{'count':1,'credentials':[{'resident':False,'rp':'localhost'}]}]}
        self.assertTrue(checker.detected(1, receipt, 'nonresident-enrollment'))
        receipt['own_children_empty'] = False
        self.assertFalse(checker.detected(1, receipt, 'nonresident-enrollment'))


if __name__ == '__main__':
    unittest.main()
