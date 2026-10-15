"""Synthetic caller controls; actual native/browser phases remain separate."""
import base64
import copy
import hashlib
import json
import os
from pathlib import Path
import tempfile
import types
import unittest
from unittest.mock import patch

import owner_walkthrough as W
from owner_evidence import grade_queries
from cross_family.test_snapshot import IndependentSnapshots


class WalkthroughCallerTests(unittest.TestCase):
    def test_wrong_candidate_source_refused_before_fixture_or_guest(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root = Path(directory)
            receipt = root / 'deb.json'
            receipt.write_text(json.dumps({'source': {'original_head': 'a' * 40, 'source_date_epoch': 17}}))
            artifact = root / 'payload'; artifact.write_bytes(b'literal candidate')
            identity = {'source_commit': 'b' * 40}
            for key, value in (('original_head', 'c' * 40), ('epoch', 18)):
                plan = {'harness_commit': 'd' * 40, 'guide_commit': 'e' * 40, 'harness_files': {},
                        'deb_receipt': str(receipt), 'rpm_receipt': str(receipt),
                        'original_head': 'a' * 40, 'epoch': 17, 'source_commit': 'b' * 40}
                plan[key] = value
                path = root / ('plan-' + key); path.write_text(json.dumps(plan))
                def git(argv, **kwargs):
                    return types.SimpleNamespace(stdout='d' * 40 if 'rev-parse' in argv else '')
                with patch.object(W.Q, 'run', side_effect=git), \
                     patch.object(W, 'validate_inputs', return_value={'identity': identity}), \
                     patch.object(W, 'Guest') as guest:
                    with self.assertRaisesRegex(ValueError, 'candidate source differs'):
                        W.admit(path, hashlib.sha256(path.read_bytes()).hexdigest())
                    guest.assert_not_called()

    def test_installed_payload_drift_cannot_reach_version_or_completed_stage(self):
        walk = W.Walkthrough.__new__(W.Walkthrough)
        walk.artifacts = {'identity': {'payload_identity': {'usr/bin/fabric-node': {'bytes': 3, 'sha256': 'a' * 64}}}}
        walk.record = {'events': []}
        walk.observed = lambda *args: json.dumps({'usr/bin/fabric-node': {'bytes': 3, 'sha256': 'b' * 64}})
        with self.assertRaisesRegex(ValueError, 'installed binaries/complete console differ'):
            walk.installed_payload(types.SimpleNamespace(role='edge', family='debian'), True)

    def test_corrupt_original_journal_cannot_supply_query_expectations(self):
        raw = IndependentSnapshots().fixture()
        for changed in (raw[:-1], b'BAD!' + raw[4:]):
            with self.subTest(size=len(changed)):
                with self.assertRaises(ValueError):
                    grade_queries([{'file': 'batches.faj', 'bytes': base64.b64encode(changed).decode()}], [], [], b'one\n')

    def test_replay_helper_cannot_replace_independent_original_bytes(self):
        raw = IndependentSnapshots().fixture()
        with self.assertRaisesRegex(ValueError, 'independent raw journal differs'):
            grade_queries([{'file': 'batches.faj', 'bytes': base64.b64encode(raw).decode()}], [], [], b'one\n')

    def test_unregistered_usernet_override_refused_before_guest_paths(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            path = Path(directory)
            with self.assertRaisesRegex(ValueError, 'registered walkthrough network'):
                W.Guest('server', 'debian', path, path, path / 'key', 4096, network='10.22.0.0/16')
            self.assertFalse((path / 'server').exists())


if __name__ == '__main__':
    unittest.main()
