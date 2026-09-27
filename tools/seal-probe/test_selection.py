"""Integrator probes: detect corrupt lengths masquerading as missing seals.

These supplement the frozen independent oracle after candidate inspection.
"""
import importlib.util
from pathlib import Path
import sys
import unittest
from oracle_support import bodies, group_bytes


class MetadataCorruption(unittest.TestCase):
    def test_append_rejects_nonsequential_ids_and_uncommitted_previous_bytes(self):
        prior = group_bytes(0, bodies(1,11), 512)
        for previous, group_id in [(prior,0),(prior,2),(prior+bytes(512),1),(bytes(512),0)]:
            with self.subTest(length=len(previous),group_id=group_id):
                with self.assertRaises(ValueError):
                    self.impl.append_logical(previous,group_id,bodies(1,12),512,None)
        bad = bytearray(prior); bad[512+12] ^= 1
        with self.assertRaises(ValueError):
            self.impl.append_logical(bytes(bad),1,bodies(1,12),512,None)
        valid = self.impl.append_logical(prior,1,bodies(1,12),512,None)
        self.assertTrue(valid[1])
        self.assertEqual(self.impl.recover(valid[0],512)[0], 'ok')

    def test_single_acked_group_metadata_bit_flips_preserve_all_bytes(self):
        checked = 0
        for sector in (512,4096):
            for count in range(1,7):
                clean = group_bytes(0,bodies(count,11),sector)
                for offset in list(range(40)) + list(range(len(clean)-sector,len(clean)-sector+48)):
                    for bit in range(8):
                        changed = bytearray(clean)
                        changed[offset] ^= 1 << bit
                        changed = bytes(changed)
                        actual = self.impl.recover(changed,sector)
                        self.assertEqual(actual,('error',changed),
                            f'ACKed group discarded: sector={sector}, count={count}, offset={offset}, bit={bit}; result={actual[:1]}')
                        checked += 1
        self.assertEqual(checked,8448)
        print(f'8,448 deterministic descriptor/seal bit flips failed closed')


if __name__=='__main__':
    path=Path(sys.argv.pop(1)).resolve()
    spec=importlib.util.spec_from_file_location('selection_candidate',path)
    impl=importlib.util.module_from_spec(spec)
    spec.loader.exec_module(impl)
    MetadataCorruption.impl=impl
    unittest.main()
