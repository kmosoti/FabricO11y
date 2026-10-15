"""Literal prospective controls for native request-window evidence."""
import copy
import unittest
import session_clock_oracle as oracle


def ledger():
    return [dict(pid=700, tid=701, call_index=i, sequence=1, offset_s=1799,
                 clock_id=0, raw_ns=100_500_000_000, returned_ns=1899_500_000_000,
                 monotonic_before_ns=200_000_000_000+i*100,
                 monotonic_after_ns=200_000_000_050+i*100) for i in (1,2)]


class NativeRequestJoinTests(unittest.TestCase):
    def check(self, rows):
        return oracle.check_request_window(rows, 700, 1, 1799, 1899,
                                           200_000_000_000, 200_000_001_000)

    def test_all_possible_native_calls_are_retained(self):
        self.assertEqual(self.check(ledger()), ledger())

    def test_literal_defects_refuse_their_named_boundary(self):
        for index, field, value, error in (
            (1,'call_index',3,'request-ledger-index'),
            (1,'call_index',1,'request-ledger-index'),
            (0,'tid',0,'request-ledger-fields'),
            (0,'pid',702,'request-ledger-pid'),
            (0,'sequence',2,'request-native-frame'),
            (0,'offset_s',1800,'request-native-frame'),
            (0,'returned_ns',1899_500_000_001,'request-native-wall'),
            (0,'monotonic_before_ns',199_999_999_999,'request-native-straddle'),
            (1,'returned_ns',1900_500_000_000,'request-native-wall')):
            changed=copy.deepcopy(ledger());changed[index][field]=value
            with self.subTest(field=field), self.assertRaisesRegex(ValueError,error):self.check(changed)
        with self.assertRaisesRegex(ValueError,'request-native-missing'):self.check([])
        changed=ledger();changed[0]['monotonic_after_ns']=199_999_999_999
        with self.assertRaisesRegex(ValueError,'request-ledger-fields'):self.check(changed)
        with self.assertRaisesRegex(ValueError,'request-ledger-bound'):
            self.check(ledger()*10000)


if __name__=='__main__':unittest.main()

class ServerExecBoundaryTests(unittest.TestCase):
    def test_entry_helper_sets_clock_only_after_cgroup_placement(self):
        import os
        from unittest.mock import patch
        import session_clock_exec as entry
        events=[]
        def entered(group):
            self.assertEqual(group,'/owned/group')
            self.assertNotIn('LD_PRELOAD',os.environ);events.append('entered')
        def executed(binary,command):
            self.assertEqual(events,['entered'])
            self.assertEqual(binary,'/owned/server');self.assertEqual(command,['/owned/server','serve','/owned/config'])
            self.assertEqual(os.environ['LD_PRELOAD'],'/owned/helper.so')
            self.assertEqual(os.environ['FABRIC_CLOCK_FD'],'7');events.append('exec')
        with patch.dict(os.environ,{},clear=True), patch.object(entry.cgroups,'enter',side_effect=entered), \
             patch.object(entry.os,'get_inheritable',return_value=True), patch.object(entry.os,'execv',side_effect=executed):
            entry.main(['/owned/group','/owned/helper.so','7','/owned/server','serve','/owned/config'])
        self.assertEqual(events,['entered','exec'])

    def test_preloaded_launcher_and_missing_descriptor_refuse_before_exec(self):
        import os
        from unittest.mock import patch
        import session_clock_exec as entry
        for environment,inherited,refusal in [({'LD_PRELOAD':'/bad'},True,'fixture-environment'),
                ({'FABRIC_CLOCK_FD':'7'},True,'fixture-environment'),({},False,'fixture-server-control')]:
            with patch.dict(os.environ,environment,clear=True), patch.object(entry.cgroups,'enter') as enter, \
                 patch.object(entry.os,'get_inheritable',return_value=inherited), patch.object(entry.os,'execv') as execute:
                with self.assertRaisesRegex(ValueError,refusal):entry.main(['/owned/group','/owned/helper.so','7','/owned/server'])
                enter.assert_not_called();execute.assert_not_called()
