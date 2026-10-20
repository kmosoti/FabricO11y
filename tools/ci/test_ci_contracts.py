"""Test CI evidence failures and exact extended-profile scheduling coverage."""
from __future__ import annotations

import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits

sys.path.insert(0, str(ROOT / 'tools/ui'))
import fixture_cadence
from fixture_cadence import FixtureCadence
sys.path.insert(0, str(ROOT / 'tools/qualification'))
from check_delivery_fault_control import detected


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


class WorkflowTests(unittest.TestCase):
    @staticmethod
    def browser_entry_contract(workflow):
        fixture = (
            'python3 -B tools/resource_group.py --delegate -- '
            'cargo test --locked -p xtask --test browser_entry '
            'actual_parent_seam_binds_before_releasing_native_short_child '
            '-- --ignored --exact --nocapture')
        pin = 'FABRIC_BROWSER_ENTRY_HELPER_SHA256=$(sha256sum "$helper" | cut -d \' \' -f1)'
        helper_pair = (
            '--browser-entry-helper "$FABRIC_BROWSER_ENTRY_HELPER" '
            '--browser-entry-helper-sha256 "$FABRIC_BROWSER_ENTRY_HELPER_SHA256"')
        security_flag = '--security-probes'
        browser_calls = [line for line in workflow.splitlines() if 'live_browser_check.py' in line]
        normal_positive = next((line for line in browser_calls if line.endswith('--out $FABRIC_STORAGE_ROOT/results/ci-live-browser')), '')
        security_positive = next((line for line in browser_calls if '--out $FABRIC_STORAGE_ROOT/results/ci-live-browser-security' in line), '')
        api_cache_negative = next((line for line in browser_calls if '--inject-api-cache-defect' in line), '')
        fixture_at = workflow.find(fixture)
        build_at = workflow.find('name: Build native authenticated browser fixture')
        pin_at = workflow.find(pin)
        first_browser_at = workflow.find('live_browser_check.py')
        return (
            '  fast-checks:' in workflow
            and fixture_at >= 0
            and build_at >= 0
            and build_at < fixture_at < pin_at < first_browser_at
            and 'helper="$FABRIC_STORAGE_ROOT/cargo/debug/xtask"' in workflow
            and 'test -x "$helper"' in workflow
            and 'FABRIC_BROWSER_ENTRY_HELPER=$helper' in workflow
            and len(browser_calls) == 3
            and all(helper_pair in line for line in browser_calls)
            and normal_positive
            and security_positive
            and api_cache_negative
            and security_flag not in normal_positive
            and security_flag in security_positive
            and '--inject-api-cache-defect' not in normal_positive
            and '--inject-api-cache-defect' not in security_positive
            and security_flag not in api_cache_negative
            and api_cache_negative.endswith(
                '--out $FABRIC_STORAGE_ROOT/results/ci-live-browser-defect --inject-api-cache-defect')
            and 'results/ci-live-browser-security/' in workflow
            and 'cargo xtask check-browser-defect "$defect_status" $FABRIC_STORAGE_ROOT/results/ci-live-browser-defect/browser.json' in workflow
            and 'name: Check actual passkeys and scoped native HTTPS console in Chrome' in workflow
            and 'name: Check the positive Chrome security probes' in workflow
            and 'name: Require the exact API-cache negative-control failure' in workflow)

    def test_hosted_browser_entry_helper_is_required_with_negative_controls(self):
        workflow = (ROOT / '.github/workflows/rust.yml').read_text()
        self.assertTrue(self.browser_entry_contract(workflow), 'hosted native browser entry contract drifted')
        fixture = (
            'python3 -B tools/resource_group.py --delegate -- '
            'cargo test --locked -p xtask --test browser_entry '
            'actual_parent_seam_binds_before_releasing_native_short_child '
            '-- --ignored --exact --nocapture')
        helper_pair = (
            '--browser-entry-helper "$FABRIC_BROWSER_ENTRY_HELPER" '
            '--browser-entry-helper-sha256 "$FABRIC_BROWSER_ENTRY_HELPER_SHA256"')
        security_flag = '--security-probes'
        browser_calls = [line for line in workflow.splitlines() if 'live_browser_check.py' in line]
        normal_positive = next((line for line in browser_calls if line.endswith('--out $FABRIC_STORAGE_ROOT/results/ci-live-browser')), '')
        security_positive = next((line for line in browser_calls if '--out $FABRIC_STORAGE_ROOT/results/ci-live-browser-security' in line), '')
        api_cache_negative = next((line for line in browser_calls if '--inject-api-cache-defect' in line), '')
        missing_launcher = workflow.replace(fixture, '', 1)
        omitted_native_control = workflow.replace(helper_pair, '', 1)
        omitted_normal_positive = workflow.replace(normal_positive, '', 1)
        omitted_security_positive = workflow.replace(security_positive, '', 1)
        omitted_security_probe = workflow.replace(f' {security_flag}', '', 1)
        security_probe_on_normal = workflow.replace(
            normal_positive, normal_positive.replace(' --out ', f' {security_flag} --out '), 1)
        security_probe_on_negative = workflow.replace(
            api_cache_negative, api_cache_negative.replace('--inject-api-cache-defect', f'{security_flag} --inject-api-cache-defect'), 1)
        self.assertNotEqual(missing_launcher, workflow)
        self.assertNotEqual(omitted_native_control, workflow)
        self.assertNotEqual(omitted_normal_positive, workflow)
        self.assertNotEqual(omitted_security_positive, workflow)
        self.assertNotEqual(omitted_security_probe, workflow)
        self.assertNotEqual(security_probe_on_normal, workflow)
        self.assertNotEqual(security_probe_on_negative, workflow)
        self.assertFalse(self.browser_entry_contract(missing_launcher), 'missing Linux launcher was accepted')
        self.assertFalse(self.browser_entry_contract(omitted_native_control), 'omitted helper pair was accepted')
        self.assertFalse(self.browser_entry_contract(omitted_normal_positive), 'omitted normal positive browser check was accepted')
        self.assertFalse(self.browser_entry_contract(omitted_security_positive), 'omitted security positive browser check was accepted')
        self.assertFalse(self.browser_entry_contract(omitted_security_probe), 'missing positive security probes were accepted')
        self.assertFalse(self.browser_entry_contract(security_probe_on_normal), 'security probes were moved onto normal positive browser check')
        self.assertFalse(self.browser_entry_contract(security_probe_on_negative), 'security probes were added to the API-cache negative control')

    def test_extended_workflow_uses_two_shards_and_original_runner(self):
        workflow = (ROOT / '.github/workflows/verification.yml').read_text()
        self.assertIn('checks=(semantic-mutants)', workflow)
        self.assertIn(
            'checks=(delivery-faults tla-delivery tla-transport trace-model cargo-mutants kani-core fuzz-smoke dependency-policy coverage-report)',
            workflow)
        self.assertIn(
            'python3 -B tools/resource_group.py -- cargo xtask checks --only "$check" --receipts "$SHARD_RECEIPTS"',
            workflow)
        self.assertIn(
            'cargo xtask verify-receipts --profile extended --receipts "$FABRIC_STORAGE_ROOT/results/verification-pilot/combined"',
            workflow)
        self.assertNotIn('tools/ci/verify_receipts.py', workflow)
        pilot = (ROOT / '.github/workflows/verification-concurrency-experiment.yml').read_text()
        self.assertIn(
            'cargo xtask verify-receipts --profile extended --receipts "$FABRIC_STORAGE_ROOT/results/verification-pilot/combined"',
            pilot)
        self.assertNotIn('tools/ci/verify_receipts.py', pilot)

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
        checks = {check['id']: check for check in registry['checks']
                  if check.get('profile') == 'fast'}
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
        self.assertIn(
            'verify-receipts --profile fast --receipts "$FABRIC_STORAGE_ROOT/results/ci-fast-receipts"',
            workflow)
        self.assertNotIn('tools/ci/verify_receipts.py', workflow)


if __name__ == '__main__':
    require_limits()
    unittest.main()
