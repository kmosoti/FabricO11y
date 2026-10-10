"""Continuation cannot promote unrelated failures or changed historical inputs."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('continuation', Path(__file__).with_name('continue-qemu.py'))
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)


class ContinuationProvenanceTests(unittest.TestCase):
    def test_only_bound_reboot_observer_failure_is_eligible(self):
        with tempfile.TemporaryDirectory() as scratch:
            prior = Path(scratch)
            (prior / 'sources').mkdir()
            for name in ('candidate', 'predecessor', 'sources/lifecycle.sh'):
                (prior / name).write_text(name)
            (prior / 'acceptance.txt').write_text('ACCEPT A1 PASS\nACCEPT L1 PASS\n')
            receipt = {'acceptance_exit': 2, 'guest_acceptance_exit': 0,
                       'error': 'RuntimeError: guest did not complete a distinct reboot',
                       'package_family': 'fedora', 'mutation': None,
                       'qemu_process_cleanup_confirmed': True,
                       'package': str(prior / 'candidate'),
                       'upgrade_from': str(prior / 'predecessor'),
                       'package_sha256': M.Q.digest(prior / 'candidate', 'sha256'),
                       'upgrade_from_sha256': M.Q.digest(prior / 'predecessor', 'sha256'),
                       'lifecycle_sha256': M.Q.digest(prior / 'sources/lifecycle.sh', 'sha256')}
            def store(value):
                (prior / 'receipt.json').write_text(json.dumps(value))
            store(receipt)
            self.assertEqual(M.validate_prior(prior), receipt)
            for key, changed in [('error', 'package installation failed'), ('acceptance_exit', 0),
                                 ('guest_acceptance_exit', 1), ('mutation', 'root-user'),
                                 ('qemu_process_cleanup_confirmed', False)]:
                store({**receipt, key: changed})
                with self.subTest(key=key), self.assertRaises(ValueError):
                    M.validate_prior(prior)
            store(receipt)
            for name in ('candidate', 'predecessor', 'sources/lifecycle.sh'):
                original = (prior / name).read_text()
                (prior / name).write_text('substituted')
                with self.subTest(name=name), self.assertRaises(ValueError):
                    M.validate_prior(prior)
                (prior / name).write_text(original)
            (prior / 'acceptance.txt').write_text('ACCEPT L1 PASS\nACCEPT A3 FAIL\n')
            with self.assertRaises(ValueError):
                M.validate_prior(prior)


if __name__ == '__main__':
    unittest.main()
