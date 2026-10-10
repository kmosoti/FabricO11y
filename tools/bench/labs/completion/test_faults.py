"""Controls for cleanup before recovery can erase leaked builder files."""
import tempfile
import unittest
from pathlib import Path

import faults


class CleanupEvidence(unittest.TestCase):
    def test_ordinary_errors_require_immediate_cleanup(self):
        good = dict(injected=True, failed_exit=1, input_unchanged=True,
                    retry_exit=0, exact_manifest=True, leftovers=[], pre_restart_leftovers=[])
        self.assertTrue(faults.grade(good, False))
        for field, wrong in [('injected', False), ('failed_exit', 0),
                             ('input_unchanged', False), ('retry_exit', 1),
                             ('exact_manifest', False), ('leftovers', ['.run-0-0']),
                             ('pre_restart_leftovers', ['.building-1'])]:
            self.assertFalse(faults.grade(dict(good, **{field: wrong}), False), field)
        killed = dict(good, failed_exit=-9, pre_restart_leftovers=['.building-1'])
        self.assertTrue(faults.grade(killed, True))
        self.assertFalse(faults.grade(dict(killed, failed_exit=1), True))
        self.assertFalse(faults.grade(dict(killed, leftovers=['.building-1']), True))

    def test_discovery_detects_spill_files_outside_build_directories(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory)
            (state / 'segments').mkdir()
            (state / 'segments/.building-1').mkdir()
            (state / 'segments/.run-0-0').write_bytes(b'leaked')
            (state / 'segments/seg-1').mkdir()
            (state / 'segments/seg-1/logs.parquet').write_bytes(b'committed')
            self.assertEqual(faults.leftovers(state),
                             ['segments/.building-1', 'segments/.run-0-0'])


if __name__ == '__main__':
    unittest.main()
