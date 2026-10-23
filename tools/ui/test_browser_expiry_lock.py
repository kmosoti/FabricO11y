"""Origin: clock-server-diagnostic-01, real401 and locked form, absent old testid."""
import copy
import inspect
import unittest
import browser_expiry

ACTUAL_LOCKED = dict(title='Connect to FabricO11y', sign_in_visible=True,
    principal_visible=True, sign_out_disabled=True, protected_results=False,
    owner_visible=False, legacy_testid=False)

def witness(snapshot):
    if hasattr(browser_expiry,'locked_view'):return browser_expiry.locked_view(snapshot)
    # The old actual predicate asked only for this nonexistent attribute.
    assert '[data-testid=locked-screen]' in inspect.getsource(browser_expiry.run_expiry)
    return snapshot.get('legacy_testid') is True

class LockedViewWitness(unittest.TestCase):
    def test_actual_locked_form_does_not_require_an_unshipped_testid(self):
        self.assertTrue(witness(ACTUAL_LOCKED))

    def test_partial_or_protected_view_is_not_locked_even_with_legacy_testid(self):
        for name,value in [('title','Connected'),('sign_in_visible',False),
                ('principal_visible',False),('sign_out_disabled',False),
                ('protected_results',True),('owner_visible',True)]:
            bad=dict(ACTUAL_LOCKED,legacy_testid=True);bad[name]=value
            with self.subTest(name=name):self.assertFalse(witness(bad))

    def test_missing_observation_is_not_a_locked_view(self):
        for name in ACTUAL_LOCKED:
            if name=='legacy_testid':continue
            bad=copy.deepcopy(ACTUAL_LOCKED);del bad[name]
            with self.subTest(name=name):self.assertFalse(witness(bad))

if __name__=='__main__':unittest.main()
