"""Literal namespace counts and the fast04 disappearing-directory trace."""
from pathlib import Path
import errno
import os
import tempfile
import unittest
from unittest.mock import patch

import storage_census


class Census(unittest.TestCase):
    def test_complete_count_includes_links_without_following_them(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            (root / 'payload').write_bytes(b'123456789')
            child = root / 'child'
            child.mkdir()
            (child / 'other').write_bytes(b'abc')
            (root / 'file-link').symlink_to('payload')
            (root / 'dir-link').symlink_to('child', target_is_directory=True)
            value = storage_census.scan(root)
            self.assertEqual(value['data_bytes'], 12 + len(b'payload') + len(b'child'))
            self.assertEqual(value['regular_path_bytes'], 12)
            self.assertEqual(value['path_count'], 3)

    def test_three_partial_disappearances_then_a_complete_scan_reset_totals(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            (root / 'payload').write_bytes(b'123456789')
            attempts = []

            def disappearing(*args, **kwargs):
                attempts.append(True)
                yield str(root), [], ['payload']
                if len(attempts) <= 3:
                    raise FileNotFoundError('fast04 storage directory disappeared')

            with patch.object(storage_census.os, 'walk', side_effect=disappearing), \
                 patch.object(storage_census.time, 'sleep'):
                value = storage_census.scan(root)
            self.assertEqual(value['attempt'], 4)
            self.assertEqual(value['data_bytes'], 9)
            self.assertEqual(value['path_count'], 1)

    def test_unreadable_or_other_io_error_is_never_retried_or_omitted(self):
        with tempfile.TemporaryDirectory() as name:
            for error in (PermissionError(errno.EACCES, 'unreadable'), OSError(errno.EIO, 'I/O')):
                with patch.object(storage_census.os, 'walk', side_effect=error) as walk:
                    with self.assertRaises(type(error)):
                        storage_census.scan(Path(name))
                    self.assertEqual(walk.call_count, 1)

    def test_expired_race_horizon_refuses_instead_of_returning_a_partial_sum(self):
        with tempfile.TemporaryDirectory() as name:
            with patch.object(storage_census.os, 'walk', side_effect=FileNotFoundError('gone')):
                with self.assertRaises(FileNotFoundError):
                    storage_census.scan(Path(name), retry_seconds=0)
            for value in (True, -1, 19, float('inf'), float('nan')):
                with self.assertRaises(ValueError):
                    storage_census.scan(Path(name), retry_seconds=value)


if __name__ == '__main__':
    unittest.main()
