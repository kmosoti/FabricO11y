"""Named rejection must survive provenance and environment checks."""
import importlib.util
from pathlib import Path
import unittest
import subprocess
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('install_qemu', Path(__file__).with_name('run-qemu.py'))
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class MutationGradingTests(unittest.TestCase):
    def test_reboot_witness_runs_after_privilege_transition(self):
        # Origin: Fedora upgrade03 really rebooted, but SSH flattened bash -c
        # argv, expanding protected paths too early and leaving a bare test.
        def fake_ssh(args, *, input_text=None, **kwargs):
            shell = '''sudo() { privileged=yes; "$@"; }
cat() {
  [ "$privileged" = yes ] || return 1
  case "$1" in
    /proc/sys/kernel/random/boot_id) echo current ;;
    /root/accept/lifecycle-boot-id) echo "$saved" ;;
    *) return 1 ;;
  esac
}
export -f cat
export privileged saved
'''
            return subprocess.run(['bash', '-c', shell + ' '.join(args[1:])],
                                  input=input_text, text=True, capture_output=True,
                                  env={'PATH': '/usr/bin:/bin', 'saved': saved})
        with patch.object(MODULE, 'run', fake_ssh):
            saved = 'before'
            self.assertEqual(MODULE.changed_boot(['fake-ssh']).returncode, 0)
            saved = 'current'
            self.assertNotEqual(MODULE.changed_boot(['fake-ssh']).returncode, 0)
            saved = 'before'
            old = fake_ssh(['fake-ssh', 'sudo', 'bash', '-c',
                           'test "$(cat /proc/sys/kernel/random/boot_id)" != "$(cat /root/accept/lifecycle-boot-id)"'])
            self.assertNotEqual(old.returncode, 0)

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
