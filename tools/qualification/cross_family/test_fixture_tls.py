"""Smoke02 CA extension counterexample, checked by strict OpenSSL verification."""
from pathlib import Path
import subprocess
import tempfile
import unittest

import run as campaign


class FixtureTLS(unittest.TestCase):
    def test_fixture_chain_is_strict_and_missing_ca_usage_is_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            campaign.fixture_tls(root)
            verify = ['openssl', 'verify', '-x509_strict', '-purpose', 'sslserver',
                      '-verify_ip', '127.0.0.1', '-CAfile', str(root / 'ca.pem'),
                      str(root / 'server.pem')]
            positive = subprocess.run(verify, text=True, capture_output=True)
            self.assertEqual(positive.returncode, 0, positive.stderr)
            # Reproduce the retained CA defect independently of the producer.
            subprocess.run(['openssl', 'req', '-x509', '-new', '-key', str(root / 'ca.key'),
                '-config', '/dev/null', '-subj', '/CN=cross-family-owned-ca',
                '-addext', 'basicConstraints=critical,CA:TRUE', '-days', '2',
                '-out', str(root / 'bad-ca.pem')], check=True, capture_output=True)
            negative = subprocess.run([x.replace('/ca.pem', '/bad-ca.pem') for x in verify],
                                      text=True, capture_output=True)
            self.assertNotEqual(negative.returncode, 0)
            self.assertIn('CA cert does not include key usage extension', negative.stderr)


if __name__ == '__main__':
    unittest.main()
