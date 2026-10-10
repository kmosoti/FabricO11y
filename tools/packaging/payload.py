"""Bounded exact regular-file inventories for candidate Deb/RPM equivalence."""
import hashlib
import io
import os
from pathlib import PurePosixPath
import stat
import select
import subprocess
import tarfile
import time

CAP = 128 * 1024**2


def bounded_command(command):
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    deadline = time.monotonic() + 30
    result = bytearray()
    try:
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise ValueError('payload extraction deadline exceeded')
            ready, _, _ = select.select([process.stdout], [], [], min(1, remaining))
            if not ready:
                continue
            chunk = os.read(process.stdout.fileno(), 65536)
            if not chunk:
                break
            if len(result) + len(chunk) > CAP:
                raise ValueError('payload extraction exceeds bounded inventory cap')
            result.extend(chunk)
        if process.wait(timeout=max(0.01, deadline - time.monotonic())):
            raise ValueError('payload extraction failed')
        return bytes(result)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=10)
        process.stdout.close()


def identity(body):
    return {'bytes': len(body), 'sha256': hashlib.sha256(body).hexdigest()}


def safe(name):
    path = PurePosixPath(name.removeprefix('./'))
    if path.is_absolute() or '..' in path.parts or not path.parts or path.parts[0] != 'usr':
        raise ValueError('unsafe candidate payload path: ' + name)
    return str(path)


def cpio(raw):
    if len(raw) > CAP:
        raise ValueError('RPM payload exceeds bounded inventory cap')
    result = {}
    offset = 0
    while offset + 110 <= len(raw):
        if raw[offset:offset+6] != b'070701':
            raise ValueError('unsupported RPM cpio encoding')
        fields = [int(raw[offset+6+i*8:offset+14+i*8], 16) for i in range(13)]
        mode, size, namesize = fields[1], fields[6], fields[11]
        start = offset + 110
        name_raw = raw[start:start+namesize]
        if not namesize or len(name_raw) != namesize or name_raw[-1] != 0:
            raise ValueError('truncated RPM cpio name')
        name = name_raw[:-1].decode()
        content_at = (start + namesize + 3) & ~3
        body = raw[content_at:content_at+size]
        if len(body) != size:
            raise ValueError('truncated RPM cpio body')
        offset = (content_at + size + 3) & ~3
        if name == 'TRAILER!!!':
            return result
        if stat.S_ISDIR(mode):
            continue
        name = safe(name)
        if not stat.S_ISREG(mode) or name in result:
            raise ValueError('nonregular or duplicate RPM payload')
        result[name] = identity(body)
    raise ValueError('RPM cpio trailer missing')


def inventory(package):
    if package.suffix == '.rpm':
        raw = bounded_command(['rpm2cpio', str(package)])
        return cpio(raw)
    raw = bounded_command(['ar', 'p', str(package), 'data.tar.xz'])
    if len(raw) > CAP:
        raise ValueError('Debian compressed payload exceeds cap')
    result = {}
    size = 0
    with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
        for member in archive:
            if member.isdir():
                continue
            name = safe(member.name)
            size += member.size
            if not member.isfile() or member.size > 32 * 1024**2 or size > CAP or name in result:
                raise ValueError('unsafe Debian payload entry')
            result[name] = identity(archive.extractfile(member).read())
    return result


def assert_equivalent(deb, rpm):
    expected = inventory(deb)
    actual = inventory(rpm)
    extra = {'usr/share/doc/fabrico11y/CANDIDATE-PROVENANCE.json'}
    if set(actual) != set(expected) | extra or any(actual[name] != value for name, value in expected.items()):
        raise ValueError('RPM must preserve every exact Debian regular payload byte')
    return expected
