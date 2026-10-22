"""Literal stopped-bundle copies; no synthetic record authenticates a guest."""
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import shlex
import signal
import stat
import struct
import subprocess
import tarfile
import time
import unittest
from unittest.mock import patch

import custody_native as native
import test_custody_native as literal
SOURCE, NODE = literal.SOURCE, literal.NODE
from test_custody_witness import frame, BINDING


class CaptureAdmissionControls(literal.NativeFixture, unittest.TestCase):
    """Independent outer tar records surround existing hand-encoded raw frames."""
    def setUp(self):
        super().setUp()
        self.module = literal.adapter()
        self._cutoff_pin = None
        self._cutoff_results = []
        self._standalone_helpers = []
        self._preserve_cutoff_diagnostics = False

    def tearDown(self):
        # Keep the complete invocation bundle and Rust stdout/stderr when a
        # testcase fails; those files are the evidence needed to diagnose it.
        if getattr(self._outcome, 'success', False) and not self._preserve_cutoff_diagnostics:
            import shutil
            for result, helper in self._cutoff_results:
                shutil.rmtree(result, ignore_errors=True)
                Path(helper['path']).unlink(missing_ok=True)
            for helper in self._standalone_helpers:
                Path(helper['path']).unlink(missing_ok=True)
        elif self._cutoff_results:
            print('preserved custody cutoff diagnostics: ' + ', '.join(str(row[0]) for row in self._cutoff_results))
        super().tearDown()

    def cutoff_pin(self):
        if self._cutoff_pin is None:
            self._cutoff_pin = self.pinned_helper(Path(os.environ['FABRIC_STORAGE_ROOT']),
                suffix='fixture-' + str(os.getpid()) + '-' + str(time.monotonic_ns()))
        return self._cutoff_pin

    def show_cutoff_diagnostics(self, result):
        self._preserve_cutoff_diagnostics = True
        print('preserved custody cutoff diagnostics: ' + str(result))
        for phase in ('drain', 'reboot'):
            for suffix in ('.stdout', '.stderr'):
                path = result / ('custody-cutoff-' + phase + suffix)
                if path.is_file():
                    with path.open('rb') as stream:
                        body = stream.read(8193)
                    print(f'--- {path.name} (first 8192 bytes) ---')
                    print(body[:8192].decode('utf-8', 'replace'))

    def admit_transfer(self, bundle, admission, expected_sha, *, preserve_on_error=False):
        storage = Path(os.environ['FABRIC_STORAGE_ROOT']).resolve()
        pin = self.cutoff_pin()
        admission['cutoff_helper'] = pin
        result = storage / 'results' / f'custody-capture-{os.getpid()}-{id(self)}-{time.monotonic_ns()}'
        result.mkdir(mode=0o700)
        archived = result / 'native-bundle.tar'
        import shutil
        shutil.copyfile(bundle, archived)
        self._cutoff_results.append((result, pin))
        try:
            return native.admit_transfer(archived, admission, expected_sha,
                cutoff_helper=pin, result_dir=result, data_root=storage, deadline=time.monotonic() + 60)
        except BaseException:
            if preserve_on_error:
                self.show_cutoff_diagnostics(result)
            raise

    def reset_fixture(self):
        import shutil
        if not self._preserve_cutoff_diagnostics:
            for result, helper in self._cutoff_results:
                shutil.rmtree(result, ignore_errors=True)
                Path(helper['path']).unlink(missing_ok=True)
        for helper in self._standalone_helpers:
            Path(helper['path']).unlink(missing_ok=True)
        self.temp.cleanup()
        self.setUp()

    reboot_chain = literal.RebootRawControls.reboot_chain

    def test_original_raw_tree_survives_semantic_reader_refusal(self):
        observer = native.Guest.__new__(native.Guest)
        observer.output = self.root / 'captured'; observer.output.mkdir()
        observer.observe_stop = lambda: {}
        observer.read_input = lambda path, limit: (b'original source',list(native.witness.identity(self.spool.stat())),None)
        original_read = Path.read_text
        def settings(path, *args, **kwargs):
            return (f'spool_dir={self.spool}\nspool_bytes=268435456\ntoken_file=/etc/fabrico11y/node-token\n'
                    f'log={SOURCE}\nlog=/var/log/fabric-accept/denied.log\n') if str(path)=='/etc/fabrico11y/node.conf' else original_read(path,*args,**kwargs)
        def reject(*args, **kwargs):
            with tarfile.open(observer.output/'before-raw.tar') as archive:
                self.assertEqual(archive.extractfile('spool/identity').read(), (self.spool/'identity').read_bytes())
                self.assertIn('spool/diagnostics/spindle.log',archive.getnames())
            raise ValueError('injected semantic reader refusal')
        with patch.object(native,'ROOT',self.spool), patch.object(Path,'read_text',settings), \
                patch.object(native.witness,'stopped_snapshot',side_effect=reject):
            with self.assertRaisesRegex(ValueError,'injected semantic reader refusal'):
                observer.capture('before')

    def fixture_files(self):
        configuration = self.spool / 'applied-config.json'
        configuration.write_bytes(literal.APPLIED); configuration.chmod(0o600)
        chain, reboot, reboot_events = self.reboot_chain()
        before, installed, drain, queries, events = chain
        boots = {'literal-boot-a': '00000000-0000-4000-8000-000000000001',
                 'literal-boot-b': '00000000-0000-4000-8000-000000000002'}
        files = {}
        snapshots = dict(prepare=before, installed=installed, drain=drain, reboot=reboot)
        for phase, snapshot in snapshots.items():
            name = 'before' if phase == 'prepare' else phase
            configuration = json.dumps(native.expected_profile(phase), separators=(',', ':')).encode()
            snapshot['applied_configuration'] = configuration
            snapshot['files']['applied-config.json'].update(sha256=native.sha(configuration), size=len(configuration))
            snapshot['file_observations']['applied-config.json'] = list(snapshot['file_observations']['applied-config.json'])
            snapshot['file_observations']['applied-config.json'][6] = len(configuration)
            snapshot['boot_id'] = boots[snapshot['boot_id']]
            process = snapshot['candidate_process']
            if process:
                process['boot_id'] = boots[process['boot_id']]
                process['start_ns'] *= 1000; process['end_ns'] *= 1000
                process['invocation_id'] = ('1' if phase == 'drain' else '2') * 32
                process.update(exe_sha256='a' * 64, status=f'Uid:\t{os.getuid()}\nGid:\t{os.getgid()}\n',
                    show=dict(stdout=f"MainPID={process['pid']}\nInvocationID={process['invocation_id']}\nControlGroup=/system.slice/system-fabrico11y.slice/{native.UNITS[1]}\n"))
            receipt = dict(binding=BINDING, spool_root=str(native.ROOT), boot_id=snapshot['boot_id'],
                service_identity=dict(uid=os.getuid(),gid=os.getgid()),
                root_stat=list(native.witness.identity(self.spool.stat())))
            files[phase + '/' + name + '-receipt.json'] = receipt
            files[phase + '/' + name + '-snapshot.json'] = native.packed(snapshot)
            files[phase + '/processes.json'] = dict(proc=[], extra_units=[], ps=dict(stdout='', exit=0))
            for unit in native.UNITS:
                files[phase + '/' + unit + '.json'] = dict(active=dict(exit=3, stdout='inactive\n'),
                    show=dict(exit=0, stdout='MainPID=0\nControlGroup=\nActiveState=inactive\n'), members={})
            if phase in ('drain', 'reboot'):
                selected = events if phase == 'drain' else reboot_events
                journal = []
                for event in selected:
                    event['boot_id'] = boots[event['boot_id']]; event['at_ns'] *= 1000
                    journal.append(dict(MESSAGE=event['message'], _PID=str(event['pid']),
                        _BOOT_ID=event['boot_id'].replace('-', ''), _SYSTEMD_UNIT=event['unit'],
                        _SYSTEMD_INVOCATION_ID=process['invocation_id'], __MONOTONIC_TIMESTAMP='123456',
                        _SYSTEMD_CGROUP='/system.slice/system-fabrico11y.slice/' + event['unit'],
                        _EXE='/usr/bin/fabric-node', _UID=str(os.getuid()), _GID=str(os.getgid()),
                        __REALTIME_TIMESTAMP=str(event['at_ns'] // 1000)))
                files[phase + '/journal.jsonl'] = ''.join(json.dumps(event) + '\n' for event in journal).encode()
                files[phase + '/ack-events.json'] = selected
            rows = queries['before'] if phase == 'prepare' else queries['after_' + phase] if phase != 'installed' else None
            if rows is not None:
                label = 'before' if phase == 'prepare' else 'after_' + phase
                files[phase + '/' + label + '-query-1-page-0.json'] = dict(rows=rows, next_page=None)
            files[phase + '/' + name + '-credential.json'] = dict(enrollment='literal-only',
                files={'/etc/fabrico11y/node-token': dict(stat=[7, 9, 0, 0, 0, 1, 32, 0, 0],
                    sha256='d' * 64, trimmed_sha256='d' * 64, btrfs_identity=None)})
            sources = {}
            for index, path in enumerate((SOURCE, native.DIAG)):
                log = snapshot['logs'][path]
                leaf = name + '-source-' + str(index)
                files[phase + '/' + leaf] = log['bytes']
                sources[path] = dict(archive_path=str(native.FIXTURE / BINDING['run_id'] / phase / leaf),
                    device=log['device'], inode=log['inode'], sha256=log['sha256'],
                    btrfs_identity=log['btrfs_identity'], original_stat=[log['device'], log['inode']])
            files[phase + '/' + name + '-sources.json'] = sources
            content = {'identity': b'FAI1' + NODE + struct.pack('<Q', 1),
                'acked': b'FAK1' + struct.pack('<QQ', 1, snapshot['ack']), 'writer.lock': b'', 'batches.faj': b'',
                'applied-config.json': configuration}
            for source in snapshot['sources']:
                content['batches.faj'] += frame(source['payload'], len(content['batches.faj']))
            for leaf, observed in snapshot['diagnostics'].items(): content['diagnostics/' + leaf] = observed['bytes']
            stream = io.BytesIO()
            with tarfile.open(fileobj=stream, mode='w') as raw:
                self.tar_member(raw, 'spool', b'', receipt['root_stat'], directory=True)
                for leaf, info in snapshot['file_observations'].items():
                    self.tar_member(raw, 'spool/' + leaf, content.get(leaf, b''), info, directory=stat.S_ISDIR(info[2]))
            files[phase + '/' + name + '-raw.tar'] = stream.getvalue()
        files['reboot/conservation.json'] = dict(status='accepted', required_checkpoints=4)
        admission = dict(binding=BINDING, node_sha256='a' * 64, boot_before=boots['literal-boot-a'])
        self.captured_metadata(files)
        return files, admission

    @staticmethod
    def tar_member(archive, name, content, info, *, directory=False):
        item = tarfile.TarInfo(name)
        item.uid, item.gid, item.mode = info[3], info[4], stat.S_IMODE(info[2])
        item.type = tarfile.DIRTYPE if directory else tarfile.REGTYPE
        item.size = 0 if directory else len(content)
        archive.addfile(item, None if directory else io.BytesIO(content))

    def bundle(self, files, *, owner=0, unsafe=False):
        encoded = {name: value if isinstance(value, bytes) else json.dumps(value).encode()
                   for name, value in files.items()}
        for phase in ('prepare', 'installed', 'drain', 'reboot'):
            entries = {Path(name).name: dict(sha256=hashlib.sha256(raw).hexdigest(),
                stat=[0, 1, stat.S_IFREG | 0o600, 0, 0, 1, len(raw), 0, 0])
                for name, raw in encoded.items() if name.startswith(phase + '/')}
            encoded[phase + '/manifest.json'] = json.dumps(dict(binding=BINDING, phase=phase, files=entries)).encode()
        bundle = self.root / 'bundle.tar'
        with tarfile.open(bundle, 'w') as archive:
            for name, raw in encoded.items():
                self.tar_member(archive, '../' + name if unsafe else name, raw,
                                [0, 1, stat.S_IFREG | 0o600, owner, 0, 1, len(raw), 0, 0])
        return bundle

    def test_literal_complete_bundle_is_admitted(self):
        files, admission = self.fixture_files()
        bundle = self.bundle(files)
        self.assertEqual(self.admit_transfer(bundle, admission, native.sha(bundle.read_bytes()),
            preserve_on_error=True)['phases'],
                         ['drain', 'installed', 'prepare', 'reboot'])

    def test_coherently_rehashed_missing_or_forged_configuration_is_refused(self):
        for missing in (True, False):
            with self.subTest(missing=missing):
                files, admission = self.fixture_files()
                for phase in ('prepare','installed','drain','reboot'):
                    name = 'before' if phase == 'prepare' else phase
                    key = phase+'/'+name+'-snapshot.json'
                    snapshot = native.unpack(files[key])
                    changed = literal.APPLIED.replace(b'false',b'true')
                    if missing:
                        snapshot['applied_configuration'] = None
                        snapshot['files'].pop('applied-config.json')
                        snapshot['file_observations'].pop('applied-config.json')
                    else:
                        snapshot['applied_configuration'] = changed
                        snapshot['files']['applied-config.json'].update(sha256=native.sha(changed),size=len(changed))
                        snapshot['file_observations']['applied-config.json'][6] = len(changed)
                    files[key] = native.packed(snapshot)
                    rawkey=phase+'/'+name+'-raw.tar'; stream=io.BytesIO()
                    with tarfile.open(fileobj=io.BytesIO(files[rawkey])) as old, tarfile.open(fileobj=stream,mode='w') as new:
                        for item in old.getmembers():
                            if item.name == 'spool/applied-config.json' and missing: continue
                            raw=old.extractfile(item).read() if item.isfile() else None
                            if item.name == 'spool/applied-config.json': raw=changed; item.size=len(raw)
                            new.addfile(item,io.BytesIO(raw) if raw is not None else None)
                    files[rawkey]=stream.getvalue()
                bundle=self.bundle(files)  # Recomputes every manifest and transfer hash.
                with self.assertRaisesRegex(ValueError,'native applied configuration'):
                    self.admit_transfer(bundle,admission,native.sha(bundle.read_bytes()))
            self.reset_fixture()

    def test_transfer_digest_path_and_ownership_refusals(self):
        files, admission = self.fixture_files()
        for variant in ('digest', 'path', 'owner'):
            with self.subTest(variant=variant):
                bundle = self.bundle(files, owner=55 if variant == 'owner' else 0, unsafe=variant == 'path')
                expected = '0' * 64 if variant == 'digest' else native.sha(bundle.read_bytes())
                with self.assertRaisesRegex(ValueError, 'transfer digest|transfer path/ownership'):
                    self.admit_transfer(bundle, admission, expected)

    def test_bound_copy_controls_refuse_at_named_boundary(self):
        for variant, message in (('binding', 'binding'), ('live-ps', 'stopped observations'),
                ('unit-pid', 'native stop'), ('missing-unit', 'stopped observations'), ('raw-tree', 'raw tree'), ('raw-bytes', 'raw bytes'),
                ('query-drop', 'row conservation'), ('query-duplicate', 'row conservation'),
                ('ack-hash', 'ACK exact bytes'), ('credential', 'credential metadata'),
                ('candidate', 'candidate process executable'), ('journal-incarnation', 'raw journal sender'), ('snapshot-root', 'root/binding')):
            with self.subTest(variant=variant):
                files, admission = self.fixture_files()
                if variant == 'binding': admission['binding'] = dict(BINDING, candidate_frozen='0' * 40)
                elif variant == 'live-ps': files['prepare/processes.json']['ps']['stdout'] = '55 1 1000 1000 fabric-node /usr/bin/fabric-node\n'
                elif variant == 'missing-unit': del files['prepare/' + native.UNITS[1] + '.json']
                elif variant == 'unit-pid': files['prepare/' + native.UNITS[1] + '.json']['show']['stdout'] = 'MainPID=55\nActiveState=inactive\n'
                elif variant in ('raw-tree', 'raw-bytes'):
                    original = files['prepare/before-raw.tar']; modified = io.BytesIO()
                    with tarfile.open(fileobj=io.BytesIO(original)) as old, tarfile.open(fileobj=modified, mode='w') as new:
                        for member in old.getmembers():
                            if variant == 'raw-tree' and member.name == 'spool/identity': continue
                            content = old.extractfile(member).read() if member.isfile() else b''
                            if variant == 'raw-bytes' and member.name == 'spool/identity': content = b'x' * len(content)
                            new.addfile(member, io.BytesIO(content) if member.isfile() else None)
                    files['prepare/before-raw.tar'] = modified.getvalue()
                elif variant == 'query-drop': files['reboot/after_reboot-query-1-page-0.json']['rows'].pop()
                elif variant == 'query-duplicate': files['reboot/after_reboot-query-1-page-0.json']['rows'].append(copy.deepcopy(files['reboot/after_reboot-query-1-page-0.json']['rows'][-1]))
                elif variant == 'ack-hash':
                    event = next(event for event in files['reboot/ack-events.json']
                        if event['message'].startswith('delivery sequence='))
                    original = event['message']
                    digest = original.split('sha256=', 1)[1].split(' ', 1)[0]
                    event['message'] = original.replace(digest, '0' * 64)
                    files['reboot/journal.jsonl'] = files['reboot/journal.jsonl'].replace(original.encode(), event['message'].encode())
                elif variant == 'journal-incarnation':
                    files['reboot/journal.jsonl'] = files['reboot/journal.jsonl'].replace(('2' * 32).encode(), ('f' * 32).encode())
                elif variant == 'credential': files['reboot/reboot-credential.json']['files']['/etc/fabrico11y/node-token']['stat'][1] = 42
                elif variant == 'candidate': files['drain/drain-snapshot.json']['candidate_process']['exe_sha256'] = 'b' * 64
                elif variant == 'snapshot-root': files['prepare/before-receipt.json']['spool_root'] = '/var/lib/filtered'
                bundle = self.bundle(files)
                with self.assertRaisesRegex(ValueError, message):
                    self.admit_transfer(bundle, admission, native.sha(bundle.read_bytes()))
            self.reset_fixture()

    def test_source_freeze_refuses_dirty_or_replaced_source_before_package_access(self):
        owner = self.root / 'owner.json'; owner.write_text(json.dumps(dict(harness_commit='a' * 40,
            source_hashes={}, cutoff_helper=self.pinned_helper(self.root, suffix='source-freeze'))))
        source_root = Path(native.__file__).resolve().parents[3]
        for dirty in (True, False):
            with self.subTest(dirty=dirty), patch.object(native.subprocess, 'check_output', side_effect=lambda argv, **kwargs:
                ' M source' if dirty and argv[-2:] == ['status', '--porcelain'] else '' if argv[-2:] == ['status', '--porcelain'] else 'a' * 40), \
                    patch.object(native.subprocess, 'run'):
                with self.assertRaisesRegex(ValueError, 'clean harness source|producer/helper source'):
                    native.host_admission(owner, source_root, self.root, self.root / 'package.deb', 'a' * 40, 'literal', 'debian')

    def candidate_admission_fixture(self, name, mutation):
        """Build coherent temporary owner inputs; mutate one actual admission join."""
        storage = self.root / name
        debdir = storage / 'results/installation-package-build/certificate-distrust-01-deb-a'
        rpmdir = storage / 'results/packages/certificate-distrust-01-rpm-a'
        admission = storage / 'results/candidate-build-certificate-distrust-01/artifact-admission.json'
        inspection = storage / 'results/candidate-build-certificate-distrust-01/package-inspection.json'
        console = storage / 'results/console-certificate-distrust-01-a/build.json'
        source_root = self.root / (name + '-source')
        (source_root / 'tools/qualification/install').mkdir(parents=True)
        for filename in ('custody_native.py', 'custody_witness.py', 'lifecycle.sh', 'run-qemu.py', 'acceptance.sh', 'removal_observer.py'):
            (source_root / 'tools/qualification/install' / filename).write_text(filename)
        (source_root / 'tools/qualification/query_oracle.py').write_text('literal oracle')
        source_hashes = {str(path.relative_to(source_root)): native.sha(path.read_bytes())
            for path in [*(source_root / 'tools/qualification/install' / filename for filename in
                ('custody_native.py', 'custody_witness.py', 'lifecycle.sh', 'run-qemu.py', 'acceptance.sh', 'removal_observer.py')),
                source_root / 'tools/qualification/query_oracle.py']}
        manifest_bytes, bundle_bytes, console_bytes = b'literal source manifest', b'literal source bundle', b'literal console receipt'
        inspection_bytes = b'literal package inspection'
        source = dict(original_head='1eaf9ad17dd5507809cb5cca0964e6e84758648a',
            source_commit='531e67cbf678af483debab7ea0c0f7ecf6b79686',
            manifest_sha256=native.sha(manifest_bytes), source_bundle_sha256=native.sha(bundle_bytes))
        if mutation == 'candidate-source':
            source['original_head'] = 'e078ba23b543126011a2136dd1f7e98ca65370eb'
        helperdir = debdir / 'qualification-helpers'
        helperdir.mkdir(parents=True)
        helper_files = {}
        for filename in ('spool_dump', 'server_dump', 'spindle_sim'):
            payload = ('literal ' + filename).encode()
            (helperdir / filename).write_bytes(payload)
            helper_files[filename] = native.sha(payload)
        manifest = dict(source=source, files=helper_files)
        (helperdir / 'manifest.json').write_text(json.dumps(manifest, sort_keys=True))
        (debdir / 'provenance').mkdir()
        (debdir / 'provenance/source-manifest.json').write_bytes(manifest_bytes)
        (debdir / 'provenance/frozen-source.bundle').write_bytes(bundle_bytes)
        console.parent.mkdir(parents=True)
        console.write_bytes(console_bytes)
        admission.parent.mkdir(parents=True)
        admission.write_bytes(b'literal candidate admission')
        inspection.write_bytes(inspection_bytes)
        debpkg = debdir / 'candidate.deb'; debpkg.write_bytes(b'literal deb package')
        rpmpkg = rpmdir / 'candidate.rpm'; rpmpkg.parent.mkdir(parents=True); rpmpkg.write_bytes(b'literal rpm package')
        common = dict(source=source, qualification_helpers=manifest)
        deb = dict(common, package=str(debpkg), package_sha256=native.sha(debpkg.read_bytes()),
            source_commit='531e67cbf678af483debab7ea0c0f7ecf6b79686',
            console_receipt_sha256=native.sha(console_bytes))
        rpm = dict(common, package=str(rpmpkg), package_sha256=native.sha(rpmpkg.read_bytes()),
            payload_identity={'usr/bin/fabric-node': {'sha256': 'd' * 64}})
        deb_receipt, rpm_receipt = debdir / 'receipt.json', rpmdir / 'receipt.json'
        deb_receipt.write_text(json.dumps(deb, sort_keys=True)); rpm_receipt.write_text(json.dumps(rpm, sort_keys=True))
        fixed_fixture_digests = {
            deb_receipt.read_bytes(): '0b304acefd1dab014ae547f7c9767aba3dec05ff363a540e8b941b381b79627c',
            rpm_receipt.read_bytes(): '0d11fde82ce12a3de06da9f516a6f651ebaf1597aeaa990ec18fdd0c6bf28721',
            admission.read_bytes(): 'ed4219351dbaceef6c143b93a1bea662d17f10053cc8da549fcd63465f00d4ec',
            inspection_bytes: '27b6ae8820d5a4c0f5f3c90a15e203faee19de8828d7882db85d8bcf7c261031',
        }
        if mutation == 'admission': admission.write_bytes(b'changed candidate admission')
        elif mutation == 'receipt': deb_receipt.write_bytes(deb_receipt.read_bytes() + b'changed')
        elif mutation == 'package': debpkg.write_bytes(b'changed package bytes')
        elif mutation == 'helper': (helperdir / 'spool_dump').write_bytes(b'changed helper bytes')
        elif mutation == 'inspection-missing': inspection.unlink()
        elif mutation == 'inspection-tampered': inspection.write_bytes(inspection_bytes + b' changed')
        owner = self.root / (name + '-owner.json')
        owner.write_text(json.dumps(dict(harness_commit='a' * 40, source_hashes=source_hashes,
            cutoff_helper=self.pinned_helper(storage, suffix=name))))
        return storage, owner, source_root, debpkg, fixed_fixture_digests

    def test_candidate_identity_and_receipt_mismatch_controls(self):
        # Git is stubbed only to isolate the actual host_admission tuple checks;
        # this is not evidence that a pre-merge source passes ancestry admission.
        cases = (('admission', 'owner package/admission receipt mismatch'),
                 ('receipt', 'owner package/admission receipt mismatch'),
                 ('candidate-source', 'candidate frozen/source mismatch'),
                 ('source-argument', 'candidate frozen/source mismatch'),
                 ('package', 'canonical package mismatch'),
                 ('helper', 'canonical helper binary mismatch'),
                 ('package-argument', 'noncanonical candidate path'))
        for mutation, error in cases:
            with self.subTest(mutation=mutation):
                storage, owner, source_root, package, fixed_fixture_digests = self.candidate_admission_fixture('candidate-' + mutation, mutation)
                def git_output(argv, **kwargs):
                    if argv[-2:] == ['status', '--porcelain']: return ''
                    return 'a' * 40
                def fixture_sha(raw):
                    return fixed_fixture_digests.get(raw, hashlib.sha256(raw).hexdigest())
                requested_package = storage / 'not-the-canonical-package' if mutation == 'package-argument' else package
                requested_source = 'c' * 40 if mutation == 'source-argument' else '531e67cbf678af483debab7ea0c0f7ecf6b79686'
                with patch.object(native, 'sha', side_effect=fixture_sha), \
                     patch.object(native.subprocess, 'check_output', side_effect=git_output), \
                     patch.object(native.subprocess, 'run'):
                    with self.assertRaisesRegex(ValueError, error):
                        native.host_admission(owner, source_root, storage, requested_package,
                            requested_source, 'literal', 'debian')

    def test_package_inspection_is_required_and_hash_pinned(self):
        # The real preboot package inspection is fixed at
        # results/candidate-build-certificate-distrust-01/package-inspection.json
        # (SHA-256 27b6ae8820d5a4c0f5f3c90a15e203faee19de8828d7882db85d8bcf7c261031).
        # Both absence and byte substitution must fail before candidate use.
        for mutation in ('inspection-missing', 'inspection-tampered'):
            with self.subTest(mutation=mutation):
                storage, owner, source_root, package, fixed_fixture_digests = self.candidate_admission_fixture(
                    'candidate-' + mutation, mutation)
                def git_output(argv, **kwargs):
                    if argv[-2:] == ['status', '--porcelain']: return ''
                    return 'a' * 40
                def fixture_sha(raw):
                    return fixed_fixture_digests.get(raw, hashlib.sha256(raw).hexdigest())
                with patch.object(native, 'sha', side_effect=fixture_sha), \
                     patch.object(native.subprocess, 'check_output', side_effect=git_output), \
                     patch.object(native.subprocess, 'run'):
                    if mutation == 'inspection-missing':
                        with self.assertRaises(FileNotFoundError):
                            native.host_admission(owner, source_root, storage, package,
                                '531e67cbf678af483debab7ea0c0f7ecf6b79686', 'literal', 'debian')
                    else:
                        with self.assertRaisesRegex(ValueError, 'owner package/admission receipt mismatch'):
                            native.host_admission(owner, source_root, storage, package,
                                '531e67cbf678af483debab7ea0c0f7ecf6b79686', 'literal', 'debian')


    def test_btrfs_reboot_device_is_separate_from_immutable_credential_metadata(self):
        for changed_namespace in (False, True):
            with self.subTest(changed_namespace=changed_namespace):
                files, admission = self.fixture_files()
                for phase in ('prepare', 'installed', 'drain', 'reboot'):
                    name = 'before' if phase == 'prepare' else phase
                    item = files[phase + '/' + name + '-credential.json']['files']['/etc/fabrico11y/node-token']
                    item['btrfs_identity'] = dict(uuid='01' * 16, subvolume_id=256)
                    if phase == 'reboot':
                        item['stat'][0] = 42
                        if changed_namespace: item['btrfs_identity']['subvolume_id'] = 257
                bundle = self.bundle(files)
                if changed_namespace:
                    with self.assertRaisesRegex(ValueError, 'credential metadata/hash drift'):
                        self.admit_transfer(bundle, admission, native.sha(bundle.read_bytes()))
                else:
                    self.admit_transfer(bundle, admission, native.sha(bundle.read_bytes()), preserve_on_error=True)
            self.reset_fixture()

    def test_btrfs_uapi_buffers_use_literal_kernel_layout(self):
        source = self.root / 'source'; source.write_bytes(b'literal source\n')
        guest = native.Guest.__new__(native.Guest)
        guest.command = lambda argv: dict(stdout='9123683e\n')
        requests = []
        def ioctl(descriptor, request, buffer, mutate):
            requests.append(request)
            self.assertTrue(mutate)
            if request == 0x8400941f:
                self.assertEqual(len(buffer), 1024)
                self.assertEqual(buffer, bytes(1024))
                buffer[16:32] = bytes(range(1, 17))
            elif request == 0xd0009412:
                self.assertEqual(len(buffer), 4096)
                self.assertEqual(struct.unpack_from('<QQ', buffer), (0, 256))
                struct.pack_into('<Q', buffer, 0, 257)
            else:
                self.fail('unexpected Btrfs ioctl')
        with patch.object(native.fcntl, 'ioctl', side_effect=ioctl):
            raw, _, observed = guest.read_input(str(source), 4096)
        self.assertEqual(raw, b'literal source\n')
        self.assertEqual(observed, dict(uuid=bytes(range(1, 17)).hex(), subvolume_id=257))
        self.assertEqual(requests, [0x8400941f, 0xd0009412])

    def pinned_helper(self, storage, *, payload=None, suffix='actual'):
        source = Path(os.environ['FABRIC_CUSTODY_CUTOFF_HELPER'])
        raw = source.read_bytes() if payload is None else payload
        expected = hashlib.sha256(raw).hexdigest()
        if payload is None:
            self.assertEqual(expected, os.environ['FABRIC_CUSTODY_CUTOFF_HELPER_SHA256'])
        target = storage / 'results' / f'custody-caller-helper-{os.getpid()}-{id(self)}-{suffix}'
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(raw)
        target.chmod(0o500)
        pin = dict(path=str(target), sha256=expected)
        self._standalone_helpers.append(pin)
        return pin

    def captured_metadata(self, files):
        fixture = json.loads((Path(__file__).resolve().parents[3] /
            'docs/experiments/formal/fixtures/native-cutoff-projection-input.json').read_text())
        for phase, prior_phase in (('drain', 'installed'), ('reboot', 'drain')):
            snap_name = phase + '/' + phase + '-snapshot.json'
            snapshot = native.unpack(files[snap_name])
            process = snapshot['candidate_process']
            process['calibration'] = dict(realtime_ns=400, boottime_ns=350,
                clock_ticks=100, start_ticks=10)
            process['start_ns'], process['end_ns'] = 1, 1000000
            current = copy.deepcopy(fixture)
            current.pop('classification', None)
            current['phase'] = phase
            current['candidate'] = BINDING['candidate_frozen']
            current['relative_phase_root'] = 'phase/' + phase
            revision = 2 if phase == 'drain' else 4
            current['observed_events'][1]['message'] = f'config revision={revision} paused=true interval_s=15'
            prior_snapshot = 'installed/installed-snapshot.json' if prior_phase == 'installed' else 'drain/drain-snapshot.json'
            prior_ack = native.unpack(files[prior_snapshot])['ack']
            artifact_index = 0
            for command in current['commands']:
                command['started_ns'] *= 1000
                command['finished_ns'] *= 1000
                if 'stdout_artifact' in command:
                    artifact_index += 1
                    # Synthetic stopped-inspection bytes bind this literal
                    # node/generation and the actual phase ACK prefix. The
                    # unrelated native-inspect-success artifact stays intact.
                    observed_ack = prior_ack if artifact_index == 1 else snapshot['sources'][-1]['sequence']
                    inspection = (
                        f'node_id={NODE.hex()} generation=1 batches={len(snapshot["sources"])} '
                        f'metric_points=0 log_records=0 gaps=0 otlp_payload_bytes=0 committed_bytes=0 '
                        f'file_bytes=0 coverage_unknown=false recovery_required=false interrupted_append=false '
                        f'next_sequence={snapshot["sources"][-1]["sequence"] + 1} '
                        f'acked_through={observed_ack} log_backlog_bytes=0\n').encode()
                    command['stdout_artifact']['path'] = f'inspection-{artifact_index}.txt'
                    command['stdout_artifact']['sha256'] = native.sha(inspection)
                    files[phase + '/' + command['stdout_artifact']['path']] = inspection
            for sample in current['process_observations']:
                sample['started_ns'] *= 1000
                sample['finished_ns'] *= 1000
                sample.update(unit=process['unit'], pid=process['pid'], boot_id=process['boot_id'],
                    invocation_id=process['invocation_id'], exe_sha256=process['exe_sha256'])
                sample['calibration']['start_ticks'] = 10
            events = [dict(unit=process['unit'], pid=process['pid'], boot_id=process['boot_id'],
                at_ns=50000, message='batch=4 metrics=1 logs=1 gaps=0 spool_bytes=1 log_backlog_bytes=0 acked_through=3'),
                dict(unit=process['unit'], pid=process['pid'], boot_id=process['boot_id'], at_ns=150000,
                    message=current['observed_events'][1]['message'])]
            event_time = 222000
            for source in snapshot['sources']:
                if source['sequence'] > prior_ack:
                    events.append(dict(unit=process['unit'], pid=process['pid'], boot_id=process['boot_id'],
                        at_ns=event_time, message=f"delivery sequence={source['sequence']} sha256={source['sha256']} "
                            f"status=ack committed_through={source['sequence']} elapsed_us=10"))
                    event_time += 1000
            current['observed_events'] = events
            files[phase + '/commands.json'] = json.dumps(current['commands']).encode()
            files[phase + '/process-observations.json'] = json.dumps(current['process_observations']).encode()
            files[phase + '/ack-events.json'] = events
            files[snap_name] = native.packed(snapshot)
            journal = [dict(MESSAGE=event['message'], _PID=str(process['pid']),
                _BOOT_ID=process['boot_id'].replace('-', ''), _SYSTEMD_UNIT=process['unit'],
                _SYSTEMD_INVOCATION_ID=process['invocation_id'], __MONOTONIC_TIMESTAMP='123456',
                _SYSTEMD_CGROUP='/system.slice/system-fabrico11y.slice/' + process['unit'],
                _EXE='/usr/bin/fabric-node', _UID=str(os.getuid()), _GID=str(os.getgid()),
                __REALTIME_TIMESTAMP=str(event['at_ns'] // 1000)) for event in events]
            files[phase + '/journal.jsonl'] = ''.join(json.dumps(row) + '\n' for row in journal).encode()

    def run_native_cutoff(self, files, admission, helper, result, bundle, *, preserve_on_error=False, timeout_seconds=60):
        admission['cutoff_helper'] = helper
        import shutil
        shutil.copyfile(bundle, result / 'native-bundle.tar')
        self._cutoff_results.append((result, helper))
        try:
            return native.admit_transfer(bundle, admission, native.sha(bundle.read_bytes()),
                cutoff_helper=helper, result_dir=result, data_root=Path(os.environ['FABRIC_STORAGE_ROOT']),
                deadline=time.monotonic() + timeout_seconds)
        except BaseException:
            if preserve_on_error:
                self.show_cutoff_diagnostics(result)
            raise

    def test_native_caller_runs_actual_rust_helper_after_independent_admission(self):
        storage = Path(os.environ['FABRIC_STORAGE_ROOT']).resolve()
        helper = self.pinned_helper(storage)
        result = storage / 'results' / f'custody-cutoff-positive-{os.getpid()}-{time.monotonic_ns()}'
        result.mkdir(mode=0o700)
        files, admission = self.fixture_files()
        bundle = self.bundle(files)
        observed = self.run_native_cutoff(files, admission, helper, result, bundle, preserve_on_error=True)
        self.assertEqual([item['state'] for item in observed['cutoff_outcomes']], ['completed', 'completed'])
        self.assertTrue(all(item['bundle_sha256'] == observed['bundle_sha256']
            and item['result']['status'] == 'structural-cutoff' for item in observed['cutoff_outcomes']))
        self.assertFalse((result / 'custody-cutoff-inputs').exists())
        self.assertTrue((result / 'custody-cutoff-drain.json').is_file())
        self.assertTrue((result / 'custody-cutoff-reboot.json').is_file())

    def test_native_caller_retains_body_substitution_failure(self):
        storage = Path(os.environ['FABRIC_STORAGE_ROOT']).resolve()
        helper = self.pinned_helper(storage)
        result = storage / 'results' / f'custody-cutoff-body-{os.getpid()}-{time.monotonic_ns()}'
        result.mkdir(mode=0o700)
        files, admission = self.fixture_files()
        files['drain/inspection-1.txt'] = b'substituted captured inspection'
        bundle = self.bundle(files)
        with self.assertRaisesRegex(ValueError, 'inspection archive/stdout mismatch'):
            self.run_native_cutoff(files, admission, helper, result, bundle)
        self.assertTrue((result / 'custody-cutoff-failure.json').is_file())
        # Python rejects the archive/body join before launching the Rust phase.
        self.assertFalse((result / 'custody-cutoff-drain.json').exists())
        self.assertTrue((result / 'custody-cutoff-inputs' / 'drain').is_dir())
        self.assertEqual(hashlib.sha256((result / 'custody-cutoff-inputs' / 'drain' / 'inspection-1.txt').read_bytes()).hexdigest(),
            hashlib.sha256(b'substituted captured inspection').hexdigest())

    def test_native_caller_requires_the_frozen_helper_pair(self):
        storage = Path(os.environ['FABRIC_STORAGE_ROOT']).resolve()
        files, admission = self.fixture_files()
        bundle = self.bundle(files)
        result = storage / 'results' / f'custody-cutoff-pin-{os.getpid()}-{time.monotonic_ns()}'
        result.mkdir(mode=0o700)
        helper = self.cutoff_pin()
        self._cutoff_results.append((result, helper))
        for pin in (None, dict(path=str(result / 'missing'), sha256='0' * 64)):
            with self.subTest(pin=pin), self.assertRaisesRegex(ValueError, 'frozen cutoff invocation'):
                native.admit_transfer(bundle, admission, native.sha(bundle.read_bytes()),
                    cutoff_helper=pin, result_dir=result, data_root=storage, deadline=time.monotonic() + 60)

    def test_cutoff_pin_rejects_fifo_without_blocking(self):
        storage = Path(os.environ['FABRIC_STORAGE_ROOT']).resolve()
        fifo = storage / 'results' / f'custody-cutoff-fifo-{os.getpid()}-{time.monotonic_ns()}'
        os.mkfifo(fifo, 0o500)
        owner = self.root / 'fifo-owner.json'
        owner.write_text(json.dumps({'cutoff_helper': dict(path=str(fifo), sha256='0' * 64)}))
        try:
            with self.assertRaisesRegex(ValueError, 'unsafe cutoff executable'):
                native.host_admission(owner, self.root, storage, self.root / 'package', 'a' * 40, 'literal', 'debian')
        finally:
            fifo.unlink(missing_ok=True)

    def test_host_admission_rejects_cutoff_helper_digest_mismatch(self):
        storage = Path(os.environ['FABRIC_STORAGE_ROOT']).resolve()
        helper = self.pinned_helper(storage, suffix='wrong-pin')
        owner = self.root / 'wrong-helper-owner.json'
        owner.write_text(json.dumps({'cutoff_helper': dict(path=helper['path'], sha256='0' * 64)}))
        with self.assertRaisesRegex(ValueError, 'cutoff executable changed or digest mismatch'):
            native.host_admission(owner, self.root, storage, self.root / 'package', 'a' * 40, 'literal', 'debian')

    def test_cutoff_failure_reports_nonzero_and_malformed_pinned_helpers(self):
        storage = Path(os.environ['FABRIC_STORAGE_ROOT']).resolve()
        files, admission = self.fixture_files()
        bundle = self.bundle(files)
        candidate = BINDING['candidate_frozen']
        def output(raw):
            return ("#!/bin/sh\nprintf '%s\\n' '" + raw + "'\n").encode()
        cases = (('nonzero', b'#!/bin/sh\nprintf "{}\\n"\nexit 7\n', 'Rust capture verifier refused'),
            ('malformed', b'#!/bin/sh\nprintf "not-json\\n"\nexit 0\n', 'Expecting value'),
            ('duplicate', output('{"status":"invalid","status":"structural-cutoff","candidate":"' + candidate + '"}'), 'duplicate Rust report key'),
            ('nonfinite', output('{"status":"structural-cutoff","candidate":"' + candidate + '","value":NaN}'), 'nonfinite Rust report number'),
            ('overflow-exponent', output('{"status":"structural-cutoff","candidate":"' + candidate + '","value":1e999}'), 'nonfinite Rust report number'),
            ('wrong-candidate', output('{"status":"structural-cutoff","candidate":"' + '0' * 40 + '"}'), 'Rust capture report mismatch'),
            ('oversized', b'#!/bin/sh\ndd if=/dev/zero bs=65537 count=1 2>/dev/null\n', 'Rust capture verifier refused'))
        for suffix, payload, message in cases:
            helper = self.pinned_helper(storage, payload=payload, suffix=suffix)
            result = storage / 'results' / f'custody-cutoff-{suffix}-{os.getpid()}-{time.monotonic_ns()}'
            result.mkdir(mode=0o700)
            with self.subTest(helper=suffix), self.assertRaisesRegex(ValueError, message):
                self.run_native_cutoff(files, admission, helper, result, bundle)
            self.assertTrue((result / 'custody-cutoff-failure.json').is_file())
            self.assertTrue((result / 'custody-cutoff-drain.json').is_file())
            self.assertTrue((result / 'custody-cutoff-drain.stdout').is_file())
            self.assertLessEqual((result / 'custody-cutoff-drain.stdout').stat().st_size, 65536)
            for path in result.glob('custody-cutoff-*.*'):
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)


    def cutoff_transport_fixture(self):
        storage = Path(os.environ['FABRIC_STORAGE_ROOT']).resolve()
        files, admission = self.fixture_files()
        bundle = self.bundle(files)
        candidate = BINDING['candidate_frozen']
        valid = (json.dumps(dict(status='structural-cutoff', candidate=candidate),
            separators=(',', ':')) + '\n').encode()
        return storage, files, admission, bundle, valid

    def cutoff_transport_run(self, storage, files, admission, bundle, name, script, *, timeout_seconds=60):
        helper = self.pinned_helper(storage, payload=script, suffix=name)
        result = storage / 'results' / f'custody-cutoff-transport-{name}-{os.getpid()}-{time.monotonic_ns()}'
        result.mkdir(mode=0o700)
        return helper, result, self.run_native_cutoff(files, admission, helper, result, bundle,
            timeout_seconds=timeout_seconds)

    def test_cutoff_stdout_valid_json_at_exact_limit_is_accepted(self):
        storage, files, admission, bundle, valid = self.cutoff_transport_fixture()
        padding = 65536 - len(valid)
        script = ("#!/bin/sh\nprintf '%s' " + shlex.quote(valid.decode()) + "\n"
            + "head -c " + str(padding) + " /dev/zero | tr '\\000' ' '\n").encode()
        _, result, outcomes = self.cutoff_transport_run(storage, files, admission, bundle,
            'stdout-exact-limit', script)
        self.assertEqual([row['state'] for row in outcomes['cutoff_outcomes']], ['completed', 'completed'])
        self.assertTrue(all((result / f'custody-cutoff-{phase}.stdout').stat().st_size == 65536
            for phase in ('drain', 'reboot')))

    def test_cutoff_stdout_one_byte_over_limit_is_refused(self):
        storage, files, admission, bundle, valid = self.cutoff_transport_fixture()
        padding = 65537 - len(valid)
        script = ("#!/bin/sh\nprintf '%s' " + shlex.quote(valid.decode()) + "\n"
            + "head -c " + str(padding) + " /dev/zero | tr '\\000' ' '\n").encode()
        helper = self.pinned_helper(storage, payload=script, suffix='stdout-over-limit')
        result = storage / 'results' / f'custody-cutoff-transport-over-{os.getpid()}-{time.monotonic_ns()}'
        result.mkdir(mode=0o700)
        with self.assertRaisesRegex(ValueError, 'Rust capture verifier refused'):
            self.run_native_cutoff(files, admission, helper, result, bundle)
        self.assertLessEqual((result / 'custody-cutoff-drain.stdout').stat().st_size, 65536)

    def test_cutoff_stderr_over_limit_with_valid_stdout_is_refused(self):
        storage, files, admission, bundle, valid = self.cutoff_transport_fixture()
        script = ("#!/bin/sh\nprintf '%s' " + shlex.quote(valid.decode()) + "\n"
            + "head -c 65537 /dev/zero | tr '\\000' e >&2\n").encode()
        helper = self.pinned_helper(storage, payload=script, suffix='stderr-over-limit')
        result = storage / 'results' / f'custody-cutoff-transport-stderr-{os.getpid()}-{time.monotonic_ns()}'
        result.mkdir(mode=0o700)
        with self.assertRaisesRegex(ValueError, 'Rust capture verifier refused'):
            self.run_native_cutoff(files, admission, helper, result, bundle)
        self.assertEqual((result / 'custody-cutoff-drain.stdout').read_bytes(), valid)
        self.assertLessEqual((result / 'custody-cutoff-drain.stderr').stat().st_size, 65536)

    def test_cutoff_concurrent_stdout_stderr_growth_is_bounded_without_deadlock(self):
        storage, files, admission, bundle, valid = self.cutoff_transport_fixture()
        script = ("#!/bin/sh\nprintf '%s' " + shlex.quote(valid.decode()) + "\n"
            + "(head -c 1048576 /dev/zero | tr '\\000' x) &\n"
            + "(head -c 1048576 /dev/zero | tr '\\000' e >&2) &\nwait\n").encode()
        helper = self.pinned_helper(storage, payload=script, suffix='both-streams-grow')
        result = storage / 'results' / f'custody-cutoff-transport-both-{os.getpid()}-{time.monotonic_ns()}'
        result.mkdir(mode=0o700)
        with self.assertRaisesRegex(ValueError, 'Rust capture verifier refused'):
            self.run_native_cutoff(files, admission, helper, result, bundle, timeout_seconds=5)
        self.assertLessEqual((result / 'custody-cutoff-drain.stdout').stat().st_size, 65536)
        self.assertLessEqual((result / 'custody-cutoff-drain.stderr').stat().st_size, 65536)

    def test_cutoff_valid_json_with_nonzero_helper_exit_is_refused(self):
        storage, files, admission, bundle, valid = self.cutoff_transport_fixture()
        script = ("#!/bin/sh\nprintf '%s' " + shlex.quote(valid.decode()) + "\nexit 7\n").encode()
        helper = self.pinned_helper(storage, payload=script, suffix='valid-json-nonzero')
        result = storage / 'results' / f'custody-cutoff-transport-exit-{os.getpid()}-{time.monotonic_ns()}'
        result.mkdir(mode=0o700)
        with self.assertRaisesRegex(ValueError, 'Rust capture verifier refused'):
            self.run_native_cutoff(files, admission, helper, result, bundle)
        report = json.loads((result / 'custody-cutoff-drain.json').read_bytes())
        self.assertEqual(report['exit'], 7)
        self.assertEqual((result / 'custody-cutoff-drain.stdout').read_bytes(), valid)

    def test_cutoff_timeout_retains_streams_and_reaps_helper(self):
        storage, files, admission, bundle, valid = self.cutoff_transport_fixture()
        pid_path = self.root / 'cutoff-timeout.pid'
        script = ("#!/bin/sh\nprintf '%s' " + shlex.quote(valid.decode())
            + "\nprintf 'timeout-diagnostic' >&2\nprintf '%s\\n' \"$$\" > "
            + shlex.quote(str(pid_path)) + "\nexec sleep 30\n").encode()
        helper = self.pinned_helper(storage, payload=script, suffix='timeout')
        result = storage / 'results' / f'custody-cutoff-transport-timeout-{os.getpid()}-{time.monotonic_ns()}'
        result.mkdir(mode=0o700)
        with self.assertRaises(subprocess.TimeoutExpired):
            self.run_native_cutoff(files, admission, helper, result, bundle, timeout_seconds=3)
        self.assertEqual((result / 'custody-cutoff-drain.stdout').read_bytes(), valid)
        self.assertIn(b'timeout-diagnostic', (result / 'custody-cutoff-drain.stderr').read_bytes())
        report = json.loads((result / 'custody-cutoff-drain.json').read_bytes())
        self.assertEqual(report['state'], 'failed')
        self.assertEqual(report['error'], 'TimeoutExpired')
        with self.assertRaises(ProcessLookupError):
            os.kill(int(pid_path.read_text()), 0)

    def test_cutoff_quiet_descendant_is_cleaned_after_parent_exits(self):
        storage, files, admission, bundle, valid = self.cutoff_transport_fixture()
        pid_path = self.root / 'cutoff-quiet-child.pid'
        script = ("#!/bin/sh\ncase \"$5\" in */drain/manifest.json) sleep 30 </dev/null >/dev/null 2>&1 & cat /proc/\"$!\"/stat > "
            + shlex.quote(str(pid_path)) + " ;; esac\nprintf '%s' " + shlex.quote(valid.decode()) + "\n").encode()
        try:
            _, result, outcomes = self.cutoff_transport_run(storage, files, admission, bundle,
                'quiet-descendant', script)
            self.assertEqual([row['state'] for row in outcomes['cutoff_outcomes']], ['completed', 'completed'])
            self.assertTrue(all(row['exit'] == 0 for row in outcomes['cutoff_outcomes']))
            child_pid = int(pid_path.read_text().split()[0])
            proc_stat = Path(f'/proc/{child_pid}/stat')
            until = time.monotonic() + 1
            while True:
                try:
                    state = proc_stat.read_text().split()[2]
                except FileNotFoundError:
                    break
                if state == 'Z':
                    break
                self.assertLess(time.monotonic(), until,
                    f'quiet helper descendant remains live: pid={child_pid}')
                time.sleep(0.01)
        finally:
            # Bind cleanup to the original start time and an open pidfd. A
            # successful invocation may already have reaped this descendant.
            if pid_path.exists():
                original = pid_path.read_text().split()
                child_pid = int(original[0])
                try:
                    pidfd = os.pidfd_open(child_pid)
                except ProcessLookupError:
                    pidfd = None
                if pidfd is not None:
                    try:
                        current = Path(f'/proc/{child_pid}/stat').read_text().split()
                        if current[21] == original[21] and current[2] != 'Z':
                            signal.pidfd_send_signal(pidfd, signal.SIGKILL)
                    except (FileNotFoundError, ProcessLookupError):
                        pass
                    finally:
                        os.close(pidfd)

    def test_cutoff_large_independent_side_file_does_not_limit_valid_stdout(self):
        storage, files, admission, bundle, valid = self.cutoff_transport_fixture()
        result = storage / 'results' / f'custody-cutoff-transport-side-{os.getpid()}-{time.monotonic_ns()}'
        result.mkdir(mode=0o700)
        side_file = result / 'independent-side-output.bin'
        script = ("#!/bin/sh\nset -e\ndd if=/dev/zero of=" + shlex.quote(str(side_file))
            + " bs=65537 count=1 2>/dev/null\nprintf '%s' " + shlex.quote(valid.decode()) + "\n").encode()
        helper = self.pinned_helper(storage, payload=script, suffix='side-file-over-limit')
        outcomes = self.run_native_cutoff(files, admission, helper, result, bundle)
        self.assertEqual([row['state'] for row in outcomes['cutoff_outcomes']], ['completed', 'completed'])
        self.assertEqual(side_file.stat().st_size, 65537)


class GuestObservationControls(literal.NativeFixture, unittest.TestCase):
    def observation_guest(self):
        guest = native.Guest.__new__(native.Guest)
        guest.output = self.root / 'guest-output'
        guest.output.mkdir(mode=0o700)
        guest.commands = []
        guest.process_observations = []
        guest.deadline = 10.0
        guest.config = dict(binding=dict(run_id='literal-observation'), node_sha256='a' * 64)
        return guest

    def run_command_with_clock(self, guest, argv, returncode, stdout, *, check=True):
        result = type('Completed', (), dict(returncode=returncode, stdout=stdout, stderr=''))()
        with patch.object(native.time, 'monotonic', side_effect=(1.0, 2.0, 2.25)), \
                patch.object(native.time, 'monotonic_ns', side_effect=(1000, 1250)), \
                patch.object(native.time, 'time_ns', side_effect=(10100, 10200)), \
                patch.object(native.subprocess, 'run', return_value=result):
            return guest.command(argv, check=check)

    def test_command_records_real_bounds_and_only_successful_inspect_stdout_artifact(self):
        guest = self.observation_guest()
        query = self.run_command_with_clock(guest, ['fabricctl', 'admin', 'query'], 0, '{"rows":[]}', check=True)
        failed = self.run_command_with_clock(guest, ['fabricctl', 'inspect', '/etc/fabrico11y/node.conf'],
                                             2, '', check=False)
        inspect = self.run_command_with_clock(guest, ['fabricctl', 'inspect', '/etc/fabrico11y/node.conf'],
                                              0, 'next_sequence=4\nacked_through=3\n')

        for record in (query, failed, inspect):
            self.assertEqual((record['started_ns'], record['finished_ns']), (10100, 10200))
            self.assertEqual(record['elapsed_ns'], 250)
            self.assertAlmostEqual(record['elapsed_seconds'], 0.25)
        self.assertNotIn('stdout_artifact', query)
        self.assertNotIn('stdout_artifact', failed)
        expected = b'next_sequence=4\nacked_through=3\n'
        leaf = 'command-0003-stdout.txt'
        artifact = guest.output / leaf
        self.assertEqual(artifact.read_bytes(), expected)
        self.assertEqual(stat.S_IMODE(artifact.stat().st_mode), 0o600)
        self.assertEqual(inspect['stdout'], expected.decode('utf-8'))
        self.assertEqual(inspect['stdout_artifact'], dict(path=leaf, sha256=native.sha(expected),
            encoding='utf-8-from-subprocess-text'))
        saved = json.loads((guest.output / 'commands.json').read_text())
        self.assertEqual(saved[-1], inspect)
        self.assertEqual(sorted(path.name for path in guest.output.glob('*.txt')), [leaf])

    def test_process_appends_bounded_raw_proc_observation_without_changing_return_shape(self):
        guest = self.observation_guest()
        pid = 123
        invocation = 'b' * 32
        control_group = '/system.slice/system-fabrico11y.slice/' + native.UNITS[1]
        fields = ['S'] + ['0'] * 18 + ['17']
        proc_stat = str(pid) + ' (fabric-node) ' + ' '.join(fields) + '\n'
        status = 'Uid:\t1000\t1000\t1000\t1000\nGid:\t1001\t1001\t1001\t1001\n'
        cgroup = '0::' + control_group + '\n'
        paths = {
            '/proc/sys/kernel/random/boot_id': '00000000-0000-4000-8000-000000000001\n',
            '/proc/123/stat': proc_stat,
            '/proc/123/status': status,
            '/proc/123/cgroup': cgroup,
        }

        class ProcPath:
            def __init__(self, value): self.value = str(value)
            def __truediv__(self, leaf): return ProcPath(self.value + '/' + str(leaf))
            def __str__(self): return self.value
            def read_text(self): return paths[self.value]

        real_path = Path
        def selected_path(value):
            return ProcPath(value) if str(value).startswith('/proc') else real_path(value)

        show = dict(stdout=f'MainPID={pid}\nControlGroup={control_group}\nInvocationID={invocation}\n',
                    stderr='', exit=0)
        def command(_guest, argv, check=True):
            if argv[0] == 'systemctl':
                return show
            return dict(stdout='a' * 64 + '  /proc/123/exe\n', stderr='', exit=0)

        identity = type('Identity', (), dict(pw_uid=1000, pw_gid=1001))()
        with patch.object(native, 'Path', side_effect=selected_path), \
                patch.object(native.Guest, 'command', new=command), \
                patch.object(native.pwd, 'getpwnam', return_value=identity), \
                patch.object(native.os, 'sysconf', return_value=100), \
                patch.object(native.time, 'time_ns', side_effect=(1000, 2000, 3000, 4000, 5000, 6000)), \
                patch.object(native.time, 'clock_gettime_ns', return_value=500):
            process = guest.process()
            repeated = guest.process()

        self.assertEqual(process['proc_stat'], proc_stat)
        self.assertEqual(process['status'], status)
        self.assertEqual(process['show'], show)
        self.assertEqual(process['calibration'], dict(realtime_ns=2000, boottime_ns=500,
            clock_ticks=100, start_ticks=17))
        self.assertEqual(repeated['pid'], process['pid'])
        self.assertNotIn('cgroup', process)  # Existing caller record shape remains stable.
        saved = json.loads((guest.output / 'process-observations.json').read_text())
        self.assertEqual(len(saved), 2)
        self.assertEqual((saved[0]['started_ns'], saved[0]['finished_ns']), (1000, 3000))
        self.assertEqual(saved[0]['proc_stat'], proc_stat)
        self.assertEqual(saved[0]['status'], status)
        self.assertEqual(saved[0]['cgroup'], cgroup)
        self.assertEqual(saved[0]['show'], show)
        self.assertEqual(saved[0]['calibration'], process['calibration'])
        for key, value in process.items():
            self.assertEqual(saved[0][key], value)
        self.assertEqual(saved[0]['unit'], native.UNITS[1])
        self.assertEqual(saved[0]['boot_id'], '00000000-0000-4000-8000-000000000001')
        self.assertEqual(saved[0]['exe_sha256'], 'a' * 64)
        self.assertEqual((saved[1]['started_ns'], saved[1]['finished_ns']), (4000, 6000))
        for key, value in repeated.items():
            self.assertEqual(saved[1][key], value)
        self.assertEqual(saved[1]['unit'], native.UNITS[1])
        self.assertEqual(saved[1]['boot_id'], '00000000-0000-4000-8000-000000000001')
        self.assertEqual(saved[1]['exe_sha256'], 'a' * 64)
        self.assertEqual(saved[1]['cgroup'], cgroup)


class RawSenderControls(unittest.TestCase):
    """Origin: raw-sender-red-01 missed twelve Batch/config field substitutions."""
    process=dict(pid=42,unit='fabrico11y-node.service',invocation_id='1'*32,
        status='Uid: 1000 1000 1000 1000\nGid: 1000 1000 1000 1000\n')
    event=dict(_PID='42',_SYSTEMD_UNIT='fabrico11y-node.service',_EXE='/usr/bin/fabric-node',
        _SYSTEMD_INVOCATION_ID='1'*32,_SYSTEMD_CGROUP='/system.slice/system-fabrico11y.slice/fabrico11y-node.service',
        _UID='1000',_GID='1000',_BOOT_ID='00112233445566778899aabbccddeeff',__REALTIME_TIMESTAMP='1000')
    messages=['batch=1 metrics=0 logs=0 gaps=0 spool_bytes=0 log_backlog_bytes=0 acked_through=0',
        'config revision=2 paused=true interval_s=15',
        'delivery sequence=1 sha256='+'ab'*32+' status=ack committed_through=1 elapsed_us=10']

    def test_valid_record_shapes_remain_unchanged(self):
        for message in self.messages:
            rows=native.observed_events(json.dumps(dict(self.event,MESSAGE=message)),self.process)
            self.assertEqual(rows,[dict(unit=self.process['unit'],pid=42,
                boot_id='00112233-4455-6677-8899-aabbccddeeff',at_ns=1000000,message=message)])

    def test_each_recognized_message_requires_all_sender_joins(self):
        for message in self.messages:
            for key,bad in dict(_SYSTEMD_UNIT='wrong.service',_EXE='/usr/bin/other',
                    _SYSTEMD_INVOCATION_ID='2'*32,_SYSTEMD_CGROUP='/wrong',_UID='1001',_GID='1001').items():
                with self.subTest(message=message.split()[0],field=key):
                    event=dict(self.event,MESSAGE=message);event[key]=bad
                    with self.assertRaisesRegex(ValueError,'raw journal sender mismatch'):
                        native.observed_events(json.dumps(event),self.process)
