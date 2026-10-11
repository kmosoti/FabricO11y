"""Test the vendored-crate check on a synthetic archive and tree, with deliberate edits that must fail."""
import hashlib, importlib.util, io, json, tarfile, tempfile, unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location('check_vendor', Path(__file__).with_name('check-vendor.py'))
check_vendor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check_vendor)

UPSTREAM = {
    'Cargo.toml': '[package]\nname = "foo"\n\n[dependencies.paste]\nversion = "1.0"\ndefault-features = true\n',
    'Cargo.toml.orig': 'upstream manifest\n',
    'src/lib.rs': 'pub fn answer() -> u8 { 42 }\n',
}
PATCHED = UPSTREAM['Cargo.toml'].replace('version = "1.0"', 'package = "pastey"\nversion = "=0.2.3"')
PATCH = {'dependency_alias': 'paste', 'package': 'pastey', 'version': '=0.2.3'}


def build(root: Path) -> bytes:
    """Write an unmodified patched tree under root and return its upstream archive."""
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode='w:gz') as tar:
        for name, text in UPSTREAM.items():
            member = tarfile.TarInfo('foo-1.0.0/' + name)
            member.size = len(text.encode())
            tar.addfile(member, io.BytesIO(text.encode()))
    crate = {'name': 'foo', 'version': '1.0.0', 'source': 'https://example.invalid/foo.crate',
             'archive_sha256': hashlib.sha256(buffer.getvalue()).hexdigest(), 'path': 'vendor/foo',
             'modified': ['Cargo.toml'], 'added': ['FABRIC-PATCH.md'], 'patch': PATCH}
    files = {'Cargo.toml': '[patch.crates-io]\nfoo = { path = "vendor/foo" }\n',
             'vendor/provenance.json': json.dumps({'crates': [crate]})}
    files |= {'vendor/foo/' + name: text for name, text in
              {**UPSTREAM, 'Cargo.toml': PATCHED, 'FABRIC-PATCH.md': 'local patch\n'}.items()}
    for name, text in files.items():
        (root / name).parent.mkdir(parents=True, exist_ok=True)
        (root / name).write_text(text)
    return buffer.getvalue()


class VendorCheckTests(unittest.TestCase):
    def fixture(self):
        directory = tempfile.TemporaryDirectory(prefix='vendor-check-')
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        return root, build(root)

    def test_declared_patch_passes(self):
        root, archive = self.fixture()
        self.assertEqual(check_vendor.check(root, lambda url: archive), [])

    def test_every_undeclared_difference_is_reported(self):
        cases = [
            ('edited source', 'vendor/foo/src/lib.rs', 'pub fn answer() -> u8 { 43 }\n', 'src/lib.rs: differs'),
            ('extra file', 'vendor/foo/build.rs', 'fn main() {}\n', 'build.rs: not declared'),
            ('manifest beyond the alias', 'vendor/foo/Cargo.toml', PATCHED + '[dependencies.evil]\nversion = "1"\n',
             'Cargo.toml: differs'),
            ('unpatched manifest', 'vendor/foo/Cargo.toml', UPSTREAM['Cargo.toml'], 'Cargo.toml: differs'),
            ('deleted source', 'vendor/foo/src/lib.rs', None, 'src/lib.rs: missing'),
            ('deleted patch note', 'vendor/foo/FABRIC-PATCH.md', None, 'FABRIC-PATCH.md: missing'),
            ('unlisted patch path', 'Cargo.toml', '[patch.crates-io]\nfoo = { path = "elsewhere/foo" }\n', 'paths differ'),
        ]
        for label, name, text, message in cases:
            with self.subTest(label):
                root, archive = self.fixture()
                if text is None:
                    (root / name).unlink()
                else:
                    (root / name).write_text(text)
                problems = check_vendor.check(root, lambda url: archive)
                self.assertTrue(any(message in problem for problem in problems), problems)

    def test_archive_that_does_not_match_provenance_is_reported(self):
        root, archive = self.fixture()
        problems = check_vendor.check(root, lambda url: archive + b'\0')
        self.assertEqual(len(problems), 1)
        self.assertIn('archive SHA-256 differs', problems[0])

    def test_symbolic_link_in_vendored_tree_is_refused(self):
        root, archive = self.fixture()
        (root / 'vendor/foo/src/alias.rs').symlink_to('lib.rs')
        with self.assertRaisesRegex(ValueError, 'symbolic link'):
            check_vendor.check(root, lambda url: archive)


if __name__ == '__main__':
    unittest.main()
