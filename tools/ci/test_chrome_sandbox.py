"""Reject blanket sandbox changes and unrelated Chrome startup errors."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('chrome_sandbox', Path(__file__).with_name('chrome-sandbox.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class SandboxTests(unittest.TestCase):
    def test_only_actual_restricted_sandbox_error_allows_retry(self):
        self.assertTrue(module.restricted_sandbox_failure('FATAL: No usable sandbox!', '1'))
        self.assertFalse(module.restricted_sandbox_failure('missing libnss3.so', '1'))
        self.assertFalse(module.restricted_sandbox_failure('No usable sandbox!', '0'))

    def test_profile_is_exact_and_does_not_disable_chrome_sandbox(self):
        text = module.profile_text(module.CHROME)
        self.assertIn(str(module.CHROME), text)
        self.assertIn('userns,', text)
        self.assertNotIn('*', text)
        with self.assertRaises(RuntimeError):
            module.profile_text(Path('/usr/bin/chrome'))

    def test_actual_namespace_and_seccomp_required(self):
        document = ('<td>PID namespaces</td><td>Yes</td><td>Network namespaces</td><td>Yes</td>'
                    '<td>Seccomp-BPF sandbox</td><td>Yes</td>')
        self.assertTrue(module.sandbox_enabled(document))
        self.assertFalse(module.sandbox_enabled(document.replace('Yes', 'No', 1)))
        self.assertFalse(module.sandbox_enabled('session successfully created'))


if __name__ == '__main__':
    unittest.main()
