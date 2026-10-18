"""Test CI evidence failures and exact extended-profile scheduling coverage."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest
import sys

from verify_receipts import ROOT, current_identity, require_limits, selected_checks, verify

COMMIT = '1' * 40
sys.path.insert(0, str(ROOT / 'tools/ui'))
import fixture_cadence
from fixture_cadence import FixtureCadence
sys.path.insert(0, str(ROOT / 'tools/qualification'))
from check_delivery_fault_control import detected
from test_browser_defect import BrowserDefectTests  # Include the hosted counterexample in this gate.


class DeliveryControlTests(unittest.TestCase):
    def test_unrelated_failures_cannot_satisfy_the_ack_loss_control(self):
        good = dict(mutate='drop-recovered', passed=False, oracle_passed=False,
                    companion_observed=True, server_exit=0, node_exits=[0,0,0],
                    acks=5, recovered=4, violations=[{'rule':'ACKED-DURABLE'}])
        self.assertTrue(detected(1, good))
        for status, change in [(0, {}), (2, {}), (1, {'acks':0}),
                               (1, {'node_exits':[0,1,0]}),
                               (1, {'violations':[{'rule':'NO-FABRICATION'}]}),
                               (1, {'violations':[]}), (1, {'server_exit':1})]:
            self.assertFalse(detected(status, {**good, **change}))


class CadenceTests(unittest.TestCase):
    def assert_native_peer_limits(self, source):
        declaration = re.search(r'const LIMITS\s*:\s*\[Limits;\s*2\]\s*=\s*\[(.*?)\];', source, re.S)
        self.assertIsNotNone(declaration, 'native peer admission declaration missing')
        fields = ('rate', 'burst', 'peer_live', 'global_live')
        limits = []
        for entry in re.findall(r'Limits\s*\{([^}]+)\}', declaration[1]):
            values = re.findall(r'\b(rate|burst|peer_live|global_live)\s*:\s*(\d+)\s*,', entry)
            self.assertEqual(len(values), len(fields), 'native peer admission fields missing or duplicated')
            values = dict(values)
            self.assertEqual(set(values), set(fields), 'native peer admission fields missing or duplicated')
            limits.append(tuple(int(values[field]) for field in fields))
        self.assertEqual(limits, [fixture_cadence.PEER_ADMISSION[lane]['limits']
                                 for lane in ('public', 'private')], 'native peer admission limits drifted')

    def test_native_peer_limits_match_the_caller_mirror_and_reject_drift(self):
        source = (ROOT / 'crates/fabric-server/src/peer_admission.rs').read_text()
        self.assert_native_peer_limits(source)
        for original, replacement in (('rate: 8,', 'rate: 7,'), ('peer_live: 4,', 'peer_live: 3,')):
            changed = source.replace(original, replacement)
            self.assertNotEqual(changed, source, 'representative drift was not injected')
            with self.subTest(field=original), self.assertRaisesRegex(AssertionError, 'limits drifted'):
                self.assert_native_peer_limits(changed)

    def test_actions_and_probes_share_one_schedule_without_replays(self):
        for interval, minimum, initial, duration in ((None, .550, 0.0, .01),
                                                     (.500, .500, 0.0, .01),
                                                     (.250, .250, 0.0, .01),
                                                     (250_000_000, 250_000_000, 10**18 + 1, 10_000_000)):
            with self.subTest(interval=interval):
                now, sleeps = [initial], []
                def sleep(delay):
                    sleeps.append(delay)
                    now[0] += delay
                cadence = FixtureCadence(lambda: now[0], sleep,
                                         **({} if interval is None else {'interval': interval}))
                starts = []
                for _ in range(20):
                    cadence.pace()
                    starts.append(now[0])
                    now[0] += duration
                self.assertEqual(len(starts), 20)
                self.assertTrue(all(b-a >= minimum-1e-12 for a,b in zip(starts,starts[1:])))
                self.assertEqual(len(sleeps), 19)
                now[0] += 1000 * duration
                cadence.pace()
                self.assertEqual(len(sleeps), 19)


class FuzzFreezeTests(unittest.TestCase):
    """A stale or rewritten fuzz lock must not become clean-trial evidence."""
    def trial(self, control):
        with tempfile.TemporaryDirectory(prefix='fuzz-freeze-',
                                          dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root = Path(directory)
            (root / 'fuzz').mkdir()
            (root / 'bin').mkdir()
            shutil.copyfile(ROOT / 'fuzz/smoke.sh', root / 'fuzz/smoke.sh')
            (root / 'fuzz/Cargo.lock').write_text('frozen-input\n')
            cargo = root / 'bin/cargo'
            cargo.write_text('''#!/usr/bin/env python3
import os, pathlib, sys
args = sys.argv[1:]
with open('calls', 'a') as stream:
    stream.write(' '.join(args) + '\\n')
control = os.environ['FABRIC_FUZZ_CHECK_CONTROL']
if 'metadata' in args:
    assert '--locked' in args
    sys.exit(101 if control == 'stale' else 0)
if 'fuzz' in args:
    if control == 'rewrite':
        pathlib.Path('fuzz/Cargo.lock').write_text('rewritten-input\\n')
    print('Done 1 runs')
''')
            cargo.chmod(0o700)
            marker = root / 'bin/cargo-fuzz'
            marker.write_text('#!/bin/sh\nexit 0\n')
            marker.chmod(0o700)
            result = subprocess.run(['bash', str(root / 'fuzz/smoke.sh'), '60'],
                cwd=root, env=dict(os.environ,
                    PATH=str(root / 'bin') + ':' + os.environ['PATH'],
                    FABRIC_FUZZ_CHECK_CONTROL=control),
                text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30)
            return result, (root / 'calls').read_text().splitlines()

    def test_stale_lock_is_rejected_before_any_fuzz_target(self):
        result, calls = self.trial('stale')
        self.assertEqual(result.returncode, 1)
        self.assertIn('dependency lock does not resolve unchanged', result.stdout)
        self.assertFalse(any(' fuzz ' in call for call in calls))

    def test_mid_trial_lock_rewrite_is_rejected(self):
        result, calls = self.trial('rewrite')
        self.assertEqual(result.returncode, 1)
        self.assertIn('dependency lock changed during trial', result.stdout)
        self.assertEqual(sum(' fuzz ' in call for call in calls), 4)

    def test_frozen_trial_retains_all_targets_and_original_bounds(self):
        result, calls = self.trial('clean')
        self.assertEqual(result.returncode, 0, result.stdout)
        fuzz = [call for call in calls if ' fuzz ' in call]
        self.assertEqual([call.split()[3] for call in fuzz],
                         ['batch_identify', 'query_request', 'frame_recovery', 'observation_block'])
        self.assertTrue(all('-max_total_time=60' in call and '-rss_limit_mb=2048' in call
                            for call in fuzz))


class LiveCandidateTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='ci-live-input-',
                                                    dir=os.environ['FABRIC_SCRATCH_ROOT'])
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.git('init', '--quiet')
        self.git('config', 'core.hooksPath', '/dev/null')
        (self.root / 'input').write_text('frozen\n')
        self.git('add', 'input')
        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                 '-c', 'commit.gpgsign=false', 'commit', '--quiet', '-m', 'freeze')

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.root,
                                       text=True, stderr=subprocess.STDOUT, timeout=10).strip()

    def test_clean_live_candidate_identity_is_accepted(self):
        self.assertEqual(current_identity(self.root), self.git('rev-parse', 'HEAD'))

    def test_input_changes_after_receipts_are_rejected(self):
        for tracked in (True, False):
            with self.subTest(tracked=tracked):
                source = self.root / ('input' if tracked else 'new-input')
                source.write_text('changed after checks\n')
                with self.assertRaisesRegex(ValueError, 'worktree changed'):
                    current_identity(self.root)
                if tracked:
                    source.write_text('frozen\n')
                else:
                    source.unlink()


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
    def assert_two_shard_contract(self, workflow):
        arrays = re.findall(r'^\s+checks=\(([^)]*)\)\s*$', workflow, re.MULTILINE)
        self.assertEqual(arrays, [
            'semantic-mutants',
            'delivery-faults tla-delivery tla-transport trace-model cargo-mutants kani-core fuzz-smoke dependency-policy coverage-report',
        ])
        self.assertEqual(sum(line.strip() == 'max-parallel: 2' for line in workflow.splitlines()), 1)
        self.assertEqual(sum(line.strip() == 'fail-fast: false' for line in workflow.splitlines()), 1)
        self.assertIn(
            'python3 -B tools/resource_group.py -- cargo xtask checks --only "$check" --receipts "$SHARD_RECEIPTS"',
            workflow)
        aggregate = workflow.split('\n  extended:\n', 1)[1]
        header, body = aggregate.split('    steps:\n', 1)
        self.assertIn('needs: shards', [line.strip() for line in header.splitlines()])
        self.assertIn(
            "if: ${{ always() && (github.event_name != 'pull_request' || github.event.pull_request.draft == false) }}",
            [line.strip() for line in header.splitlines()])
        self.assertIn('name: pilot-receipts-${{ matrix.shard }}', workflow)
        self.assertIn('pattern: pilot-receipts-*', body)
        self.assertIn('merge-multiple: false', body)
        self.assertIn('cargo run --locked -p xtask --example merge_verification_receipts', body)
        self.assertIn('python3 -B tools/resource_group.py -- python3 -B tools/ci/verify_receipts.py --profile extended', body)
        self.assertIn('if [ "$SHARD_RESULT" != success ]; then', body)

    def test_extended_workflow_uses_two_shards_and_original_runner(self):
        workflow = (ROOT / '.github/workflows/verification.yml').read_text()
        self.assert_two_shard_contract(workflow)

    def test_shard_contract_rejects_broken_aggregate_and_worker_bound(self):
        workflow = (ROOT / '.github/workflows/verification.yml').read_text()
        mutations = (
            ('max-parallel: 2', 'max-parallel: 20'),
            ('    needs: shards\n', '    needs: other\n'),
            ("    if: ${{ always() && (github.event_name != 'pull_request' || github.event.pull_request.draft == false) }}",
             "    if: ${{ success() && (github.event_name != 'pull_request' || github.event.pull_request.draft == false) }}"),
            ('tools/ci/verify_receipts.py --profile extended', 'tools/ci/verify_receipts.py --profile fast'),
        )
        for before, after in mutations:
            with self.subTest(mutation=before):
                self.assertIn(before, workflow)
                with self.assertRaises(AssertionError):
                    self.assert_two_shard_contract(workflow.replace(before, after, 1))

    def test_extended_workflow_runs_when_ready_not_for_drafts(self):
        workflow = (ROOT / '.github/workflows/verification.yml').read_text()
        self.assertRegex(workflow, r'(?m)^  pull_request:\n    types: \[[^\]]*\bready_for_review\b')
        self.assertRegex(workflow, r'(?m)^  merge_group:')
        self.assertRegex(workflow, r'(?m)^  schedule:')
        self.assertRegex(workflow, r'(?m)^  workflow_dispatch:')
        self.assertRegex(
            workflow,
            r"(?m)^  extended:\n    needs: shards\n    if: \$\{\{ always\(\) && \(github\.event_name != 'pull_request' \|\| github\.event\.pull_request\.draft == false\) \}\}$")

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
