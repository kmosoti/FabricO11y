"""Literal protocol outcomes; source controls do not establish native browser acceptance."""
import copy
import unittest
import browser_security as security


def complete():
    result=[]
    for name in ('registration-replay','login-replay','wrong-origin','wrong-rp','missing-uv','attacker-cookie','previous-cookie','reader-expiry'):
        protected=name.endswith('cookie') or name=='reader-expiry'
        row={'property':name,'status':401 if protected else 403,'error':'sign in required' if protected else 'access denied','positive_status':200}
        if name in ('wrong-origin','wrong-rp','missing-uv'):
            row.update(signature_verified=True,positive_signature_verified=True)
        if name.endswith('cookie'):
            row.update(old_sha256='a'*64,new_sha256='b'*64)
        if name=='reader-expiry':
            row.update(before_status=200,long_reader_status=200,wait_ns=4_100_000_000,wall_elapsed_ns=4_100_000_000,wall_after_s=105,expires_unix_s=103)
        result.append(row)
    return result


class SecurityControls(unittest.TestCase):
    def test_literal_positive(self):
        self.assertTrue(security.grade(complete()))
    def test_each_missing_or_duplicate_property(self):
        for i in range(8):
            for rows in (complete()[:i]+complete()[i+1:],complete()+[complete()[i]]):
                with self.subTest(i=i),self.assertRaisesRegex(ValueError,'inventory'):
                    security.grade(rows)
    def test_each_forged_result_and_collateral_failure(self):
        for i in range(8):
            for mutation in ({'status':200},{'status':429},{'error':'different mechanism'},{'positive_status':403}):
                rows=complete();rows[i].update(mutation)
                with self.subTest(i=i,mutation=mutation),self.assertRaisesRegex(ValueError,'property rejected: '+rows[i]['property']):
                    security.grade(rows)
    def test_unsigned_or_missing_positive_signature(self):
        for i in (2,3,4):
            for field in ('signature_verified','positive_signature_verified'):
                rows=complete();rows[i][field]=False
                with self.assertRaisesRegex(ValueError,'signature witness'):
                    security.grade(rows)
    def test_unreplaced_or_unobserved_cookie(self):
        for i in (5,6):
            for value in ('a'*64,'','unobserved'):
                rows=complete();rows[i]['new_sha256']=value
                with self.assertRaisesRegex(ValueError,'cookie replacement'):
                    security.grade(rows)
    def test_real_expiry_requires_each_independent_witness(self):
        for mutation in ({'before_status':401},{'long_reader_status':401},{'wait_ns':3_999_999_999},
                         {'wall_after_s':103},{'wall_elapsed_ns':8_100_000_000}):
            rows=complete();rows[-1].update(mutation)
            with self.subTest(mutation=mutation),self.assertRaisesRegex(ValueError,'real expiry'):
                security.grade(rows)
    def test_fixture_base64_roundtrip_and_invalid(self):
        raw=bytes(range(256))
        self.assertEqual(security.decode(security.encode(raw)),raw)
        with self.assertRaises(ValueError):security.decode('%%%')


if __name__=='__main__':unittest.main()
