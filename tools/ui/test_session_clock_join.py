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
