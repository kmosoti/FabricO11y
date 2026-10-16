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

    def fixture_files(self):
        chain, reboot, reboot_events = self.reboot_chain()
        before, installed, drain, queries, events = chain
        boots = {'literal-boot-a': '00000000-0000-4000-8000-000000000001',
                 'literal-boot-b': '00000000-0000-4000-8000-000000000002'}
        files = {}
        snapshots = dict(prepare=before, installed=installed, drain=drain, reboot=reboot)
        for phase, snapshot in snapshots.items():
            name = 'before' if phase == 'prepare' else phase
            snapshot['boot_id'] = boots[snapshot['boot_id']]
            process = snapshot['candidate_process']
            if process:
                process['boot_id'] = boots[process['boot_id']]
                process['start_ns'] *= 1000; process['end_ns'] *= 1000
                process['invocation_id'] = ('1' if phase == 'drain' else '2') * 32
                process.update(exe_sha256='a' * 64, status=f'Uid:\t{os.getuid()}\nGid:\t{os.getgid()}\n',
                    show=dict(stdout=f"MainPID={process['pid']}\nInvocationID={process['invocation_id']}\nControlGroup=/system.slice/system-fabrico11y.slice/{native.UNITS[1]}\n"))
            receipt = dict(binding=BINDING, spool_root=str(native.ROOT), boot_id=snapshot['boot_id'],
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
                'acked': b'FAK1' + struct.pack('<QQ', 1, snapshot['ack']), 'writer.lock': b'', 'batches.faj': b''}
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
