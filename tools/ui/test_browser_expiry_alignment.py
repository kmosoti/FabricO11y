"""Origin: Firefox C ledger overflow after32 correct native session responses."""
import unittest
from unittest.mock import Mock,patch
import browser_expiry as expiry

def clock(offset=0):
    return Mock(frames=[b'FCLKv1\0\0'+bytes(8)+offset.to_bytes(8,'little')])

class SubsecondPlacement(unittest.TestCase):
    def test_paced_forward_jump_at650ms_does_not_wait_another_second(self):
        target=clock();pace=Mock()
        with patch.object(expiry,'kernel_time',side_effect=[100_650_000_000,101_050_000_000]), \
                patch.object(expiry.time,'monotonic',return_value=0),patch.object(expiry.time,'sleep') as sleep:
            expiry.align(target,1000,240,pace)
        target.advance.assert_called_once_with(900);sleep.assert_not_called();pace.assert_called_once_with()

    def test_outside_dispatch_window_waits_for_a_native_eligible_observation(self):
        for fraction in (49_999_999,700_000_001):
            target=clock()
            with self.subTest(fraction=fraction), \
                    patch.object(expiry,'kernel_time',side_effect=[100_000_000_000+fraction,101_050_000_000]), \
                    patch.object(expiry.time,'monotonic',return_value=0),patch.object(expiry.time,'sleep') as sleep:
                expiry.align(target,1000,240,Mock())
            target.advance.assert_called_once_with(899);sleep.assert_called_once_with(.005)

    def test_adjacent_boundary_at650ms_still_waits_real_time_without_offset_change(self):
        target=clock(10)
        with patch.object(expiry,'kernel_time',side_effect=[100_650_000_000,101_050_000_000]), \
                patch.object(expiry.time,'monotonic',return_value=0),patch.object(expiry.time,'sleep') as sleep:
            expiry.align(target,111,240,Mock())
        target.advance.assert_not_called();sleep.assert_called_once_with(.005)

if __name__=='__main__':unittest.main()
