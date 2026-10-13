"""Packaging regressions: exact assets, tamper rejection and bounded unit policy."""
import hashlib
import io
import json
import lzma
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import tarfile
import time
from types import SimpleNamespace
import unittest
import zlib
from unittest.mock import patch

import dependency_notices
import build_candidate_rpm
from test_payload import assert_fixture_stopped, owned_process_fixture

from stage_console import stage
from stage_predecessor import stage_predecessor
from stage_candidate import stage_candidate, validate_control

ROOT = Path(__file__).resolve().parents[2]


def png(extra=None):
    """A minimal valid 1x1 PNG, optionally with one metadata chunk before IEND."""
    def chunk(kind, data):
        return len(data).to_bytes(4, 'big') + kind + data + zlib.crc32(kind + data).to_bytes(4, 'big')
    body = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', bytes([0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0]))
    body += chunk(b'IDAT', zlib.compress(b'\x00\x00'))
    if extra == b'tIME':
        body += chunk(extra, bytes([7, 234, 10, 12, 17, 0, 6]))
    elif extra:
        body += chunk(extra, b'date:modify\x002026-10-12T10:00:00+00:00')
    return body + chunk(b'IEND', b'')


class PackageTests(unittest.TestCase):
    def test_build_stops_real_descendant_on_success_and_timeout(self):
        # Only the test's real communicate timer is shortened; production requests 300s.
        real_popen = subprocess.Popen
        for mode in ('success', 'timeout'):
            with self.subTest(mode=mode), owned_process_fixture() as (directory, command):
                timeouts = []
                def spawn(*args, **kwargs):
                    process = real_popen(*args, **kwargs)
                    communicate = process.communicate
                    def bounded_communicate(*args, **kwargs):
                        timeouts.append(kwargs.get('timeout'))
                        if kwargs.get('timeout') == 300:
                            kwargs['timeout'] = 1
                        return communicate(*args, **kwargs)
                    process.communicate = bounded_communicate
                    return process
                with patch.object(build_candidate_rpm.subprocess, 'Popen', side_effect=spawn):
                    if mode == 'success':
                        self.assertEqual(build_candidate_rpm.run_build(command + [mode], os.environ.copy(), directory), 0)
                    else:
                        with self.assertRaises(subprocess.TimeoutExpired):
                            build_candidate_rpm.run_build(command + [mode], os.environ.copy(), directory)
                self.assertIn(300, timeouts, 'native build deadline changed in the control')
                self.assertEqual((directory / 'build.stdout').read_text(), 'literal process stdout\n')
                self.assertEqual((directory / 'build.stderr').read_text(), 'literal process stderr\n')
                assert_fixture_stopped(self, directory)

    def test_build_final_drain_interruption_preserves_raw_diagnostics(self):
        # Inject a typed final-drain error only AFTER a real bounded drain/reap.
        real_popen = subprocess.Popen
        with owned_process_fixture() as (directory, command):
            timeouts = []
            def spawn(*args, **kwargs):
                process = real_popen(*args, **kwargs)
                communicate = process.communicate
                def bounded_communicate(*args, **kwargs):
                    timeouts.append(kwargs.get('timeout'))
                    kwargs['timeout'] = 1
                    stdout, stderr = communicate(*args, **kwargs)
                    if len(timeouts) == 3:
                        raise subprocess.TimeoutExpired(process.args, 10, output=stdout, stderr=stderr)
                    return stdout, stderr
                process.communicate = bounded_communicate
                return process
            with patch.object(build_candidate_rpm.subprocess, 'Popen', side_effect=spawn):
                with self.assertRaises(subprocess.TimeoutExpired):
                    build_candidate_rpm.run_build(command + ['drain'], os.environ.copy(), directory)
            self.assertEqual(timeouts, [300, 10, 10])
            assert_fixture_stopped(self, directory)
            for stream in ('stdout', 'stderr'):
                with self.subTest(stream=stream):
                    path = directory / ('build.' + stream)
                    self.assertTrue(path.is_file(), 'final drain interruption lost raw ' + stream)
                    self.assertEqual(path.read_text(), 'literal process ' + stream + '\n')

    def test_constructor_parent_term_preserves_interrupted_receipt_and_stage(self):
        # Admission is synthetic; signals, children, staging and diagnostics are real.
        with owned_process_fixture() as (directory, command):
            driver = directory / 'constructor.py'
            driver.write_text('''import json, os, sys
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
import build_candidate_rpm as builder
directory = Path(sys.argv[1])
controls = json.loads((builder.ROOT / 'docs/experiments/formal/fixtures/owner-walkthrough.json').read_text())
packages = {family: directory / family for family in ('debian', 'fedora')}
for package in packages.values(): package.write_bytes(b'literal package stand-in')
receipt = directory / 'input.json'
receipt.write_text(json.dumps({'source': {'original_head': controls['original_head'], 'source_date_epoch': controls['source_date_epoch']}}))
def command_output(command):
    if command == ['dpkg', '--compare-versions', '0.1.0~alpha.0', 'lt', '0.1.0~alpha.1']: return b''
    if command[0] == 'dpkg-deb': return b'0.1.0~alpha.1'
    if '--eval' in command: return b'-1'
    return b'0' if '%{EPOCHNUM}' in command else b'0.1.0-0.alpha.1'
real_build = builder.run_build
def owned_build(command, env, out, label):
    if command[:2] != ['dpkg-deb', '-R'] or label != 'deb-unpack': raise AssertionError('unexpected native build command')
    stage = Path(command[-1]).parent
    return real_build([sys.executable, '-B', str(directory / 'child.py'), str(directory), 'timeout', str(stage)], env, out, label)
sys.argv = ['constructor.py', '--same-code-fixture', '--deb-receipt', str(receipt), '--rpm-receipt', str(receipt),
            '--out', str(directory / 'results/out'), '--frozen', controls['original_head'],
            '--source-date-epoch', str(controls['source_date_epoch']), '--harness-commit', '1' * 40]
with patch.dict('sys.modules', {'inputs': SimpleNamespace(validate_inputs=lambda *args: packages)}), \\
     patch.object(builder, 'STORAGE', directory), \\
     patch.object(builder, 'validate_harness'), \\
     patch.object(builder, 'inventory', return_value={'usr/bin/fabric-server': {'bytes': 3}}), \\
     patch.object(builder, 'deb_controls', return_value={'preinst': {'bytes': 3}}), \\
     patch.object(builder, 'rpm_records', return_value={'scripts': {'PREIN': {'bytes': 3}}, 'metadata': {}}), \\
     patch.object(builder, 'bounded_command', side_effect=command_output), \\
     patch.object(builder, 'run_build', side_effect=owned_build):
    raise SystemExit(builder.main())
''')
            scratch = directory / 'scratch'
            scratch.mkdir()
            env = {**os.environ, 'PYTHONPATH': str(ROOT / 'tools/packaging'), 'FABRIC_SCRATCH_ROOT': str(scratch)}
            process = subprocess.Popen([sys.executable, '-B', str(driver), str(directory)], env=env,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
            descriptor = os.pidfd_open(process.pid)
            try:
                deadline = time.monotonic() + 5
                while not (directory / 'diagnostics-ready').exists() and process.poll() is None and time.monotonic() < deadline:
                    time.sleep(.01)
                self.assertTrue((directory / 'diagnostics-ready').is_file(), 'constructor did not reach real owned staging')
                signal.pidfd_send_signal(descriptor, signal.SIGTERM)
                stdout, stderr = process.communicate(timeout=8)
                self.assertEqual(process.returncode, 2, stdout + stderr)
                path = directory / 'results/out/fixture-receipt.json'
                self.assertTrue(path.is_file(), 'parent TERM lost interrupted receipt; exit=' + str(process.returncode))
                report = json.loads(path.read_text())
                self.assertEqual((report['state'], report['exit']), ('interrupted', 2))
                retained = Path(report['failure_evidence'])
                self.assertTrue(retained.is_relative_to(directory / 'results/out'))
                self.assertEqual((retained / 'held-stage.bin').read_bytes(), b'literal owned staging')
                for stream in ('stdout', 'stderr'):
                    self.assertEqual((directory / ('results/out/deb-unpack.' + stream)).read_text(),
                                     'literal process ' + stream + '\n')
                self.assertEqual(list(scratch.iterdir()), [])
                assert_fixture_stopped(self, directory)
            finally:
                if process.poll() is None:
                    signal.pidfd_send_signal(descriptor, signal.SIGKILL)
                process.wait(timeout=3)
                os.close(descriptor)
                process.stdout.close()
                process.stderr.close()

    def test_same_code_fixture_archive_errors_preserve_failed_receipt(self):
        # Origin: #59 C-041 SF2, a corrupt archive escaped after staging began.
        controls = json.loads((ROOT / 'docs/experiments/formal/fixtures/owner-walkthrough.json').read_text())
        for case in controls['archive_error_controls']:
            with self.subTest(defect=case['defect']), tempfile.TemporaryDirectory() as scratch:
                directory = Path(scratch)
                packages = {family: directory / family for family in ('debian', 'fedora')}
                for package in packages.values():
                    package.write_bytes(b'literal package stand-in; no native installation')
                receipt = directory / 'deb-receipt.json'
                receipt.write_text(json.dumps({'source': {'original_head': controls['original_head'],
                                                        'source_date_epoch': controls['source_date_epoch']}}))
                args = SimpleNamespace(deb_receipt=receipt, rpm_receipt=receipt, out=directory / 'results/out',
                                       frozen=controls['original_head'], source_date_epoch=controls['source_date_epoch'],
                                       harness_commit='1' * 40)
                def command_output(command):
                    if command == ['dpkg', '--compare-versions', '0.1.0~alpha.0', 'lt', '0.1.0~alpha.1']:
                        return b''
                    if command[0] == 'dpkg-deb':
                        return b'0.1.0~alpha.1'
                    if '--eval' in command:
                        return b'-1'
                    return b'0' if '%{EPOCHNUM}' in command else b'0.1.0-0.alpha.1'
                def corrupt_archive(command, env, out, label):
                    work = Path(command[-1]).parent
                    raw = bytes.fromhex(case['bytes_hex'])
                    (work / 'literal-corrupt-archive.bin').write_bytes(raw)
                    if case['defect'] == 'truncated-control-tar':
                        with tarfile.open(fileobj=io.BytesIO(raw)):
                            pass
                    elif case['defect'] == 'invalid-xz':
                        lzma.decompress(raw)
                    elif case['defect'] == 'missing-member':
                        {}['literal-required-control-member']
                    else:
                        raise EOFError('literal truncated archive member')
                    self.fail('literal archive defect did not raise')
                with patch.dict('sys.modules', {'inputs': SimpleNamespace(validate_inputs=lambda *args: packages)}), \
                     patch.dict(os.environ, {'FABRIC_SCRATCH_ROOT': scratch}), \
                     patch.object(build_candidate_rpm, 'STORAGE', directory), \
                     patch.object(build_candidate_rpm, 'validate_harness'), \
                     patch.object(build_candidate_rpm, 'inventory', return_value={'usr/bin/fabric-server': {'bytes': 3}}), \
                     patch.object(build_candidate_rpm, 'deb_controls', return_value={'preinst': {'bytes': 3}}), \
                     patch.object(build_candidate_rpm, 'rpm_records', return_value={'scripts': {'PREIN': {'bytes': 3}}, 'metadata': {}}), \
                     patch.object(build_candidate_rpm, 'bounded_command', side_effect=command_output), \
                     patch.object(build_candidate_rpm, 'run_build', side_effect=corrupt_archive):
                    escaped = None
                    result = None
                    try:
                        result = build_candidate_rpm.fixture_main(args)
                    except Exception as error:
                        escaped = error
                        self.assertEqual(type(error).__name__, case['exception'])
                report = json.loads((args.out / 'fixture-receipt.json').read_text())
                self.assertIsNone(escaped, 'archive exception escaped without an archived failed verdict')
                self.assertEqual(result, 2)
                self.assertEqual((report['state'], report['exit']), ('failed', 2))
                self.assertTrue(report.get('error'), 'archive failure diagnosis is missing')
                retained = Path(report['failure_evidence'])
                self.assertTrue(retained.is_relative_to(args.out))
                self.assertEqual((retained / 'literal-corrupt-archive.bin').read_bytes(), bytes.fromhex(case['bytes_hex']))
                self.assertEqual(list(directory.glob('owner-fixture-*')), [])
                self.assertEqual(report['packages'], {})

    def test_same_code_fixture_deb_tar_header_controls(self):
        # Origin: #59 C-041 SF4, equal bytes did not bind tar member metadata.
        controls = json.loads((ROOT / 'docs/experiments/formal/fixtures/owner-walkthrough.json').read_text())
        self.assertTrue(callable(getattr(build_candidate_rpm, 'deb_headers', None)),
                        'missing complete fixture Debian tar-header collector')
        original = controls['deb_header_fixture']
        def collect(records):
            archives = {}
            for name, rows in records.items():
                raw = io.BytesIO()
                with tarfile.open(fileobj=raw, mode='w') as archive:
                    for row in rows:
                        member = tarfile.TarInfo(row['name'])
                        member.type = row['type'].encode()
                        for field in ('mode', 'uid', 'gid', 'uname', 'gname', 'mtime', 'size', 'linkname'):
                            setattr(member, field, row[field])
                        archive.addfile(member, io.BytesIO(b'x' * row['size']))
                archives[name] = raw.getvalue()
            with patch.object(build_candidate_rpm, 'bounded_command', side_effect=lambda command: archives[command[-1]]):
                return build_candidate_rpm.deb_headers(Path('literal.deb'))
        self.assertEqual(collect(original), original)
        for archive in original:
            for member in range(len(original[archive])):
                for case in controls['deb_header_controls']:
                    if original[archive][member]['type'] == '5' and case['field'] == 'size':
                        continue
                    with self.subTest(archive=archive, member=member, field=case['field']):
                        changed = json.loads(json.dumps(original))
                        if case['field'] == 'omitted':
                            changed[archive].pop(member)
                        else:
                            changed[archive][member][case['field']] = case['value']
                        self.assertNotEqual(collect(changed), original, 'tar-header drift was invisible')

    def test_same_code_fixture_rpm_provide_and_weak_dependency_controls(self):
        # Origin: #59 C-041 SF3; only the two native self-provide EVRs may change.
        controls = json.loads((ROOT / 'docs/experiments/formal/fixtures/owner-walkthrough.json').read_text())
        def collect(records):
            xml = '<rpmHeader>'
            for name, value in records.items():
                kind, values = value['kind'], value['values']
                item = 'integer' if kind.startswith('integer') else 'string'
                body = ''.join('<' + item + '>' + str(v) + '</' + item + '>' for v in values)
                if kind.endswith('Array'):
                    body = '<' + kind + '>' + body + '</' + kind + '>'
                xml += '<rpmTag name="' + name + '">' + body + '</rpmTag>'
            xml += '</rpmHeader>'
            with patch.object(build_candidate_rpm, 'bounded_command', return_value=xml.encode()):
                return build_candidate_rpm.rpm_records(Path('literal.rpm'))['metadata']
        original = controls['rpm_metadata_fixture']
        expected = collect(original)
        for case in controls['rpm_metadata_controls']:
            with self.subTest(defect=case['defect']):
                changed = json.loads(json.dumps(original))
                changed['RELEASE']['values'][0] = '0.alpha.0'
                changed['PROVIDEVERSION']['values'][:2] = ['0.1.0-0.alpha.0'] * 2
                if case['tag'] is not None:
                    changed[case['tag']]['values'][case['index']] = case['value']
                actual = collect(changed)
                if case['expected'] == 'accepted':
                    self.assertEqual(actual, expected, 'expected self-provide EVR change was refused')
                else:
                    self.assertNotEqual(actual, expected, 'unrelated native metadata drift was invisible')

    def test_same_code_fixture_source_and_literal_transform_controls(self):
        # Origin: owner walkthrough requires a real native version increase
        # without rewriting exact candidate payload, scripts or provenance.
        self.assertTrue(callable(getattr(build_candidate_rpm, 'validate_fixture_source', None)),
                        'missing fixture source validator')
        self.assertTrue(callable(getattr(build_candidate_rpm, 'validate_fixture_transform', None)),
                        'missing fixture transform validator')
        validate_fixture_source = build_candidate_rpm.validate_fixture_source
        validate_fixture_transform = build_candidate_rpm.validate_fixture_transform
        controls = json.loads((ROOT / 'docs/experiments/formal/fixtures/owner-walkthrough.json').read_text())
        source = {'original_head': controls['original_head'], 'source_date_epoch': controls['source_date_epoch']}
        validate_fixture_source(source, controls['original_head'], controls['source_date_epoch'])
        for field, bad in (('original_head', '0' * 40), ('source_date_epoch', 1), ('source_date_epoch', True)):
            with self.subTest(field=field, bad=bad), self.assertRaisesRegex(ValueError, 'fixture source pins differ'):
                validate_fixture_source({**source, field: bad}, controls['original_head'], controls['source_date_epoch'])
        for family, versions in controls['fixture_versions'].items():
            for case in controls['transform_controls']:
                with self.subTest(family=family, defect=case['defect']):
                    before = {'usr/bin/fabric-server': {'sha256': 'a' * 64, 'bytes': 3},
                              'usr/share/fabrico11y/console/index.html': {'sha256': 'b' * 64, 'bytes': 4},
                              'usr/share/doc/fabrico11y/CANDIDATE-PROVENANCE.json': {'sha256': 'c' * 64, 'bytes': 5}}
                    after = {name: dict(value) for name, value in before.items()}
                    scripts, changed = b'exact maintainer scripts', b'exact maintainer scripts'
                    version = versions[1]
                    if case['defect'] == 'payload':
                        after['usr/share/doc/fabrico11y/CANDIDATE-PROVENANCE.json']['sha256'] = 'd' * 64
                    elif case['defect'] == 'script':
                        changed = b'exit 0'
                    elif case['defect'] == 'equal-version':
                        version = versions[0]
                    if case['expected'] == 'accepted':
                        validate_fixture_transform(family, versions[0], version, before, after, scripts, changed)
                    else:
                        with self.assertRaisesRegex(ValueError, case['expected']):
                            validate_fixture_transform(family, versions[0], version, before, after, scripts, changed)

    def test_mpl_source_archive_identity_and_tamper_refusal(self):
        # Origin: release source obligation. A declared version alone cannot
        # establish the identity of the source shipped beside binary code.
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            archive = directory / 'cargo/registry/cache/registry/webauthn-rs-0.5.5.crate'
            archive.parent.mkdir(parents=True)
            archive.write_bytes(b'exact upstream source fixture')
            expected = hashlib.sha256(archive.read_bytes()).hexdigest()
            lock = directory / 'Cargo.lock'
            lock.write_text('[[package]]\nname="webauthn-rs"\nversion="0.5.5"\nsource="registry+fixture"\nchecksum="' + expected + '"\n')
            package = {'name': 'webauthn-rs', 'version': '0.5.5', 'source': 'registry+fixture', 'id': 'fixture'}
            with patch.object(dependency_notices, 'ROOT', directory), patch.dict(os.environ, {'CARGO_HOME': str(directory / 'cargo')}):
                identity = dependency_notices.bundle_mpl_source(package, directory / 'docs')
                self.assertEqual(identity['sha256'], expected)
                self.assertEqual((directory / 'docs' / identity['path']).read_bytes(), archive.read_bytes())
                archive.write_bytes(b'tampered')
                with self.assertRaises(ValueError):
                    dependency_notices.bundle_mpl_source(package, directory / 'bad')
                self.assertFalse((directory / 'bad').exists())
                package['version'] = '0.5.6'
                with self.assertRaises(ValueError):
                    dependency_notices.bundle_mpl_source(package, directory / 'future')

    def test_candidate_rejects_wrong_version_and_substituted_package(self):
        for version in ('0.1.0~alpha.0', '0.1.0~alpha.2', '0.1.0'):
            with self.assertRaises(ValueError):
                validate_control('Package: fabrico11y\nVersion: ' + version + '\nArchitecture: amd64\n')
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            package = directory / 'candidate.deb'
            package.write_bytes(b'substituted payload')
            receipt = directory / 'receipt.json'
            receipt.write_text(json.dumps({'state': 'built', 'package_family': 'debian', 'container_cleanup_confirmed': True, 'package': str(package), 'package_sha256': '0' * 64}))
            with self.assertRaises(ValueError):
                stage_candidate(package, receipt, directory / 'stage')
            self.assertFalse((directory / 'stage').exists())

    def fixture(self, directory):
        build = directory / 'build'
        dist = build / 'dist'
        dist.mkdir(parents=True)
        assets = {}
        for name in ('index.html', 'service-worker.js', 'manifest.webmanifest', 'icon-192.png', 'icon-512.png', 'client-a.wasm'):
            body = png() if name.endswith('.png') else name.encode()
            (dist / name).write_bytes(body)
            assets[name] = {'bytes': len(body), 'sha256': hashlib.sha256(body).hexdigest()}
        (build / 'build.json').write_text(json.dumps({'exit': 0, 'assets': assets, 'source_sha256': {'crates/fabric-ui/index.html': hashlib.sha256((ROOT / 'crates/fabric-ui/index.html').read_bytes()).hexdigest()}, 'headers': {'Content-Security-Policy': "default-src 'self'"}}))
        return build

    def test_complete_assets_preserve_identity(self):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            build = self.fixture(directory)
            stage(build, directory / 'package')
            manifest = json.loads((directory / 'package/asset-manifest.json').read_text())
            for name, identity in manifest.items():
                self.assertEqual(hashlib.sha256((directory / 'package' / name).read_bytes()).hexdigest(), identity['sha256'])
            self.assertTrue((directory / 'package/console-headers.json').is_file())

    def test_tampered_or_missing_or_unlisted_assets_refused(self):
        # Origin: packaging review. Contract: a candidate must ship the exact
        # complete hashed app shell. Defects are deliberately injected.
        for defect in ('changed', 'missing', 'unlisted', 'symlink', 'failed', 'source'):
            with self.subTest(defect=defect), tempfile.TemporaryDirectory() as scratch:
                directory = Path(scratch)
                build = self.fixture(directory)
                if defect == 'changed':
                    (build / 'dist/client-a.wasm').write_bytes(b'corrupt')
                elif defect == 'missing':
                    (build / 'dist/icon-192.png').unlink()
                elif defect == 'unlisted':
                    (build / 'dist/extra.js').write_text('extra')
                elif defect == 'symlink':
                    (build / 'dist/client-a.wasm').unlink()
                    (build / 'dist/client-a.wasm').symlink_to(build / 'dist/index.html')
                elif defect == 'source':
                    receipt = json.loads((build / 'build.json').read_text())
                    receipt['source_sha256']['crates/fabric-ui/index.html'] = '0' * 64
                    (build / 'build.json').write_text(json.dumps(receipt))
                else:
                    receipt = json.loads((build / 'build.json').read_text())
                    receipt['exit'] = 1
                    (build / 'build.json').write_text(json.dumps(receipt))
                with self.assertRaises(ValueError):
                    stage(build, directory / 'package')
                self.assertFalse((directory / 'package').exists())

    def test_console_icons_carry_no_build_timestamp(self):
        # Origin: #55. ImageMagick wrote date:modify/date:timestamp text chunks,
        # so two console builds of one commit shipped different icons and
        # packages. Contract: separate builds of the same source are identical.
        for chunk in (b'tIME', b'tEXt', b'zTXt', b'iTXt'):
            with self.subTest(chunk=chunk), tempfile.TemporaryDirectory() as scratch:
                directory = Path(scratch)
                build = self.fixture(directory)
                body = png(chunk)
                (build / 'dist/icon-192.png').write_bytes(body)
                receipt = json.loads((build / 'build.json').read_text())
                receipt['assets']['icon-192.png'] = {'bytes': len(body), 'sha256': hashlib.sha256(body).hexdigest()}
                (build / 'build.json').write_text(json.dumps(receipt))
                with self.assertRaisesRegex(ValueError, 'time or text metadata'):
                    stage(build, directory / 'package')
                self.assertFalse((directory / 'package').exists())

    def test_preinstall_refuses_root_ids_without_touching_host_accounts(self):
        # Origin: packaging review found the old >=1000 guard admitted UID/GID0.
        # Independent contract: installed service identity must be non-root.
        for uid, gid, expected in ((0, 500, 1), (500, 0, 1), (500, 500, 0)):
            with self.subTest(uid=uid, gid=gid), tempfile.TemporaryDirectory() as scratch:
                directory = Path(scratch)
                getent = directory / 'getent'
                getent.write_text(f"#!/bin/sh\ncase \"$1\" in passwd) echo 'fabricolly:x:{uid}:{gid}:service:/:/usr/sbin/nologin';; group) echo 'fabricolly:x:{gid}:';; esac\n")
                getent.chmod(0o755)
                identity = directory / 'id'
                identity.write_text('#!/bin/sh\necho fabricolly\n')
                identity.chmod(0o755)
                result = subprocess.run(['sh', str(ROOT / 'packaging/debian/preinst'), 'install'],
                                        env=dict(os.environ, PATH=str(directory) + ':' + os.environ['PATH']),
                                        text=True, capture_output=True)
                self.assertEqual(result.returncode, expected)
                if expected:
                    self.assertIn('refusing to install', result.stderr)

    def test_unregistered_predecessor_is_refused_before_extracting(self):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            package = directory / 'wrong.deb'
            package.write_bytes(b'unregistered predecessor')
            with self.assertRaises(ValueError):
                stage_predecessor(package, directory / 'stage')
            self.assertFalse((directory / 'stage').exists())

    def test_interrupted_predecessor_reversion_exits_nonzero(self):
        # Origin: review of the predecessor script. Its cleanup trap also caught
        # TERM and then resumed, so a run signalled while packaging hashed the
        # output and exited 0. Stubs stand in for the registered package.
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            (directory / 'out').mkdir()
            for name, body in (
                ('sha256sum', 'echo "ac71995b3c8b88cbb6f7dd05d9854e347b7a783caadc1b602f6590d6460f4a2f  $1"'),
                ('dpkg-deb', 'if [ "$1" = -R ]; then mkdir -p "$3/DEBIAN"; '
                             'printf "Version: 0\\nArchitecture: amd64\\n" > "$3/DEBIAN/control"; '
                             'else kill -TERM "$PPID"; : > "$5"; fi'),
            ):
                (directory / name).write_text('#!/bin/sh\n' + body + '\n')
                (directory / name).chmod(0o755)
            result = subprocess.run(['sh', str(ROOT / 'packaging/reversion-predecessor-deb.sh'),
                                     str(directory / 'sha256sum'), str(directory / 'out')],
                                    env=dict(os.environ, TMPDIR=scratch,
                                             PATH=scratch + ':' + os.environ['PATH']),
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 143, result.stdout + result.stderr)
            self.assertEqual(list((directory / 'out').glob('*.sha256')), [])
            self.assertEqual(list(directory.glob('predecessor-*')), [])

    def test_packagers_have_valid_shell_syntax(self):
        for script in (ROOT / 'packaging').rglob('*'):
            if script.is_file() and script.read_bytes().startswith(b'#!/bin/sh'):
                subprocess.run(['sh', '-n', str(script)], check=True)
                # A cleanup trap that also catches a signal resumes the script.
                self.assertNotRegex(script.read_text(), r'trap .*EXIT.*(HUP|INT|TERM)', script)
        subprocess.run(['bash', '-n', str(ROOT / 'tools/ci/prepare-resource-host.sh')], check=True)

    def test_server_and_companion_bound_is_below_four_decimal_gb(self):
        unit = (ROOT / 'packaging/systemd/fabrico11y-server.service').read_text()
        memory = next(line.split('=', 1)[1] for line in unit.splitlines() if line.startswith('MemoryMax='))
        self.assertTrue(memory.endswith('M'))
        self.assertLessEqual(int(memory[:-1]) * 1024**2, 4_000_000_000)
        self.assertIn('MemorySwapMax=0', unit)
        self.assertIn('Slice=system-fabrico11y.slice', unit)


if __name__ == '__main__':
    unittest.main()
