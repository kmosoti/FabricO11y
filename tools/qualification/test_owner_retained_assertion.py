"""Literal witness controls; actual native signed positive/negative probe is separate."""
import copy
import hashlib
import unittest
from owner_walkthrough import verify_retained_resident_assertion


class RetainedAssertionTests(unittest.TestCase):
    def test_only_the_complete_native_challenge_origin_rp_and_uv_witness_passes(self):
        origin, rp = 'https://telemetry.example.com:7443', 'telemetry.example.com'
        value = dict(one_shot=True, original_allowlist_count=1, rp_id=rp,
                     verification_unchanged=True, challenge_matches=True, origin=origin,
                     type='webauthn.get', rp_hash=hashlib.sha256(rp.encode()).hexdigest(), up=True, uv=True)
        verify_retained_resident_assertion(value, origin, rp)
        for key, replacement in [('one_shot', False), ('original_allowlist_count', 0),
                                 ('rp_id', 'evil.example'), ('verification_unchanged', False),
                                 ('challenge_matches', False), ('origin', 'https://evil.example'),
                                 ('type', 'webauthn.create'), ('rp_hash', '0' * 64), ('up', False),
                                 ('uv', False), ('error', {'name': 'NotAllowedError'})]:
            changed = copy.deepcopy(value)
            changed[key] = replacement
            with self.subTest(key=key), self.assertRaises(ValueError):
                verify_retained_resident_assertion(changed, origin, rp)
