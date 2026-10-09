#!/usr/bin/env python3
"""Pinned standalone RowSet screen; all networking/builds/runs are launcher-only."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import select
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib

import memory_census as mc

PIN = '0439d576fb3d36acba4444f5a5bc0d4173ecd932'
PACKAGES = {
    'roaring': (f'https://codeload.github.com/RoaringBitmap/roaring-rs/tar.gz/{PIN}',
                '7239e8bec111af1f49bcc736628966622a7bb5103a9bfb71a45714f47aa48e36', '0.11.5'),
    'bytemuck': ('https://static.crates.io/crates/bytemuck/bytemuck-1.25.2.crate',
                '95832e849adfb21180ccb6826a99da14e5d266ae5c2e668e1602cf234f153797', '1.25.2'),
    'byteorder': ('https://static.crates.io/crates/byteorder/byteorder-1.5.0.crate',
                 '1fd0f2584146f6f2ef48085050886acf353beff7305ebd1ae69500e27c67f64b', '1.5.0'),
}
REGISTERED = mc.ROOT / 'docs/experiments/benchmarks/data/cross-system-sweep-01/query'
SOURCE = Path(__file__).with_name('rowset_probe.rs')
SEEDS = (2703204353, 2703204354)
SHAPES = ('sparse', 'spread', 'clustered', 'common')
ARMS = ('vector', 'dense', 'roaring')
LICENSE_ALIASES = {'roaring/LICENSE-APACHE': '../LICENSE-APACHE',
                   'roaring/LICENSE-MIT': '../LICENSE-MIT'}


def known_license_alias(package, key, linkname):
    if package != 'roaring' or key not in LICENSE_ALIASES or LICENSE_ALIASES[key] != linkname:
        raise ValueError('unregistered license alias/link rejected')
    return Path(key).name


def verified_aliases(links, members):
    result = {}
    for key, link in links.items():
        target = link['target']
        if target not in members:
            raise ValueError('known license alias lacks regular root license target')
        result[key] = {**link, 'target_regular_member': members[target], 'extracted': False}
    return result


def bytes32(values):
    return b''.join(struct.pack('<I', value) for value in values)


def fixture(n, shape, seed, operand):
    # Pure Python defines membership. No native generator or representation code.
    shift = (seed + operand*97) % n
    if shape == 'clustered':
        return {value for value in range(n) if (value-shift) % 4096 < 512}
    modulus = {'sparse': 1024, 'spread': 16, 'common': 4}[shape]
    threshold = 3 if shape == 'common' else 1
    def mix(value):
        value = (value ^ (value >> 16)) * 0x7feb352d & 0xffffffff
        value = (value ^ (value >> 15)) * 0x846ca68b & 0xffffffff
        return value ^ (value >> 16)
    def included(value):
        primary = mix(value ^ seed)
        if operand == 0:
            return primary % modulus < threshold
        secondary = mix(value ^ (seed+97))
        # Half the buckets share A; half independently sample the same density.
        return (primary if secondary & 1 == 0 else mix(value ^ (seed+193))) % modulus < threshold
    return {value for value in range(n) if included(value)}


def exact(payload, expected):
    if payload != bytes32(expected):
        raise ValueError('exact membership/order differs from independent Python sets')


def controls():
    expected = [1, 4, 9]
    exact(bytes32(expected), expected)
    rejected = []
    for name, values in [('missing', [1, 9]), ('extra', [1, 4, 7, 9]), ('order', [9, 4, 1])]:
        try:
            exact(bytes32(values), expected)
        except ValueError:
            rejected.append(name)
        else:
            raise AssertionError('membership control accepted: ' + name)
    target = known_license_alias('roaring', 'roaring/LICENSE-MIT', '../LICENSE-MIT')
    links = {'roaring/LICENSE-MIT': {'target': target, 'raw_linkname': '../LICENSE-MIT'}}
    verified_aliases(links, {'LICENSE-MIT': {'bytes': 7, 'sha256': 'control'}})
    alias_rejected = []
    cases = {'wrong_link': lambda: known_license_alias('roaring','roaring/LICENSE-MIT','../wrong'),
             'wrong_path': lambda: known_license_alias('roaring','roaring/src/LICENSE-MIT','../LICENSE-MIT'),
             'absolute_link': lambda: known_license_alias('roaring','roaring/LICENSE-MIT','/LICENSE-MIT'),
             'wrong_package': lambda: known_license_alias('bytemuck','roaring/LICENSE-MIT','../LICENSE-MIT'),
             'absent_target': lambda: verified_aliases(links,{})}
    for name, mutation in cases.items():
        try:
            mutation()
        except ValueError:
            alias_rejected.append(name)
        else:
            raise AssertionError('license alias mutation accepted: '+name)
    return {'valid_accepted': True, 'rejected': rejected,
            'known_license_alias_target_validated': True, 'license_alias_controls_rejected': alias_rejected}


def footprint(root):
    total = 0
    for path in root.rglob('*'):
        if path.is_symlink():
            raise RuntimeError('linked owned artifact')
        if path.is_file():
            info = path.stat()
            total += max(info.st_size, info.st_blocks*512)
    return total


def guard(work, destination, deadline):
    if time.monotonic() >= deadline:
        raise RuntimeError('540-second inner deadline')
    if footprint(work) > 512*mc.MIB or footprint(destination) > 64*mc.MIB:
        raise RuntimeError('512MiB scratch/64MiB evidence limit')
    if shutil.disk_usage(work).free < mc.RESERVE:
        raise RuntimeError('16GiB data-drive free reserve unavailable')


def retrieve(name, work, destination, deadline):
    url, sha, version = PACKAGES[name]
    archive = destination / (name + '.tar.gz')
    command = ['curl', '--fail', '--location', '--proto', '=https', '--proto-redir', '=https',
               '--silent', '--show-error', '--max-time', '45', '--connect-timeout', '15', url]
    record = {'url': url, 'expected_sha256': sha, 'version': version, 'argv': command,
              'compressed_limit_bytes': 2*mc.MIB, 'decoded_limit_bytes': 16*mc.MIB}
    total = 0
    child = None
    started = time.monotonic()
    try:
        with archive.open('xb') as out, (destination / (name+'-download.log')).open('wb') as err:
            child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=err)
            while True:
                if time.monotonic() >= min(deadline, started+45):
                    raise TimeoutError('bounded source download deadline')
                ready, _, _ = select.select([child.stdout], [], [], .2)
                if not ready:
                    continue
                chunk = os.read(child.stdout.fileno(), mc.MIB)
                if not chunk:
                    break
                if total+len(chunk) > 2*mc.MIB:
                    available = 2*mc.MIB-total
                    out.write(chunk[:available])
                    total += available
                    raise RuntimeError('2MiB archive prefix limit')
                out.write(chunk)
                total += len(chunk)
            child.wait(timeout=max(.001, deadline-time.monotonic()))
            if child.returncode:
                raise RuntimeError('curl exit '+str(child.returncode))
        if mc.digest(archive) != sha:
            raise ValueError('immutable source package SHA differs')
        root = work / name
        root.mkdir()
        members = {}
        skipped_links = {}
        decoded = 0
        with gzip.open(archive, 'rb') as zipped:
            class Reader:
                def read(self, count):
                    nonlocal decoded
                    block = zipped.read(count)
                    decoded += len(block)
                    if decoded > 16*mc.MIB or time.monotonic() >= deadline:
                        raise RuntimeError('decoded package/deadline ceiling')
                    return block
            reader = Reader()
            with tarfile.open(fileobj=reader, mode='r|') as stream:
                prefix = None
                for member in stream:
                    path = PurePosixPath(member.name)
                    if path.is_absolute() or '..' in path.parts or str(path) != member.name:
                        raise ValueError('unsafe/noncanonical package path')
                    if prefix is None:
                        prefix = path.parts[0]
                    if path.parts[0] != prefix:
                        raise ValueError('mixed package roots')
                    if member.isdir():
                        continue
                    if len(path.parts) < 2:
                        raise ValueError('package links/nonregular member rejected')
                    key = str(PurePosixPath(*path.parts[1:]))
                    if key in members or key in skipped_links:
                        raise ValueError('duplicate package member')
                    if member.issym():
                        skipped_links[key] = {'target': known_license_alias(name,key,member.linkname),
                                              'raw_linkname': member.linkname}
                        continue
                    if not member.isfile():
                        raise ValueError('package links/nonregular member rejected')
                    target = root / key
                    target.parent.mkdir(parents=True, exist_ok=True)
                    content = stream.extractfile(member).read()
                    target.write_bytes(content)
                    members[key] = {'bytes': len(content), 'sha256': mc.digest(target)}
            while reader.read(mc.MIB):
                pass
        licenses = [key for key in members if Path(key).name.upper().startswith(('LICENSE', 'COPYING'))]
        if not licenses:
            raise ValueError('license source absent')
        record.update(archive_sha256=mc.digest(archive), members=members, license_paths=licenses,
                      known_license_links_skipped=verified_aliases(skipped_links,members),
                      decompressed_bytes=decoded, extraction_root=str(root), source_code_changed=False)
        return root
    finally:
        if child is not None and child.poll() is None:
            child.kill()
            child.wait()
        record.update(received_bytes=total, exit_code=child.returncode if child else None,
                      archive_sha256=mc.digest(archive) if archive.exists() else None)
        mc.dump(destination / (name+'-source.json'), record)


def command(argv, work, destination, name, deadline, env=None):
    started = time.monotonic()
    with (destination / (name+'.log')).open('wb') as log:
        result = subprocess.run(argv, cwd=work, env=env, stdout=log, stderr=subprocess.STDOUT,
                                timeout=min(90, max(.001, deadline-started)))
    mc.dump(destination / (name+'.json'), {'argv': argv, 'exit_code': result.returncode,
            'package_env': {key: env.get(key) for key in ('CARGO_PKG_NAME', 'CARGO_PKG_VERSION', 'CARGO_MANIFEST_DIR')}
                           if env else {},
            'elapsed_seconds': time.monotonic()-started})
    result.check_returncode()


def preserve(work, destination, error):
    record = {'error': repr(error), 'retained_scratch': str(work), 'scratch_removed': False}
    try:
        archive = destination / 'failed-state.tar.gz'
        limit = min(32*mc.MIB, 64*mc.MIB-footprint(destination)-mc.MIB)
        if limit <= 0:
            raise RuntimeError('failure archive cannot fit; scratch retained')
        members = mc.regular_members(work)
        with archive.open('xb') as out:
            with tarfile.open(fileobj=mc.ArchiveWriter(out, limit), mode='w:gz', compresslevel=1) as stream:
                stream.add(work, arcname=work.name)
        # Every original member payload is read back before any successful cleanup.
        observed = {}
        with tarfile.open(archive, 'r:gz') as stream:
            for member in stream:
                if member.isfile():
                    key = str(Path(member.name).relative_to(work.name))
                    if key in observed:
                        raise ValueError('duplicate failure archive member')
                    content = stream.extractfile(member).read()
                    observed[key] = {'bytes': len(content), 'sha256': hashlib.sha256(content).hexdigest()}
        mc.equal_member_maps(members, observed)
        mc.equal_member_maps(members, mc.regular_members(work))
        record.update(complete_archive_sha256=mc.digest(archive), complete_archive_bytes=archive.stat().st_size,
                      payload_readback=True)
        shutil.rmtree(work)
        record['scratch_removed'] = not work.exists()
    except BaseException as preservation_error:
        record['preservation_error'] = repr(preservation_error)
        archive = destination / 'failed-state.tar.gz'
        if archive.exists():
            record.update(partial_archive_bytes=archive.stat().st_size, partial_archive_sha256=mc.digest(archive))
    mc.dump(destination / 'failure.json', record)


def main():
    mc.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--protocol', type=Path, required=True)
    args = parser.parse_args()
    destination = args.destination.resolve()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if destination.exists() or not destination.is_relative_to(REGISTERED.resolve()) or not scratch.is_relative_to(mc.DATA.resolve()):
        raise ValueError('fresh registered evidence/data-drive scratch required')
    destination.mkdir(parents=True)
    mc.dump(destination / 'negative-controls.json', controls())
    for path in (Path(__file__), SOURCE, args.protocol):
        shutil.copyfile(path, destination / (path.name if path.suffix not in ['.md'] else 'protocol.txt'))
    deadline = time.monotonic()+540
    work = Path(tempfile.mkdtemp(prefix='rowset-sweep-', dir=scratch))
    try:
        packages = {name: retrieve(name, work, destination, deadline) for name in PACKAGES}
        env = dict(os.environ)
        for name in ('RUSTFLAGS', 'RUSTC_BOOTSTRAP'):
            env.pop(name, None)
        command(['rustc', '-vV'], work, destination, 'rustc-version', deadline, env)
        common = ['rustc', '-C', 'opt-level=3', '-C', 'codegen-units=1']
        package_settings = {}
        for name in ('bytemuck', 'byteorder', 'roaring'):
            source = packages[name] / ('roaring/src/lib.rs' if name == 'roaring' else 'src/lib.rs')
            manifest_path = packages[name] / ('roaring/Cargo.toml' if name == 'roaring' else 'Cargo.toml')
            manifest = tomllib.loads(manifest_path.read_text())
            package = manifest['package']
            edition = package.get('edition', '2015')
            if edition not in ('2015', '2018', '2021', '2024') or package['version'] != PACKAGES[name][2]:
                raise ValueError('unknown/inherited edition or package version mismatch')
            wanted_defaults = [] if name == 'bytemuck' else ['std']
            if manifest.get('features', {}).get('default', []) != wanted_defaults:
                raise ValueError('actual default features differ from registered closure')
            package_settings[name] = {'edition': edition, 'version': package['version'],
                    'manifest_sha256': mc.digest(manifest_path), 'features_enabled': wanted_defaults}
            package_env = dict(env, CARGO_PKG_NAME=name, CARGO_PKG_VERSION=package['version'],
                               CARGO_MANIFEST_DIR=str(manifest_path.parent))
            argv = common + ['--edition='+edition, '--crate-name', name, '--crate-type', 'rlib', str(source), '-o', str(work / ('lib'+name+'.rlib'))]
            if name in ('byteorder', 'roaring'):
                argv += ['--cfg', 'feature="std"']
            if name == 'roaring':
                for dep in ('bytemuck', 'byteorder'):
                    argv += ['--extern', dep+'='+str(work / ('lib'+dep+'.rlib'))]
            command(argv, work, destination, 'build-'+name, deadline, package_env)
        binary = work / 'rowset-probe'
        command(common + ['--edition=2021', str(SOURCE), '--extern', 'roaring='+str(work/'libroaring.rlib'),
                         '-L', 'dependency='+str(work), '-o', str(binary)], work, destination, 'build-probe', deadline, env)
        mc.dump(destination / 'metadata.json', {'pin': PIN, 'packages': PACKAGES, 'binary_sha256': mc.digest(binary),
                'package_build_settings': package_settings,
                'probe_sha256': mc.digest(SOURCE), 'runner_sha256': mc.digest(Path(__file__)),
                'protocol_sha256': mc.digest(args.protocol), 'scratch': str(work),
                'qualification': False, 'phase_memory': 'requested/live System allocator; not reservation',
                'rss': 'native /proc/self/status postexec HWM; wait4 may include Python parent floor',
                'roaring_build': 'from_sorted_iter + optimize, optimization cost included',
                'upstream_build_scripts_executed': False, 'cargo_workspace_modified': False})
        records = []
        for n in (32768, 262144):
            for shape_index, shape in enumerate(SHAPES):
                for repeat, seed in enumerate(SEEDS):
                    guard(work, destination, deadline)
                    fixture_dir = destination / f'n-{n}-{shape}-seed-{seed}'
                    fixture_dir.mkdir()
                    aset, bset = fixture(n, shape, seed, 0), fixture(n, shape, seed, 1)
                    av, bv = sorted(aset), sorted(bset)
                    expected = {'and.u32': sorted(aset & bset), 'or.u32': sorted(aset | bset),
                                'top64-desc.u32': sorted(aset & bset, reverse=True)[:64]}
                    for name, values in [('a.u32', av), ('b.u32', bv)]:
                        (fixture_dir / name).write_bytes(bytes32(values))
                    order = list(ARMS)
                    shift = (shape_index+repeat)%3
                    order = order[shift:]+order[:shift]
                    for arm in order:
                        guard(work, destination, deadline)
                        cell = fixture_dir / arm
                        cell.mkdir()
                        output = work / ('out-'+arm)
                        argv = [str(binary), str(n), arm, str(fixture_dir/'a.u32'), str(fixture_dir/'b.u32'), str(output)]
                        started = time.monotonic()
                        with (cell/'stdout.json').open('wb') as stdout, (cell/'stderr.txt').open('wb') as stderr:
                            child = subprocess.Popen(argv, stdout=stdout, stderr=stderr)
                            while True:
                                pid, status, usage = os.wait4(child.pid, os.WNOHANG)
                                if pid:
                                    child.returncode = os.waitstatus_to_exitcode(status)
                                    break
                                try:
                                    guard(work, destination, min(deadline, started+20))
                                except BaseException:
                                    child.kill(); os.wait4(child.pid, 0); raise
                                time.sleep(.01)
                        receipt = {'argv': argv, 'exit_code': child.returncode, 'wall_seconds': time.monotonic()-started,
                                   'user_seconds': usage.ru_utime, 'system_seconds': usage.ru_stime,
                                   'wait4_rss_kib': usage.ru_maxrss}
                        mc.dump(cell/'process.json', receipt)
                        if child.returncode:
                            raise RuntimeError('native cell exit '+str(child.returncode))
                        for name, values in expected.items():
                            exact((output/name).read_bytes(), values)
                        members = mc.regular_members(output)
                        mc.dump(cell/'members.json', members)
                        archive = cell/'exact-output.tar.gz'
                        with tarfile.open(archive,'w:gz',compresslevel=1) as stream:
                            stream.add(output,arcname='output')
                        with tarfile.open(archive,'r:gz') as stream:
                            observed = {}
                            for member in stream:
                                if member.isfile():
                                    key = str(Path(member.name).relative_to('output'))
                                    content = stream.extractfile(member).read()
                                    observed[key] = {'bytes':len(content),'sha256':hashlib.sha256(content).hexdigest()}
                        mc.equal_member_maps(members, observed)
                        metrics=json.loads((cell/'stdout.json').read_text())
                        records.append({'universe':n,'shape':shape,'seed':seed,'repeat':repeat,'arm':arm,
                                        'order':order,'metrics':metrics,'process':receipt,'oracle_exact':True,
                                        'archive_sha256':mc.digest(archive)})
                        shutil.rmtree(output)
        guard(work, destination, deadline)
        mc.dump(destination/'summary.json', {'cells':records,'qualification':False})
        mc.dump(destination/'owned-scratch-members.json',mc.regular_members(work))
        shutil.rmtree(work)
        mc.dump(destination/'complete.json',{'exit_code':0,'native_cells':len(records),'scratch_removed':not work.exists(),
                                          'retained_bytes':footprint(destination),'qualification':False})
    except BaseException as error:
        preserve(work,destination,error)
        raise


if __name__ == '__main__':
    main()
