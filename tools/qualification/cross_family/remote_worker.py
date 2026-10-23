"""Remote lifetime of the existing Guest, held by an owned SSH stdin pipe."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import sys

from guest import Guest, Q


def emit(event, **values):
    print(json.dumps(dict(event=event, **values)), flush=True)


def interrupted(signum, frame):
    raise InterruptedError('remote guest interrupted: ' + str(signum))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run-id', required=True)
    parser.add_argument('--family', choices=('debian', 'fedora'), required=True)
    parser.add_argument('--manifest-sha256', required=True)
    parser.add_argument('--network', choices=('192.0.2.0/24',))
    args = parser.parse_args()
    if not re.fullmatch(r'[a-z0-9][a-z0-9-]{0,79}', args.run_id):
        raise ValueError('invalid remote run identity')
    signal.signal(signal.SIGTERM, interrupted)
    from resource_group import require_limits
    group = require_limits()
    if os.environ.get('FABRIC_RESOURCE_JOB_PROFILE') != 'edge':
        raise ValueError('remote edge requires the exact registered edge profile')
    emit('started', unit=group.name.removesuffix('.service'), cgroup=str(group), run_id=args.run_id)
    manifest = Q.DATA / 'inputs/release-inputs.json'
    if (manifest.is_symlink() or manifest.stat().st_size > 2 * 1024**2
            or Q.digest(manifest, 'sha256') != args.manifest_sha256):
        raise ValueError('remote staged manifest hash changed')
    inventory = json.loads(manifest.read_text())['files']
    if type(inventory) is not dict or len(inventory) > 20000:
        raise ValueError('remote staged inventory exceeds bounds')
    for relative, expected in inventory.items():
        path = Q.DATA / relative
        if (not path.is_relative_to(Q.DATA) or '..' in path.parts
                or path.resolve() != path or path.is_symlink()
                or Q.digest(path, 'sha256') != expected):
            raise ValueError('staged immutable input changed: ' + relative)
    root = Q.DATA / 'scratch' / ('remote-' + args.run_id)
    results = Q.DATA / 'results' / ('remote-' + args.run_id)
    # Controller creates only this private input directory, including its fixture key.
    if root.is_symlink() or root.stat().st_uid != os.getuid():
        raise ValueError('remote input ownership changed')
    results.mkdir(parents=True, exist_ok=False, mode=0o700)
    key = root / 'id_ed25519'
    if key.is_symlink() or key.stat().st_mode & 0o077:
        raise ValueError('fixture key is not private')
    guest = Guest('edge', args.family, root, results, key, 1536,
                  network=args.network, acceleration='kvm')
    status = 1
    try:
        guest.start()
        guest.ready()
        emit('ready', port=guest.port, boot_id=guest.boot_id, command=guest.command)
        # No RPC framework: one bounded copy request or the explicit stop action.
        # All guest scripts use the ordinary SSH interface over the management forward.
        while True:
            line = sys.stdin.readline(8193)
            if not line:
                raise RuntimeError('controller disconnected before explicit stop')
            if len(line) > 8192:
                raise ValueError('remote lifecycle request exceeds bound')
            request = json.loads(line)
            if request == {'action': 'stop'}:
                status = 0
                break
            if set(request) != {'action', 'path', 'sha256', 'destination'} or request['action'] != 'copy':
                raise ValueError('invalid remote lifecycle action')
            path = Path(request['path'])
            destination = request['destination']
            if (not path.is_absolute() or path.is_symlink()
                    or not path.resolve().is_relative_to(Q.DATA / 'inputs')
                    or not re.fullmatch(r'/home/accept/[a-zA-Z0-9_.-]+', destination)):
                raise ValueError('remote copy is outside staged inputs or fixed guest destinations')
            if hashlib.sha256(path.read_bytes()).hexdigest() != request['sha256']:
                raise ValueError('staged remote input hash changed')
            guest.copy(path, destination)
            emit('copied', sha256=request['sha256'], destination=destination)
    finally:
        stopped = guest.stop()
        record = dict(run_id=args.run_id, boot_id=guest.boot_id, qemu_stopped=stopped,
                      exit=status, command=getattr(guest, 'command', None))
        (results / 'lifecycle.json').write_text(json.dumps(record, indent=2) + '\n')
        emit('stopped', **record)
    return status


if __name__ == '__main__':
    sys.exit(main())
