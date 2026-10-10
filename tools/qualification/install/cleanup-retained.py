#!/usr/bin/env python3
"""Dry-run or safely remove the archived failed VM and private Podman scratch."""
import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits

DATA = Path('/run/media/kmosoti/data/FabricO11y')
QEMU_ID = 'readiness-debian13-01'
PODMAN_ID = 'readiness-01'
BASE_SHA512 = (
    'f46f0671a6e5bdec5291ab8972bae2f10e5408c2f64a74078f11efc2f06a436a'
    '9d0313ed50e0472542eeabf780e9f7c792ac0a314c6c20507fcd9fd81b468c3d'
)
REJECTED_SERVER_SHA256 = '5e283c990fcbfd4739c3fa24aa0932530f317964461681e0a2906ef5d1282da1'


def sha(path: Path, algorithm: str) -> str:
    digest = hashlib.new(algorithm)
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def read_json(path: Path) -> dict[str, object]:
    value = json.loads(path.read_text())
    if not isinstance(value, dict):
        raise RuntimeError(f'expected JSON object: {path}')
    return value


def archived_evidence() -> dict[str, object]:
    qemu = DATA / 'results/installation-qemu' / QEMU_ID
    package = DATA / 'results/installation-package-build' / PODMAN_ID
    qreceipt = read_json(qemu / 'receipt.json')
    prechange = DATA / 'results/installation-qemu/a12-memory-probe-02/acceptance.sh.before-protocol-change'
    if qreceipt.get('acceptance_exit') != 1 or not qreceipt.get('cleanup_confirmed'):
        raise RuntimeError('QEMU receipt does not record the archived failed, stopped run')
    if not all((qemu / name).is_file() for name in (
            'acceptance.txt', 'bootstrap.txt', 'guest.txt', 'journal.txt',
            'qemu-stderr.txt', 'serial.txt')):
        raise RuntimeError('QEMU acceptance, journal, guest, bootstrap or serial evidence is incomplete')
    acceptance = (qemu / 'acceptance.txt').read_text(errors='replace')
    if 'ACCEPT A12 FAIL' not in acceptance or 'timeout/' not in acceptance:
        raise RuntimeError('archived QEMU log lacks the A12 timeout counterexample')
    if not prechange.is_file() or sha(prechange, 'sha256') != qreceipt.get('acceptance_sha256'):
        raise RuntimeError('pre-change acceptance archive does not match the failed-run receipt')
    image = DATA / 'cache/installation-qemu/debian-13-genericcloud-amd64.qcow2'
    if not image.is_file() or sha(image, 'sha512') != BASE_SHA512:
        raise RuntimeError('pinned Debian base image is missing or has a different hash')

    preceipt = read_json(package / 'receipt.json')
    provenance = package / 'provenance'
    manifest = read_json(provenance / 'source-manifest.json')
    bundle_name = manifest.get('source_bundle_file')
    if not isinstance(bundle_name, str) or Path(bundle_name).name != bundle_name:
        raise RuntimeError('source bundle manifest has an unsafe filename')
    bundle = provenance / bundle_name
    if preceipt.get('state') != 'failed' or not bundle.is_file():
        raise RuntimeError('Podman failure receipt or frozen source bundle is missing')
    if sha(bundle, 'sha256') != manifest.get('source_bundle_sha256'):
        raise RuntimeError('frozen source bundle hash does not match its manifest')
    if not all((package / name).is_file() for name in (
            'compatibility-counterexample.json', 'package-build.stderr',
            'package-build.stdout', 'server-symbols.txt', 'server-versions.txt')):
        raise RuntimeError('Podman GLIBC counterexample evidence is incomplete')

    podman_id = preceipt.get('run_id')
    if podman_id != PODMAN_ID:
        raise RuntimeError('Podman receipt run ID differs from the exact cleanup target')
    podman_scratch = DATA / 'scratch' / ('install-package-build-' + PODMAN_ID)
    if preceipt.get('scratch_retained') != str(podman_scratch):
        raise RuntimeError('Podman receipt does not identify the exact retained scratch path')
    qemu_scratch = DATA / 'scratch' / ('installation-qemu-' + QEMU_ID)
    if qreceipt.get('scratch_retained') != str(qemu_scratch):
        raise RuntimeError('QEMU receipt does not identify the exact retained scratch path')
    return {'qemu': qemu, 'qemu_scratch': qemu_scratch,
            'package': package, 'podman_scratch': podman_scratch,
            'package_receipt': preceipt, 'source_manifest': manifest,
            'qemu_receipt': qreceipt}


def mounted_below(path: Path) -> list[str]:
    mounts = []
    for line in Path('/proc/self/mountinfo').read_text().splitlines():
        fields = line.split(' - ', 1)[0].split()
        mount = fields[4].replace('\\040', ' ').replace('\\011', '\t')
        if mount == str(path) or mount.startswith(str(path) + '/'):
            mounts.append(mount)
    return mounts


def using_process(path: Path) -> list[int]:
    found = []
    needle = os.fsencode(str(path))
    for item in Path('/proc').iterdir():
        if not item.name.isdigit():
            continue
        try:
            if needle in (item / 'cmdline').read_bytes():
                found.append(int(item.name))
        except OSError:
            pass
    return found


def inspect_host_owned_tree(path: Path) -> None:
    if not path.is_dir() or path.is_symlink() or path.resolve() != path:
        raise RuntimeError(f'not an existing real directory: {path}')
    for parent, dirs, files in os.walk(path, followlinks=False):
        for name in dirs + files:
            item = Path(parent) / name
            info = item.lstat()
            if item.is_symlink() or info.st_uid != os.getuid():
                raise RuntimeError(f'refusing non-user-owned or symlink entry: {item} uid={info.st_uid}')
            if not (item.is_dir() or item.is_file()):
                raise RuntimeError(f'refusing special file: {item}')
    mounts = mounted_below(path)
    if mounts:
        raise RuntimeError(f'refusing scratch tree with mounts beneath it: {mounts}')
    pids = using_process(path)
    if pids:
        raise RuntimeError(f'refusing scratch tree still referenced by processes: {path} pids={pids}')


def podman_configuration(owned: Path, receipt: dict[str, object]) -> tuple[list[str], dict[str, str], list[Path]]:
    commands = receipt.get('commands')
    if not isinstance(commands, list):
        raise RuntimeError('package receipt lacks command provenance')
    info = next((item for item in commands if isinstance(item, dict)
                 and item.get('label') == 'podman-info'), None)
    if not isinstance(info, dict) or not isinstance(info.get('command'), list):
        raise RuntimeError('package receipt lacks the isolated Podman configuration')
    original = info['command']
    podman = original[0]
    fuse = next((str(item).split('=', 1)[1] for item in original
                 if str(item).startswith('overlay.mount_program=')), None)
    expected = [podman, '--root', str(owned / 'podman/graph'), '--runroot', str(owned / 'podman/run'),
                '--tmpdir', str(owned / 'podman/tmp'), '--storage-driver', 'overlay',
                '--storage-opt', f'overlay.mount_program={fuse}']
    if original[:len(expected)] != expected or not Path(str(podman)).is_file() \
            or not fuse or not Path(fuse).is_file():
        raise RuntimeError('recorded Podman paths/programs do not match this exact private store')
    store = owned / 'podman'
    for name in ('graph', 'run', 'tmp'):
        path = store / name
        if path.is_symlink() or not path.is_dir() or path.resolve() != path:
            raise RuntimeError(f'private Podman {name} path is missing or unsafe: {path}')
    env = dict(os.environ, TMPDIR=str(store / 'tmp'), TMP=str(store / 'tmp'),
               TEMP=str(store / 'tmp'))
    payloads = [owned / name for name in ('source', 'target', 'out', 'cargo-home', 'logs')]
    for path in payloads:
        if path.is_symlink() or not path.is_dir() or path.resolve() != path:
            raise RuntimeError(f'build payload path is missing or unsafe: {path}')
    return expected, env, payloads


def preserve_rejected_binary(owned: Path, package_result: Path) -> dict[str, object]:
    source = owned / 'out/stage/usr/bin/fabric-server'
    counterexample = read_json(package_result / 'compatibility-counterexample.json')
    if (not source.is_file() or source.is_symlink()
            or source.stat().st_uid != os.getuid()
            or sha(source, 'sha256') != REJECTED_SERVER_SHA256
            or counterexample.get('sha256') != REJECTED_SERVER_SHA256):
        raise RuntimeError('retained rejected fabric-server binary differs from the GLIBC counterexample')
    archived_dir = package_result / 'counterexamples' / 'debian13-rust198'
    archived_dir.mkdir(parents=True, exist_ok=True)
    archived = archived_dir / 'fabric-server'
    manifest_path = archived_dir / 'manifest.json'
    if archived.exists() or manifest_path.exists():
        if (not archived.is_file() or archived.is_symlink()
                or sha(archived, 'sha256') != REJECTED_SERVER_SHA256
                or not manifest_path.is_file()):
            raise RuntimeError('existing rejected-binary archive differs; refusing to overwrite')
    else:
        shutil.copy2(source, archived)
        if sha(archived, 'sha256') != REJECTED_SERVER_SHA256:
            raise RuntimeError('rejected binary changed while archiving')
        details = {
            'classification': 'rejected package-build counterexample; not a release artifact',
            'source': str(source), 'archive': str(archived),
            'sha256': REJECTED_SERVER_SHA256,
            'reason': 'Debian 13 Rust 1.98 binary references GLIBC_2.39 pidfd_spawnp/pidfd_getpid; registered floor is GLIBC_2.34',
            'source_commit': read_json(package_result / 'provenance/source-manifest.json').get('source_commit'),
            'source_bundle_sha256': read_json(package_result / 'provenance/source-manifest.json').get('source_bundle_sha256'),
            'counterexample': counterexample,
        }
        manifest_path.write_text(json.dumps(details, indent=2) + '\n')
    return {'binary': str(archived), 'sha256': REJECTED_SERVER_SHA256,
            'manifest': str(manifest_path)}


def podman_cleanup(owned: Path, receipt: dict[str, object], package_result: Path,
                   followup: dict[str, object]) -> None:
    podman, env, payloads = podman_configuration(owned, receipt)
    name = 'fabric-deb-build-' + PODMAN_ID

    def invoke(label: str, args: list[str], timeout: int,
               accepted: tuple[int, ...] = (0,)) -> subprocess.CompletedProcess[str]:
        try:
            result = subprocess.run(args, text=True, capture_output=True, timeout=timeout,
                                    check=False, env=env)
        except subprocess.TimeoutExpired as exc:
            action = {'action': label, 'exit': None, 'timed_out': True,
                      'stdout': exc.stdout.decode(errors='replace') if isinstance(exc.stdout, bytes) else (exc.stdout or ''),
                      'stderr': exc.stderr.decode(errors='replace') if isinstance(exc.stderr, bytes) else (exc.stderr or '')}
            followup.setdefault('actions', []).append(action)
            write_followup(package_result, followup)
            raise RuntimeError(f'Podman cleanup command {label} timed out') from exc
        followup.setdefault('actions', []).append({
            'action': label, 'command': args, 'exit': result.returncode,
            'stdout': result.stdout[-3000:], 'stderr': result.stderr[-3000:],
        })
        write_followup(package_result, followup)
        if result.returncode not in accepted:
            raise RuntimeError(f'Podman cleanup command {label} exited {result.returncode}: {result.stderr[-1000:]}')
        return result

    state = invoke('container-exists', [*podman, 'container', 'exists', name], 20, (0, 1))
    if state.returncode == 0:
        invoke('remove-exact-build-container', [*podman, 'rm', '--force', name], 30)
    elif state.returncode != 1:
        raise RuntimeError(f'Podman container exists returned unexpected exit {state.returncode}')
    invoke('remove-images-from-private-store', [*podman, 'image', 'rm', '--all', '--force'], 90)

    if mounted_below(owned):
        raise RuntimeError('private build scratch has mounted paths before payload removal')
    pids = using_process(owned)
    # The current cleanup helper is itself allowed to mention the path; reject
    # only other processes that can hold the private store or payloads open.
    pids = [pid for pid in pids if pid != os.getpid()]
    if pids:
        raise RuntimeError(f'private Podman store has live process references: {pids}')
    invoke('remove-build-payloads-in-user-namespace',
           [*podman, 'unshare', 'rm', '-rf', '--', *map(str, payloads)], 90)
    if any(path.exists() for path in payloads):
        raise RuntimeError('Podman unshare returned success but build payloads remain')
    if mounted_below(owned):
        raise RuntimeError('private build scratch still contains mounts after payload removal')
    if [pid for pid in using_process(owned) if pid != os.getpid()]:
        raise RuntimeError('private Podman store still has a process reference after teardown')


def write_followup(result: Path, followup: dict[str, object]) -> None:
    followup['updated_utc'] = dt.datetime.now(dt.timezone.utc).isoformat()
    (result / 'cleanup-followup.json').write_text(json.dumps(followup, indent=2) + '\n')


def remove_host_owned_tree(path: Path) -> None:
    if not path.is_dir() or path.is_symlink() or path.resolve() != path:
        raise RuntimeError(f'not a real scratch directory: {path}')
    for parent, dirs, files in os.walk(path, followlinks=False):
        for name in dirs + files:
            entry = Path(parent) / name
            info = entry.lstat()
            if entry.is_symlink() or info.st_uid != os.getuid():
                raise RuntimeError(f'refusing remaining non-user-owned or symlink path: {entry} uid={info.st_uid}')
            if not (entry.is_dir() or entry.is_file()):
                raise RuntimeError(f'refusing remaining special path: {entry}')
    if mounted_below(path):
        raise RuntimeError(f'refusing host removal with mounts below {path}')
    if [pid for pid in using_process(path) if pid != os.getpid()]:
        raise RuntimeError(f'refusing host removal while another process references {path}')
    shutil.rmtree(path)
    if path.exists():
        raise RuntimeError(f'host removal did not clear exact owned path {path}')


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--execute', action='store_true', help='preserve counterexample and remove only archived-run scratch')
    args = parser.parse_args()
    paths = archived_evidence()
    qemu_scratch = paths['qemu_scratch']
    podman_scratch = paths['podman_scratch']
    package_result = paths['package']
    package_receipt = paths['package_receipt']
    if not isinstance(qemu_scratch, Path) or not isinstance(podman_scratch, Path) \
            or not isinstance(package_result, Path) or not isinstance(package_receipt, dict):
        raise RuntimeError('cleanup target metadata has unexpected types')
    if not qemu_scratch.is_dir() or qemu_scratch.is_symlink():
        raise RuntimeError('exact retained QEMU scratch directory is missing or unsafe')
    inspect_host_owned_tree(qemu_scratch)
    marker = podman_scratch / '.fabric-package-build-owned'
    if not marker.is_file() or marker.is_symlink() or marker.read_text() != 'installation-build-v1\n':
        raise RuntimeError('exact package-build scratch ownership marker is missing or invalid')
    podman, _, _ = podman_configuration(podman_scratch, package_receipt)
    _ = podman  # configuration validation is intentionally done during dry-run too
    if mounted_below(qemu_scratch) or mounted_below(podman_scratch):
        raise RuntimeError('one of the exact retained scratch paths has a live mount')
    references = using_process(qemu_scratch) + using_process(podman_scratch)
    references = sorted(set(pid for pid in references if pid != os.getpid()))
    if references:
        raise RuntimeError(f'retained scratch paths are referenced by live processes: {references}')
    print(f'{"remove" if args.execute else "would-remove"} {qemu_scratch}')
    print(f'two-phase Podman teardown then remove {podman_scratch}')
    print('preflight: archived failed-run logs and hashes verified; private Podman root/runroot confirmed')
    if not args.execute:
        print('dry run only; the rejected binary will be copied and hash-verified before any deletion')
        return 0

    containment = require_limits()
    followup: dict[str, object] = {
        'classification': 'owned failed-run scratch cleanup; no shared caches or stores',
        'status': 'running', 'targets': [str(podman_scratch), str(qemu_scratch)],
        'resource_containment': str(containment), 'actions': [],
    }
    write_followup(package_result, followup)
    try:
        binary_archive = preserve_rejected_binary(podman_scratch, package_result)
        followup['rejected_binary_archive'] = binary_archive
        followup['status'] = 'counterexample preserved; tearing down private Podman store'
        write_followup(package_result, followup)
        podman_cleanup(podman_scratch, package_receipt, package_result, followup)
        remove_host_owned_tree(podman_scratch)
        followup['podman_scratch_removed'] = True
        write_followup(package_result, followup)

        qresult = paths['qemu']
        if not isinstance(qresult, Path):
            raise RuntimeError('QEMU result path has unexpected type')
        followup['status'] = 'Podman scratch removed; removing archived QEMU scratch'
        write_followup(package_result, followup)
        remove_host_owned_tree(qemu_scratch)
        followup['qemu_scratch_removed'] = True
        followup['status'] = 'removed'
        write_followup(package_result, followup)
        (qresult / 'cleanup-followup.json').write_text(json.dumps({
            'target': str(qemu_scratch), 'status': 'removed',
            'utc': dt.datetime.now(dt.timezone.utc).isoformat(),
            'method': 'receipt/source/image/log verification; no process or mount references; host-owned exact scratch tree',
        }, indent=2) + '\n')
        print(f'removed archived scratch; rejected binary retained at {binary_archive["binary"]}')
        return 0
    except (OSError, RuntimeError, subprocess.SubprocessError, ValueError) as error:
        followup['status'] = 'failed; remaining owned scratch retained'
        followup['error'] = f'{type(error).__name__}: {error}'
        write_followup(package_result, followup)
        raise


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, RuntimeError, subprocess.SubprocessError, ValueError,
            KeyError, json.JSONDecodeError) as exc:
        print(f'retained-scratch cleanup: NOT RUN: {exc}', file=sys.stderr)
        sys.exit(2)
