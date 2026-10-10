"""Packaging regressions: exact assets, tamper rejection and bounded unit policy."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import dependency_notices

from stage_console import stage
from stage_predecessor import stage_predecessor
from stage_candidate import stage_candidate, validate_control

ROOT = Path(__file__).resolve().parents[2]


class PackageTests(unittest.TestCase):
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
        for version in ('0.1.0~alpha.1', '0.1.0~alpha.3', '0.1.0'):
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
            body = name.encode()
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

    def test_packagers_have_valid_shell_syntax(self):
        for script in (ROOT / 'packaging').rglob('*'):
            if script.is_file() and script.read_bytes().startswith(b'#!/bin/sh'):
                subprocess.run(['sh', '-n', str(script)], check=True)
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
