"""Origin: row65 isolation refusal and ordinary Firefox native diagnostics01/02."""
import copy
import inspect
import unittest
import browser_expiry

ACTUAL = dict(executable='/usr/lib64/firefox/firefox',
    executable_sha256='0b59511d21ca9ce96411918ec9297e3e9f1255746cc6d4b8e0246e0b2177a8ad',
    preload_environment=['LD_PRELOAD=libmozsandbox.so'], control_env_names=[],
    helper_mapped=False, control_fd_present=False,
    sandbox_maps=[dict(path='/usr/lib64/firefox/libmozsandbox.so',
        sha256='0d57a6de9a796042a3532664c2f137c447784bb4b0d7da2478e918b748aaa96e')])

def witness(snapshot, browser='firefox'):
    if hasattr(browser_expiry,'ordinary_isolation'):
        return browser_expiry.ordinary_isolation(snapshot,browser)
    assert "item.startswith((b'LD_PRELOAD=',b'FABRIC_CLOCK_FD='))" in inspect.getsource(browser_expiry.run_expiry)
    return (snapshot['helper_mapped'] is False and snapshot['control_fd_present'] is False
        and not snapshot['preload_environment'] and not snapshot['control_env_names'])

class OrdinaryIsolation(unittest.TestCase):
    def test_actual_firefox_sandbox_is_not_the_fixture_clock(self):
        self.assertTrue(witness(ACTUAL))

    def test_ordinary_process_without_preload_is_still_accepted(self):
        self.assertTrue(witness(dict(ACTUAL,preload_environment=[],sandbox_maps=[]),'chrome'))

    def test_clock_escape_and_unpinned_preload_are_rejected(self):
        changes=[('helper_mapped',True),('control_fd_present',True),
            ('control_env_names',['FABRIC_CLOCK_FD']),
            ('executable','/usr/bin/foreign'),('executable_sha256','0'*64),
            ('sandbox_maps',[]),('sandbox_maps',[dict(ACTUAL['sandbox_maps'][0],sha256='0'*64)]),
            ('sandbox_maps',[dict(ACTUAL['sandbox_maps'][0],path='/foreign/libmozsandbox.so')]),
            ('preload_environment',['LD_PRELOAD=/private/session-clock.so']),
            ('preload_environment',['LD_PRELOAD=libmozsandbox.so:/private/session-clock.so']),
            ('preload_environment',ACTUAL['preload_environment']*2)]
        for key,value in changes:
            with self.subTest(key=key,value=value):
                self.assertFalse(witness(dict(copy.deepcopy(ACTUAL),**{key:value})))
        self.assertFalse(witness(ACTUAL,'chrome'))

    def test_missing_or_wrongly_typed_isolation_facts_refuse(self):
        for key in ('helper_mapped','control_fd_present','control_env_names','preload_environment'):
            bad=copy.deepcopy(ACTUAL);del bad[key]
            with self.subTest(key=key):
                if hasattr(browser_expiry,'ordinary_isolation'):self.assertFalse(witness(bad))
                else:self.assertRaises(KeyError,witness,bad)

if __name__=='__main__':unittest.main()
