"""Named rejection must survive provenance and environment checks."""
import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location('install_qemu', Path(__file__).with_name('run-qemu.py'))
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class MutationGradingTests(unittest.TestCase):
    def test_all_registered_named_defects_are_required(self):
        for mutation, checks in MODULE.MUTATION_EXPECTED_FAILURES.items():
            output = '\n'.join(f'ACCEPT {check} FAIL injected' for check in sorted(checks))
            self.assertTrue(MODULE.mutation_rejected(mutation, 1, output))
            for check in checks:
                with self.subTest(mutation=mutation, missing=check):
                    self.assertFalse(MODULE.mutation_rejected(
                        mutation, 1, output.replace(f'ACCEPT {check} FAIL', f'ACCEPT {check} PASS')))

    def test_unrelated_failure_is_not_detection(self):
        self.assertFalse(MODULE.mutation_rejected('root-user', 1, 'ACCEPT A12 FAIL timeout'))

    def test_bad_provenance_or_environment_cannot_pass(self):
        output = 'ACCEPT A6 FAIL injected'
        self.assertFalse(MODULE.mutation_rejected('root-user', 1, output, 'helper hash mismatch'))
        self.assertFalse(MODULE.mutation_rejected('root-user', 1, output + '\nACCEPT A8h NOT-RUN unavailable'))
        for code in (None, 0, 2, 3, -9):
            self.assertFalse(MODULE.mutation_rejected('root-user', code, output))


if __name__ == '__main__':
    unittest.main()
