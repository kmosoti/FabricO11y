"""Regression checks for explicit, mounted Cargo/rustup home validation."""
import os
from pathlib import Path
import tempfile
import unittest

from rust_preflight import homes, require_limits


class HomeTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(
            prefix='rust-preflight-', dir=os.environ['FABRIC_SCRATCH_ROOT'])
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.cache = self.root / 'toolchain-cache'
        self.cargo = self.cache / 'cargo'
        self.rustup = self.cache / 'rustup'
        self.cargo.mkdir(parents=True)
        self.rustup.mkdir()
        self.environment = {'CARGO_HOME': str(self.cargo), 'RUSTUP_HOME': str(self.rustup)}

    def test_explicit_distinct_mounted_homes(self):
        self.assertEqual(homes(self.environment, self.root), self.environment)

    def test_missing_relative_and_parent_homes_are_rejected(self):
        for value in ('', 'relative', str(self.cache), str(self.root)):
            with self.subTest(value=value):
                with self.assertRaises(ValueError):
                    homes({**self.environment, 'CARGO_HOME': value}, self.root)

    def test_same_home_is_rejected(self):
        with self.assertRaises(ValueError):
            homes({**self.environment, 'CARGO_HOME': str(self.rustup)}, self.root)

    def test_symlink_escape_is_rejected(self):
        outside = self.root / 'outside'
        outside.mkdir()
        link = self.cache / 'linked'
        link.symlink_to(outside, target_is_directory=True)
        with self.assertRaises(ValueError):
            homes({**self.environment, 'CARGO_HOME': str(link)}, self.root)

    def test_sibling_prefix_is_not_descendant(self):
        sibling = self.root / 'toolchain-cache-other'
        sibling.mkdir()
        with self.assertRaises(ValueError):
            homes({**self.environment, 'CARGO_HOME': str(sibling)}, self.root)

    def test_diagnostics_do_not_copy_unrelated_environment(self):
        result = homes({**self.environment, 'SECRET_SENTINEL': 'must-not-be-recorded'}, self.root)
        self.assertNotIn('SECRET_SENTINEL', result)
        self.assertEqual(set(result), {'CARGO_HOME', 'RUSTUP_HOME'})


if __name__ == '__main__':
    require_limits()
    unittest.main()
