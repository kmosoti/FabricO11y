"""Origin: exact-package validation alert lacked live metadata and real speech."""
import copy
import inspect
import json
from pathlib import Path
import unittest
import live_browser_check

FIXTURE=json.loads((Path(__file__).resolve().parents[2]/
    'docs/experiments/formal/fixtures/console-alert-presentation.json').read_text())
VALID=dict(role='alert',live='assertive',atomic='true',text=FIXTURE['text'])

def witness(snapshot):
    try:from browser_alert import assertive_filter_error
    except ImportError:
        assert 'actual invalid filter error and request status have announcement roles' in inspect.getsource(live_browser_check.main)
        return snapshot.get('role')=='alert' and 'Filter exceeds 4096 UTF-8 bytes' in snapshot.get('text','')
    return assertive_filter_error(snapshot)

class AlertMetadata(unittest.TestCase):
    def test_observed_role_only_state_is_not_an_assertive_alert_witness(self):
        self.assertFalse(witness(dict(FIXTURE['observed_dom'],text=FIXTURE['text'])))

    def test_literal_assertive_atomic_error_is_a_dom_witness_only(self):
        self.assertTrue(witness(VALID))

    def test_missing_or_changed_fields_and_other_errors_refuse(self):
        for key,value in [('role','status'),('live','polite'),('atomic','false'),('text','Different error')]:
            with self.subTest(key=key):self.assertFalse(witness(dict(VALID,**{key:value})))
        for key in VALID:
            bad=copy.deepcopy(VALID);del bad[key]
            with self.subTest(missing=key):self.assertFalse(witness(bad))

    def test_counterexample_still_fails_the_unchanged_independent_synthesis_oracle(self):
        from announcement_contract import check_alert
        case=dict(FIXTURE['observed_dom'],text=FIXTURE['text'],before_ns=100,after_ns=200,browser_pid=77)
        backend=dict(module='espeak-ng',pid=9,libespeak_mapped=True,
            pcm_before_bytes=FIXTURE['pcm_before_bytes'],pcm_after_bytes=FIXTURE['pcm_after_bytes'])
        with self.assertRaisesRegex(ValueError,'announcement-synthesis'):check_alert(case,[],[],backend)

if __name__=='__main__':unittest.main()
