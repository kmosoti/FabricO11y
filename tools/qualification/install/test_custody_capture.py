"""Literal stopped-bundle copies; no synthetic record authenticates a guest."""
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import struct
import tarfile
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
        self.assertEqual(native.admit_transfer(bundle, admission, native.sha(bundle.read_bytes()))['phases'],
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
                    native.admit_transfer(bundle,admission,native.sha(bundle.read_bytes()))
            self.temp.cleanup(); self.setUp()

    def test_transfer_digest_path_and_ownership_refusals(self):
        files, admission = self.fixture_files()
        for variant in ('digest', 'path', 'owner'):
            with self.subTest(variant=variant):
                bundle = self.bundle(files, owner=55 if variant == 'owner' else 0, unsafe=variant == 'path')
                expected = '0' * 64 if variant == 'digest' else native.sha(bundle.read_bytes())
                with self.assertRaisesRegex(ValueError, 'transfer digest|transfer path/ownership'):
                    native.admit_transfer(bundle, admission, expected)

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
                    event = files['reboot/ack-events.json'][0]
                    original = event['message']; event['message'] = original.replace(original.split('sha256=')[1][:64], '0' * 64)
                    files['reboot/journal.jsonl'] = files['reboot/journal.jsonl'].replace(original.encode(), event['message'].encode())
                elif variant == 'journal-incarnation':
                    files['reboot/journal.jsonl'] = files['reboot/journal.jsonl'].replace(('2' * 32).encode(), ('f' * 32).encode())
                elif variant == 'credential': files['reboot/reboot-credential.json']['files']['/etc/fabrico11y/node-token']['stat'][1] = 42
                elif variant == 'candidate': files['drain/drain-snapshot.json']['candidate_process']['exe_sha256'] = 'b' * 64
                elif variant == 'snapshot-root': files['prepare/before-receipt.json']['spool_root'] = '/var/lib/filtered'
                bundle = self.bundle(files)
                with self.assertRaisesRegex(ValueError, message):
                    native.admit_transfer(bundle, admission, native.sha(bundle.read_bytes()))
            self.temp.cleanup(); self.setUp()

    def test_source_freeze_refuses_dirty_or_replaced_source_before_package_access(self):
        owner = self.root / 'owner.json'; owner.write_text(json.dumps(dict(harness_commit='a' * 40, source_hashes={})))
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
        debdir = storage / 'results/installation-package-build/query-gap-01-deb-a'
        rpmdir = storage / 'results/packages/query-gap-01-rpm-a'
        admission = storage / 'results/candidate-build-query-gap-01/candidate-admission.json'
        inspection = storage / 'results/candidate-build-query-gap-01/package-inspection.json'
        console = storage / 'results/console-query-gap-01-a/build.json'
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
        source = dict(original_head='b1e5874fbf9c6feb611d98ed40386ac44a703fe1',
            source_commit='43150dbd5cff9e0bdb0082997e796978c659d8af',
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
            source_commit='43150dbd5cff9e0bdb0082997e796978c659d8af',
            console_receipt_sha256=native.sha(console_bytes))
        rpm = dict(common, package=str(rpmpkg), package_sha256=native.sha(rpmpkg.read_bytes()),
            payload_identity={'usr/bin/fabric-node': {'sha256': 'd' * 64}})
        deb_receipt, rpm_receipt = debdir / 'receipt.json', rpmdir / 'receipt.json'
        deb_receipt.write_text(json.dumps(deb, sort_keys=True)); rpm_receipt.write_text(json.dumps(rpm, sort_keys=True))
        fixed_fixture_digests = {
            deb_receipt.read_bytes(): '89ce427d82a0065ca71e033aa696708e85c338d063d4a75cf4e4ba263f53fe72',
            rpm_receipt.read_bytes(): '106b834855851b604d708228493192305327ff8236de3add9f4cf25c1c830919',
            admission.read_bytes(): '9c83d196a5ccf85e00d3e9becc79032f3fd57d7aab64a396690803bef841aa5a',
            inspection_bytes: 'ad3c597a10d9841548165c47ddd7d504095db94063223e1f93a73a0d887d30b8',
        }
        if mutation == 'admission': admission.write_bytes(b'changed candidate admission')
        elif mutation == 'receipt': deb_receipt.write_bytes(deb_receipt.read_bytes() + b'changed')
        elif mutation == 'package': debpkg.write_bytes(b'changed package bytes')
        elif mutation == 'helper': (helperdir / 'spool_dump').write_bytes(b'changed helper bytes')
        elif mutation == 'inspection-missing': inspection.unlink()
        elif mutation == 'inspection-tampered': inspection.write_bytes(inspection_bytes + b' changed')
        owner = self.root / (name + '-owner.json')
        owner.write_text(json.dumps(dict(harness_commit='a' * 40, source_hashes=source_hashes)))
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
                requested_source = 'c' * 40 if mutation == 'source-argument' else '43150dbd5cff9e0bdb0082997e796978c659d8af'
                with patch.object(native, 'sha', side_effect=fixture_sha), \
                     patch.object(native.subprocess, 'check_output', side_effect=git_output), \
                     patch.object(native.subprocess, 'run'):
                    with self.assertRaisesRegex(ValueError, error):
                        native.host_admission(owner, source_root, storage, requested_package,
                            requested_source, 'literal', 'debian')

    def test_package_inspection_is_required_and_hash_pinned(self):
        # The real preboot package inspection is fixed at
        # results/candidate-build-query-gap-01/package-inspection.json
        # (SHA-256 ad3c597a10d9841548165c47ddd7d504095db94063223e1f93a73a0d887d30b8).
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
                                '43150dbd5cff9e0bdb0082997e796978c659d8af', 'literal', 'debian')
                    else:
                        with self.assertRaisesRegex(ValueError, 'owner package/admission receipt mismatch'):
                            native.host_admission(owner, source_root, storage, package,
                                '43150dbd5cff9e0bdb0082997e796978c659d8af', 'literal', 'debian')


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
                        native.admit_transfer(bundle, admission, native.sha(bundle.read_bytes()))
                else:
                    native.admit_transfer(bundle, admission, native.sha(bundle.read_bytes()))
            self.temp.cleanup(); self.setUp()

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
