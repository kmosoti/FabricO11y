"""Synthetic literal record controls; never native acceptance evidence."""
import copy
import hashlib
import json
from pathlib import Path
import unittest

from owner_commands import PHASES
from owner_evidence import completed

CONTROLS = Path(__file__).resolve().parents[2] / 'docs/experiments/formal/fixtures/owner-walkthrough.json'


def fixture():
    controls = json.loads(CONTROLS.read_text())
    binding = {'original_head': controls['original_head'], 'epoch': controls['source_date_epoch'],
               'deb_sha256': 'a' * 64, 'rpm_sha256': 'b' * 64,
               'source_commit': 'c' * 40, 'guide_commit': 'd' * 40, 'harness_commit': 'e' * 40}
    enrollment = {'flags': [{'up': True, 'uv': True, 'rp_hash': hashlib.sha256(b'telemetry.example.com').hexdigest()}] * 2,
                  'devices': [{'count': 1, 'credentials': [{'resident': True, 'rp': 'telemetry.example.com'}]}] * 2}
    record = {'binding': copy.deepcopy(binding),
              'phases': [{'name': name, 'start_ns': i, 'end_ns': i + 1, 'deadline_ns': i + 2, 'exit': 0}
                         for i, (name, _) in enumerate(PHASES)],
              'revoked_edge_http': 401, 'old_owner_denied': True, 'stale_session_http': 401,
              'old_owner_http': 403, 'old_owner_error': 'access denied',
              'old_owner_assertion': dict(one_shot=True, original_allowlist_count=1, rp_id='telemetry.example.com',
                  verification_unchanged=True, challenge_matches=True, origin='https://telemetry.example.com:7443',
                  type='webauthn.get', rp_hash=hashlib.sha256(b'telemetry.example.com').hexdigest(), up=True, uv=True),
              'signals': {name: {'raw_groups': 1, 'verdicts': [{'passed': True}] * 3}
                          for name in ('fixture', 'restored', 'upgraded', 'fresh-candidate')},
              'native_older': {'debian': True, 'fedora': True}, 'upgrade_preserved': True,
              'owner_enrollments': [copy.deepcopy(enrollment) for _ in range(3)], 'cleanup_confirmed': True}
    return record, binding


class OwnerEvidenceTests(unittest.TestCase):
    def check(self, record, binding):
        return completed(record, binding, dict(PHASES))

    def test_explicitly_synthetic_complete_record(self):
        self.assertEqual(self.check(*fixture())['exit'], 0)

    def test_registered_literal_record_defects(self):
        controls = json.loads(CONTROLS.read_text())['record_controls']
        for control in controls:
            with self.subTest(defect=control['defect']):
                record, binding = fixture()
                defect = control['defect']
                if defect == 'wrong-original-head': record['binding']['original_head'] = 'f' * 40
                elif defect == 'wrong-epoch': record['binding']['epoch'] += 1
                elif defect == 'missing-restore': del record['signals']['restored']
                elif defect == 'missing-actual-candidate-fresh-enrollment': del record['signals']['fresh-candidate']
                elif defect == 'foreign-package-digest': record['binding']['deb_sha256'] = 'f' * 64
                elif defect.startswith('HTTP'): record['revoked_edge_http'] = int(defect[4:7])
                elif defect.startswith(('timeout-', 'connection-reset-')): record['revoked_edge_http'] = None
                else: self.fail('unimplemented registered defect: ' + defect)
                with self.assertRaises(ValueError): self.check(record, binding)

    def test_old_owner_refusal_requires_exact_status_body_and_native_assertion(self):
        for key, value in [('old_owner_http', 200), ('old_owner_http', 401), ('old_owner_http', 429),
                           ('old_owner_http', 503), ('old_owner_error', 'different denial'),
                           ('old_owner_assertion', None)]:
            record, binding = fixture()
            record[key] = value
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                self.check(record, binding)

    def test_phase_failure_and_omission_never_complete(self):
        for change in ('missing', 'late', 'nonzero', 'cleanup'):
            record, binding = fixture()
            if change == 'missing': record['phases'].pop(2)
            elif change == 'late': record['phases'][2]['end_ns'] = 99
            elif change == 'nonzero': record['phases'][2]['exit'] = 1
            else: record['cleanup_confirmed'] = False
            with self.assertRaises(ValueError): self.check(record, binding)

    def test_configuration_does_not_substitute_for_actual_uv_and_residency(self):
        for field in ('uv', 'up', 'rp_hash', 'resident'):
            record, binding = fixture()
            if field == 'resident': record['owner_enrollments'][0]['devices'][0]['credentials'][0][field] = False
            else: record['owner_enrollments'][0]['flags'][0][field] = False
            with self.assertRaises(ValueError): self.check(record, binding)


if __name__ == '__main__':
    unittest.main()
