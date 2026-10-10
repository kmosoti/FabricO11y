"""Test CI evidence failures and exact extended-profile scheduling coverage."""
from __future__ import annotations

from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile
import unittest

from verify_receipts import ROOT, require_limits, selected_checks, verify

COMMIT = '1' * 40


class ReceiptTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(
            prefix='ci-receipts-', dir=os.environ['FABRIC_SCRATCH_ROOT'])
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.check = {'id': 'example', 'profile': 'extended', 'command': ['example-check']}
        self.registry = {'checks': [self.check]}
        self.log = b'synthetic observed output\n'
        self.receipt = {
            'receipt_version': 1, 'check_id': 'example', 'profile': 'extended',
            'candidate_commit': COMMIT, 'worktree_dirty': False,
            'command': ['example-check'], 'result': 'passed', 'exit_code': 0,
            'duration_ms': 12, 'output_log': 'example.log',
            'output_sha256': hashlib.sha256(self.log).hexdigest(),
        }
        self.write()

    def write(self):
        (self.root / 'example.json').write_text(json.dumps(self.receipt))
        (self.root / 'example.log').write_bytes(self.log)

    def check_result(self):
        return verify(self.registry, 'extended', COMMIT, self.root)

    def test_complete_evidence_is_accepted(self):
        result = self.check_result()
        self.assertEqual(result['check_count'], 1)
        self.assertEqual(result['total_check_ms'], 12)

    def test_missing_receipt_is_not_success(self):
        (self.root / 'example.json').unlink()
        with self.assertRaises(FileNotFoundError):
            self.check_result()

    def test_missing_or_tampered_log_is_not_success(self):
        (self.root / 'example.log').write_bytes(b'replaced')
        with self.assertRaises(ValueError):
            self.check_result()
        (self.root / 'example.log').unlink()
        with self.assertRaises(FileNotFoundError):
            self.check_result()

    def test_failure_and_incomplete_cannot_be_relabelled_by_wrapper(self):
        for result, code in [('failed', 1), ('environment-unavailable', None), ('passed', 3)]:
            with self.subTest(result=result, code=code):
                self.receipt.update(result=result, exit_code=code)
                self.write()
                with self.assertRaises(ValueError):
                    self.check_result()

    def test_stale_dirty_or_different_command_receipt_is_rejected(self):
        for key, value in [('candidate_commit', '2' * 40), ('worktree_dirty', True),
                           ('command', ['different-check']), ('profile', 'fast')]:
            original = self.receipt[key]
            self.receipt[key] = value
            self.write()
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.check_result()
            self.receipt[key] = original

    def test_running_marker_overrides_an_old_success(self):
        (self.root / 'current-check.json').write_text('{"state":"running"}')
        with self.assertRaisesRegex(ValueError, 'running'):
            self.check_result()

    def test_invalid_or_duplicate_ids_and_empty_profile_are_refused(self):
        for registry in [{'checks': []}, {'checks': [self.check, self.check]},
                         {'checks': [{**self.check, 'id': '../escape'}]}]:
            with self.subTest(registry=registry), self.assertRaises(ValueError):
                selected_checks(registry, 'extended')

    def test_symlinked_evidence_is_refused(self):
        (self.root / 'example.log').rename(self.root / 'other.log')
        (self.root / 'example.log').symlink_to('other.log')
        with self.assertRaises(ValueError):
            self.check_result()


class WorkflowTests(unittest.TestCase):
    def test_extended_workflow_schedules_each_registered_check_exactly_once(self):
        registry = json.loads((ROOT / 'xtask/checks.json').read_text())
        expected = [check['id'] for check in selected_checks(registry, 'extended')]
        workflow = (ROOT / '.github/workflows/verification.yml').read_text()
        scheduled = re.findall(
            r'^\s+run: python3 -B tools/resource_group\.py -- cargo xtask checks --only ([a-z0-9-]+)\s*$',
            workflow, re.MULTILINE)
        self.assertEqual(Counter(scheduled), Counter(expected))
        self.assertIn('verify_receipts.py --profile extended', workflow)

    def test_fast_duplicate_removals_still_have_registry_owners(self):
        registry = json.loads((ROOT / 'xtask/checks.json').read_text())
        checks = {check['id']: check for check in selected_checks(registry, 'fast')}
        expected = {
            'wasm-check': ['cargo', 'check', '--locked', '-p', 'fabric-ui', '--target', 'wasm32-unknown-unknown'],
            'ui-palette': ['python3', '-B', 'tools/ui/contrast_check.py'],
            'package-contracts': ['python3', '-B', 'tools/packaging/test_packaging.py'],
        }
        for identifier, command in expected.items():
            with self.subTest(identifier=identifier):
                self.assertEqual(checks[identifier]['command'], command)
        workflow = (ROOT / '.github/workflows/rust.yml').read_text()
        self.assertIn('cargo xtask checks --profile fast', workflow)
        self.assertIn('verify_receipts.py --profile fast', workflow)


if __name__ == '__main__':
    require_limits()
    unittest.main()
