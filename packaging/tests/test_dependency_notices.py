import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location('notices', Path(__file__).parents[1] / 'dependency-notices.py')
notices = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notices)

SCRATCH = Path(os.environ['TMPDIR'])
if not SCRATCH.is_absolute() or any(
    SCRATCH.resolve(strict=True).is_relative_to(path)
    for path in (Path('/tmp'), Path('/dev/shm'))
):
    raise ValueError('TMPDIR must be a disk-backed directory')


class InventoryTests(unittest.TestCase):
    def test_missing_text_fails_before_output(self):
        with tempfile.TemporaryDirectory(dir=SCRATCH) as root:
            root = Path(root)
            package = {'id': 'a', 'name': 'a', 'version': '1.0.0', 'manifest_path': str(root / 'Cargo.toml'), 'license': 'MIT', 'license_file': None, 'source': 'registry'}
            with self.assertRaisesRegex(ValueError, 'a 1.0.0'):
                notices.collect({'workspace_members': [], 'packages': [package]}, 'a v1.0.0', root / 'out')
            self.assertFalse((root / 'out').exists())
            (root / 'LICENSE').write_text('permission text')
            notices.collect({'workspace_members': [], 'packages': [package]}, 'a v1.0.0', root / 'out')
            self.assertEqual((root / 'out/a-1.0.0/LICENSE').read_text(), 'permission text')
            self.assertEqual(json.loads((root / 'out/inventory.json').read_text())['packages'][0]['license'], 'MIT')

    def test_supplement_requires_published_vcs_commit(self):
        with tempfile.TemporaryDirectory(dir=SCRATCH) as root:
            root = Path(root)
            package = {'id': 'alloc', 'name': 'alloc-stdlib', 'version': '0.2.4', 'manifest_path': str(root / 'Cargo.toml'), 'license': 'BSD-3-Clause', 'license_file': None, 'source': 'registry'}
            metadata = {'workspace_members': [], 'packages': [package]}
            vcs = root / '.cargo_vcs_info.json'
            vcs.write_text(json.dumps({'git': {'sha1': 'wrong'}}))
            with self.assertRaisesRegex(ValueError, 'invalid supplemental license provenance'):
                notices.collect(metadata, 'alloc-stdlib v0.2.4', root / 'out')
            vcs.write_text(json.dumps({'git': {'sha1': 'ae42d22078b98549e987d2f03d12df7b984fde47'}}))
            notices.collect(metadata, 'alloc-stdlib v0.2.4', root / 'out')
            self.assertTrue((root / 'out/alloc-stdlib-0.2.4/LICENSE').is_file())

    def test_unmatched_tree_package_fails(self):
        with tempfile.TemporaryDirectory(dir=SCRATCH) as root:
            with self.assertRaisesRegex(ValueError, 'missing from metadata'):
                notices.collect({'workspace_members': [], 'packages': []}, 'unknown v1.0.0', Path(root) / 'out')


if __name__ == '__main__':
    unittest.main()
