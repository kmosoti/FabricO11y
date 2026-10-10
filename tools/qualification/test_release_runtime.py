import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

import release_runtime as runtime


class ArchiveBounds(unittest.TestCase):
    def fixture(self, root):
        f = runtime.CandidateFixture.__new__(runtime.CandidateFixture)
        f.out, f.work = root / 'results/cell', root / 'scratch/cell'
        for directory in (f.out, f.work, root / 'evidence'):
            directory.mkdir(parents=True)
        f.extra_archives, f.receipt = [], {}
        return f

    def test_archive_is_fresh_and_reused_only_by_its_owner(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root = Path(directory)
            f = self.fixture(root)
            with patch.object(runtime, 'STORAGE', root):
                archive = f.archive_dir()
                self.assertEqual(f.archive_dir(), archive)
                self.assertEqual(archive.stat().st_mode & 0o777, 0o700)
                f.extra_archives = []
                with self.assertRaises(FileExistsError):
                    f.archive_dir()

    def test_symlink_archive_root_is_rejected(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root = Path(directory)
            f = self.fixture(root)
            (root / 'evidence').rmdir()
            (root / 'evidence').symlink_to(f.work, target_is_directory=True)
            with patch.object(runtime, 'STORAGE', root), self.assertRaises(RuntimeError):
                f.archive_dir()

    def test_source_archives_are_in_the_same_live_disk_budget(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root = Path(directory)
            f = self.fixture(root)
            with patch.object(runtime, 'STORAGE', root):
                archive = f.archive_dir()
            # Sparse allocation keeps the control cheap. The logical-byte
            # guard must reject this even when filesystem charge is small.
            with (archive / 'too-large').open('wb') as output:
                output.truncate(5 * 1024**3 + 1)
            with self.assertRaisesRegex(RuntimeError, 'live fixture disk'):
                f.resource_sample()

    def test_external_browser_temporary_is_counted_without_double_profile(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root=Path(directory);f=self.fixture(root)
            profile=f.work/'browser';profile.mkdir()
            temporary=root/'cb-owned';temporary.mkdir()
            (profile/'profile').write_bytes(b'P'*7)
            (temporary/'temporary').write_bytes(b'T'*11)
            f.bridge=SimpleNamespace(work=profile,browser_temporary=temporary)
            sample=f.storage_sample()
            self.assertEqual(sample['live_bytes'],18)
            self.assertEqual(sample['external_browser_temporary_bytes'],11)
            self.assertEqual(sample['browser_profile_and_temporary_bytes'],18)

    def test_external_temporary_cannot_escape_browser_or_total_budget(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root=Path(directory);f=self.fixture(root)
            temporary=root/'cb-owned';temporary.mkdir()
            f.bridge=SimpleNamespace(work=f.work,browser_temporary=temporary)
            with (temporary/'oversize').open('wb') as output:
                output.truncate(512*1024**2+1)
            with self.assertRaisesRegex(RuntimeError,'profile and temporary'):
                f.storage_sample()
            (temporary/'oversize').unlink()
            (temporary/'one').write_bytes(b'T')
            # The total cap must include a single external byte at its boundary.
            f.bridge.work=f.work/'empty-profile';f.bridge.work.mkdir()
            with (f.work/'fixture').open('wb') as output:
                output.truncate(5*1024**3)
            with self.assertRaisesRegex(RuntimeError,'live fixture disk'):
                f.storage_sample()


if __name__ == '__main__':
    unittest.main()
