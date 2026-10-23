"""Existing strict namespace sum, with bounded full retries for disappearing paths."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import stat
import time


def scan(root: Path, retry_seconds=18):
    if type(retry_seconds) not in (int, float) or not 0 <= retry_seconds <= 18:
        raise ValueError('namespace retry horizon must remain within 18 seconds')
    if not root.is_absolute() or root.resolve() != root or not root.is_dir():
        raise ValueError('explicit canonical storage directory required')
    started = time.monotonic()
    deadline = started + retry_seconds
    attempt = 0

    def fail(error):
        raise error

    while True:
        attempt += 1
        total = regular = count = 0
        try:
            for base, directories, files in os.walk(root, followlinks=False, onerror=fail):
                for name in directories:
                    path = Path(base) / name
                    if path.is_symlink():
                        total += path.lstat().st_size
                for name in files:
                    info = (Path(base) / name).lstat()
                    total += info.st_size
                    count += 1
                    if stat.S_ISREG(info.st_mode):
                        regular += info.st_size
            return dict(data_bytes=total, regular_path_bytes=regular, path_count=count,
                        uid=os.getuid(), gid=os.getgid(), attempt=attempt,
                        elapsed_seconds=time.monotonic() - started)
        except FileNotFoundError:
            if time.monotonic() >= deadline:
                raise
            time.sleep(.1)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(scan(args.root)))
