"""Preserve owner06's constructor error before looking for TLS evidence."""
import json
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch

import console_bridge
import owner_walkthrough as W


class BrowserFailure(unittest.TestCase):
    def walk(self, root):
        for name in ('scratch', 'results', 'budget'):
            (root / name).mkdir()
        (root / 'budget/cgroup.events').write_text('populated 0\n')
        walk = object.__new__(W.Walkthrough)
        walk.root, walk.out = root / 'scratch', root / 'results'
        walk.budget, walk.overall_end = {'path': root / 'budget'}, time.monotonic() + 30
        walk.browser_entry_helper, walk.browser_entry_helper_sha256 = root / 'helper', '0' * 64
        walk.record = {'events': []}
        walk.event = lambda *args, **values: walk.record['events'].append(values)
        return walk

    def test_original_constructor_error_survives_absent_receipt(self):
        class Refused:
            def __init__(self, *args, **kwargs):
                raise RuntimeError('literal original deadline refusal')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            walk = self.walk(root)
            with patch.object(console_bridge, 'ConsoleBridge', Refused):
                with self.assertRaisesRegex(RuntimeError, 'literal original deadline refusal'):
                    walk.browser('wrong-san', root / 'ca', root / 'leaf', wrong_san=True)
            records = walk.record['browser_constructor_errors']
            self.assertEqual(len(records), 1)
            observed = json.loads(Path(records[0]['path']).read_text())
            self.assertEqual(observed['message'], 'literal original deadline refusal')
            self.assertEqual(Path(records[0]['path']).stat().st_mode & 0o777, 0o600)

    def test_exact_certificate_refusal_and_cleanup_remain_required(self):
        for code in ('SSL_ERROR_BAD_CERT_DOMAIN', 'SEC_ERROR_UNKNOWN_ISSUER'):
            closed = []
            class Refused:
                def __init__(self, origin, leaf, work, out, **kwargs):
                    out.mkdir()
                    self.receipt = {'tls_navigation_failure': {
                        'uri': 'about:certerror?fixture', 'codes': [code]}}
                    (out / 'receipt.json').write_text(json.dumps(self.receipt))
                    raise RuntimeError('actual certificate navigation refused')
                def close(self):
                    closed.append(True)
            with self.subTest(code=code), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                walk = self.walk(root)
                with patch.object(console_bridge, 'ConsoleBridge', Refused):
                    if code == 'SSL_ERROR_BAD_CERT_DOMAIN':
                        walk.browser('wrong-san', root / 'ca', root / 'leaf', wrong_san=True)
                    else:
                        with self.assertRaisesRegex(ValueError, 'specific actual candidate certificate refusal missing'):
                            walk.browser('wrong-san', root / 'ca', root / 'leaf', wrong_san=True)
                self.assertEqual(closed, [True])


if __name__ == '__main__':
    unittest.main()
