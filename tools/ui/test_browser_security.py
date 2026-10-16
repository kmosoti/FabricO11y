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
    def test_key_probe_verifies_original_bytes_and_rejects_export_and_signature_defects(self):
        import hashlib,json,os,subprocess,tempfile,time
        from pathlib import Path
        # Literal P-256 scalar1 SEC1 fixture, never a candidate/browser key.
        der=bytes.fromhex('30770201010420'+'00'*31+'01'+'a00a06082a8648ce3d030107a14403420004'
            +'6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296'
            +'4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5')
        origin='https://localhost:443';client=json.dumps(dict(type='webauthn.get',origin=origin,challenge='fixture')).encode()
        auth=hashlib.sha256(b'localhost').digest()+b'\x05\x00\x00\x00\x01'
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root=Path(directory);source=root/'source.der';source.write_bytes(der);source.chmod(0o600)
            signed=subprocess.run(['openssl','dgst','-sha256','-keyform','DER','-sign',str(source)],
                input=auth+hashlib.sha256(client).digest(),capture_output=True,check=True).stdout
            original=dict(id=security.encode(b'fixture-id'),response=dict(authenticatorData=security.encode(auth),
                clientDataJSON=security.encode(client),signature=security.encode(signed)))
            for i,defect in enumerate((None,'missing','duplicate','DER','signature','origin','login')):
                work=root/str(i);work.mkdir();assertion=copy.deepcopy(original)
                keys=[dict(credentialId=original['id'],privateKey=security.encode(der))]
                if defect=='missing':keys=[]
                elif defect=='duplicate':keys*=2
                elif defect=='DER':keys[0]['privateKey']=security.encode(b'invalid DER')
                elif defect=='signature':assertion['response']['signature']=security.encode(signed[:-1]+bytes([signed[-1]^1]))
                elif defect=='origin':assertion['response']['clientDataJSON']=security.encode(client.replace(b'localhost',b'attacker'))
                def api(path,*args):return (403 if defect=='login' and path.endswith('/finish') else 200),{}
                call=lambda:security.probe_key(api=api,bearer=lambda *args:(200,dict(ceremony_id='literal-source-challenge',public_key={})),
                    async_js=lambda *args:dict(value=json.dumps(assertion)),export_credentials=lambda:keys,
                    origin=origin,principal='fixture',work=work,deadline=time.monotonic()+10)
                if defect:
                    with self.subTest(defect=defect),self.assertRaises(ValueError):call()
                else:
                    row=call();self.assertTrue(row['browser_signature_verified']);self.assertEqual(row['private_bytes'],len(der))
                    self.assertEqual(set(row),{'server_login_status','matched_credentials','export_format','private_bytes',
                        'public_key_sha256','browser_signature_bytes','browser_signature_verified'})

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
