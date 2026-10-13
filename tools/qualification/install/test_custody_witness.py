"""Prospective stopped-custody controls; fixture bytes never come from Rust."""
import copy
import hashlib
import importlib.util
import os
from pathlib import Path
import stat
import struct
import tempfile
import unittest
import zlib

ADAPTER = Path(__file__).with_name('custody_witness.py')
SOURCE = '/var/log/fabric-accept/allowed.log'
NODE = bytes(range(1, 17))
PENDING = b'accept-upgrade-pending-literal\n'
UNREAD = b'accept-upgrade-unread-literal\n'
BINDING = dict(run_id='custody-literal',
    candidate_frozen='8fb6f413179b094288316e25845c137440ba23fa',
    candidate_package_sha256='8edfc7709f4b5dc3da01c00a1d8c442470298e96f8c8e502cfb14e8c9e713e92',
    package_source_commit='73ff77867d10ef521ca985b094e1d177f7786b77',
    harness_commit='fcd57f62581a243fad34c61a258acb760cbdfce8')
PROCESS = dict(unit='fabrico11y-node.service', pid=55, boot_id='literal-boot-a',
               start_ns=100, end_ns=200, **BINDING)


def varint(value):
    out = bytearray()
    while value >= 128:
        out.append((value & 127) | 128); value >>= 7
    return bytes(out + bytes([value]))


def vi(tag, value):
    return varint(tag << 3) + varint(value)


def ld(tag, value):
    return varint(tag << 3 | 2) + varint(len(value)) + value


def export_record(record, *, resource=b'', scope=b''):
    return ld(1, (ld(1, resource) if resource else b'')
              + ld(2, (ld(1, scope) if scope else b'') + ld(2, record)))


def cursor(offset, *, prefix_crc=None, skipping=0, identity=b''):
    prefix = (PENDING + UNREAD)[:min(offset, 64)]
    return (ld(1, SOURCE.encode()) + vi(2, 7) + vi(3, 9) + vi(4, offset)
            + vi(5, skipping) + vi(6, len(prefix))
            + vi(7, zlib.crc32(prefix) if prefix_crc is None else prefix_crc)
            + (ld(8, identity) if identity else b''))


def batch(sequence=4, *, version=1, node=NODE, generation=1, body=PENDING,
          offset=None, cursor_bytes=None, logs_bytes=None):
    offset = len(PENDING) if offset is None else offset
    attrs = {'log.file.path': SOURCE, 'log.file.device': '7', 'log.file.inode': '9',
             'log.file.offset.start': str(offset - len(body)),
             'log.file.offset.end': str(offset)}
    record = b'\x59' + struct.pack('<Q', 123) + ld(5, ld(1, body[:-1]))
    for key, value in attrs.items():
        record += ld(6, ld(1, key.encode()) + ld(2, ld(1, value.encode())))
    logs = ld(1, ld(2, ld(2, record))) if logs_bytes is None else logs_bytes
    return (vi(1, version) + ld(2, node) + vi(3, generation) + vi(4, sequence)
            + ld(6, logs) + ld(7, cursor(offset) if cursor_bytes is None else cursor_bytes))


def frame(payload, at=0):
    first = b'FAB1' + struct.pack('<I', len(payload))
    marker = b'FAC1' + struct.pack('<Q', at + 16 + len(payload))
    return (first + struct.pack('<II', zlib.crc32(first), zlib.crc32(payload))
            + payload + marker + struct.pack('<I', zlib.crc32(marker)))


def adapter():
    spec = importlib.util.spec_from_file_location('custody_witness', ADAPTER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class MechanismAvailability(unittest.TestCase):
    def test_stopped_snapshot_and_conservation_are_available(self):
        self.assertTrue(ADAPTER.is_file(), 'NOT RUN: independent custody adapter is absent')
        module = adapter()
        self.assertTrue(callable(getattr(module, 'stopped_snapshot', None)))
        self.assertTrue(callable(getattr(module, 'conserve', None)))


class CustodyWitnessControls(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not ADAPTER.is_file():
            raise unittest.SkipTest('NOT RUN: independent custody adapter is absent')
        cls.module = adapter()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT'])
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        archive = self.root / 'allowed.log'; archive.write_bytes(PENDING + UNREAD)
        self.logs = {SOURCE: dict(archive_path=str(archive), device=7, inode=9,
                                 btrfs_identity=None)}

    def snapshot(self, name, payloads=None, *, ack=3, change=None, receipt_change=None):
        root = self.root / name; root.mkdir()
        payloads = [batch()] if payloads is None else payloads
        raw = b''
        for payload in payloads: raw += frame(payload, len(raw))
        (root / 'identity').write_bytes(b'FAI1' + NODE + struct.pack('<Q', 1))
        (root / 'acked').write_bytes(b'FAK1' + struct.pack('<QQ', 1, ack))
        (root / 'writer.lock').touch()
        (root / 'batches.faj').write_bytes(raw)
        receipt = dict(binding=dict(BINDING), spool_root=str(root), spool_bytes=268435456,
            stop_confirmed=True,
            boot_id='literal-boot-a', extra_project_units=[],
            units={unit: dict(active_state='inactive', query_exit=3, main_pid=0,
                cgroup_empty=True, companions=[])
                for unit in ('fabrico11y-server.service', 'fabrico11y-node.service')},
            candidate_process=copy.deepcopy(PROCESS) if name == 'drain' else None)
        if change: change(root)
        if receipt_change: receipt_change(receipt)
        def inventory():
            return {path.name: (path.lstat().st_mode,
                    str(path.readlink()) if path.is_symlink() else
                    path.read_bytes() if stat.S_ISREG(path.lstat().st_mode) else None)
                    for path in root.iterdir()}
        original = inventory()
        try:
            return self.module.stopped_snapshot(root, receipt, self.logs)
        finally:
            self.assertEqual(inventory(), original, 'snapshot reader mutated raw source')

    def chain(self):
        before = self.snapshot('before')
        installed = self.snapshot('installed')
        drained = self.snapshot('drain', [batch(), batch(5, body=UNREAD,
                                offset=len(PENDING + UNREAD))], ack=5)
        old = [dict(node='spindle-1', node_id=NODE.hex(), sequence=seq, index=0,
                    observed_ns=120, body=body, attributes={}) for seq, body in enumerate(
                    ('accept-line-before-start', 'accept-line-during-outage',
                     'accept-line-after-pressure'), 1)]
        rows = copy.deepcopy(old)
        for sequence, body, start in ((4, PENDING, 0), (5, UNREAD, len(PENDING))):
            rows.append(dict(node='spindle-1', node_id=NODE.hex(), sequence=sequence,
                index=0, observed_ns=123, body=body[:-1].decode(), attributes={
                    'log.file.path': SOURCE, 'log.file.device': '7', 'log.file.inode': '9',
                    'log.file.offset.start': str(start), 'log.file.offset.end': str(start + len(body))}))
        queries = dict(before=old, after_drain=copy.deepcopy(rows),
            after_reboot=copy.deepcopy(rows), boot_before='literal-boot-a',
            boot_after='literal-boot-b', pending_body=PENDING[:-1].decode(),
            unread_body=UNREAD[:-1].decode())
        events = [dict(unit=PROCESS['unit'], pid=55, boot_id=PROCESS['boot_id'], at_ns=150,
            message=f'delivery sequence={seq} sha256={hashlib.sha256(payload).hexdigest()} '
                    f'status=ack committed_through={seq} elapsed_us=1')
            for seq, payload in ((4, batch()), (5, batch(5, body=UNREAD,
                                                offset=len(PENDING + UNREAD))))]
        return before, installed, drained, queries, events

    def test_clean_raw_snapshot_and_two_line_conservation(self):
        chain = self.chain()
        self.assertEqual(chain[0]['node_id'], NODE.hex())
        self.assertEqual(chain[0]['ack'], 3)
        self.assertEqual(chain[0]['sources'][0]['sha256'], hashlib.sha256(batch()).hexdigest())
        self.assertEqual(chain[0]['cursors'][SOURCE]['offset'], len(PENDING))
        self.module.conserve(*chain)

    def test_archived_leading_lost_line_cannot_escape_source_conservation(self):
        # C-aud-11: valid sentinels/ACKs cannot hide an unrepresented consumed line.
        lost = b'accept-line-lost-by-reader\n'
        raw = lost + PENDING + UNREAD
        (self.root / 'allowed.log').write_bytes(raw)
        def source_cursor(offset):
            prefix = raw[:min(offset, 64)]
            return (ld(1, SOURCE.encode()) + vi(2, 7) + vi(3, 9) + vi(4, offset)
                    + vi(6, len(prefix)) + vi(7, zlib.crc32(prefix)))
        pending_end = len(lost + PENDING)
        pending = batch(offset=pending_end, cursor_bytes=source_cursor(pending_end))
        unread = batch(5, body=UNREAD, offset=len(raw), cursor_bytes=source_cursor(len(raw)))
        before = self.snapshot('before', [pending])
        installed = self.snapshot('installed', [pending])
        drained = self.snapshot('drain', [pending, unread], ack=5)
        old = [dict(node='spindle-1', node_id=NODE.hex(), sequence=sequence, index=0,
                    observed_ns=120, body=body, attributes={}) for sequence, body in enumerate(
                    ('accept-line-before-start', 'accept-line-during-outage',
                     'accept-line-after-pressure'), 1)]
        rows = copy.deepcopy(old)
        for sequence, body, start in ((4, PENDING, len(lost)), (5, UNREAD, pending_end)):
            rows.append(dict(node='spindle-1', node_id=NODE.hex(), sequence=sequence,
                index=0, observed_ns=123, body=body[:-1].decode(), attributes={
                    'log.file.path': SOURCE, 'log.file.device': '7', 'log.file.inode': '9',
                    'log.file.offset.start': str(start), 'log.file.offset.end': str(start + len(body))}))
        queries = dict(before=old, after_drain=copy.deepcopy(rows),
            after_reboot=copy.deepcopy(rows), boot_before='literal-boot-a',
            boot_after='literal-boot-b', pending_body=PENDING[:-1].decode(),
            unread_body=UNREAD[:-1].decode())
        events = [dict(unit=PROCESS['unit'], pid=55, boot_id=PROCESS['boot_id'], at_ns=150,
            message=f'delivery sequence={sequence} sha256={hashlib.sha256(payload).hexdigest()} '
                    f'status=ack committed_through={sequence} elapsed_us=1')
            for sequence, payload in ((4, pending), (5, unread))]
        self.assertEqual(before['logs'][SOURCE]['bytes'][:len(lost)], lost)
        self.assertEqual(before['cursors'][SOURCE]['offset'], pending_end)
        self.assertEqual(drained['cursors'][SOURCE]['offset'], len(raw))
        for snapshot in (before, installed, drained):
            self.assertEqual(snapshot['cursors'][SOURCE]['prefix_crc'],
                zlib.crc32(raw[:snapshot['cursors'][SOURCE]['prefix_len']]))
            self.assertFalse(any(row['body'] == lost[:-1].decode()
                for source in snapshot['sources'] for row in source['rows']))
        self.assertFalse(any(row['body'] == lost[:-1].decode()
            for observed in (queries['before'], queries['after_drain'], queries['after_reboot'])
            for row in observed))
        with self.assertRaisesRegex(ValueError, 'source prefix conservation'):
            self.module.conserve(before, installed, drained, queries, events)

    def test_complete_frame_and_envelope_refusals(self):
        wrong_wire = ld(1, b'\x01') + batch()[2:]
        variants = [batch(version=2), wrong_wire, batch(node=NODE[:-1]),
                    batch(generation=0), batch(sequence=0), batch(sequence=2**64 - 1),
                    batch(generation=2**64), batch(cursor_bytes=cursor(len(PENDING), skipping=2))]
        for index, payload in enumerate(variants):
            with self.subTest(payload=index), self.assertRaises(ValueError):
                self.snapshot(f'envelope-{index}', [payload])
        changes = [lambda root: (root / 'batches.faj').write_bytes(frame(batch())[:-1]),
                   lambda root: (root / 'batches.faj').write_bytes(frame(batch()) + b'F'),
                   lambda root: (root / 'batches.faj').write_bytes(frame(batch())[:16] + b'X' + frame(batch())[17:]),
                   lambda root: (root / 'recovery-required').write_bytes(b'failure'),
                   lambda root: (root / 'append-in-progress').write_bytes(b'interrupted'),
                   lambda root: (root / 'acked').write_bytes(b'FAK1' + struct.pack('<QQ', 1, 2**64 - 1)),
                   lambda root: (root / 'acked').write_bytes(b'FAK1' + struct.pack('<QQ', 2, 0))]
        for index, change in enumerate(changes):
            with self.subTest(frame=index), self.assertRaises(ValueError):
                self.snapshot(f'frame-{index}', change=change)

    def test_nested_otlp_structural_refusals(self):
        # Prost permits a sparse record with a valid unknown field, not malformed wire keys.
        positive = self.snapshot('otlp-unknown-valid',
            [batch(logs_bytes=ld(1, ld(2, ld(2, vi(99, 0)))))])
        self.assertEqual(positive['sources'][0]['rows'][0]['body'], '')
        variants = (
            ('field-zero', b'\x00\x00', 'protobuf (field|tag)'),
            ('field-too-large', ld(1, ld(2, ld(2, varint((1 << 29) << 3) + b'\x00'))),
             'protobuf (field|tag)'),
            ('varint-too-long', ld(1, ld(2, ld(2, varint(99 << 3) + b'\x80' * 10 + b'\x00'))),
             'varint'),
        )
        for name, logs, reason in variants:
            with self.subTest(defect=name), self.assertRaisesRegex(ValueError, reason):
                self.snapshot('otlp-' + name, [batch(logs_bytes=logs)])

    def test_generic_reader_positive_forms(self):
        start, end = varint(99 << 3 | 3), varint(99 << 3 | 4)
        variants = (vi(1, 7) + ld(99, b'\x00\xff'),
                    b'\x08\x87\x00', vi(1, 7) + vi((1 << 29) - 1, 2**64 - 1),
                    vi(1, 7) + start + ld(98, b'\x00\xff') + end,
                    vi(1, 7) + start * 100 + end * 100)
        for index, raw in enumerate(variants):
            with self.subTest(form=index):
                self.assertEqual(self.module.one(self.module.fields(raw, {1: 0}), 1), 7)

    def test_generic_reader_structural_refusals(self):
        start, end = varint(99 << 3 | 3), varint(99 << 3 | 4)
        variants = (
            ('zero', b'\x00\x00', 'protobuf (field|tag)'),
            ('large-key', varint((1 << 29) << 3) + b'\x00', 'protobuf (field|tag)'),
            ('wire', varint(99 << 3 | 6), 'wire'),
            ('long-value', varint(99 << 3) + b'\x80' * 10 + b'\x00', 'varint'),
            ('overflow', varint(99 << 3) + b'\xff' * 9 + b'\x02', 'varint|overflow'),
            ('long-length', varint(99 << 3 | 2) + b'\x80' * 10 + b'\x00', 'varint'),
            ('length', varint(99 << 3 | 2) + varint(3) + b'ab', 'length|truncat'),
            ('fixed', varint(99 << 3 | 1) + b'\x00' * 7, 'fixed|truncat'),
            ('lone-end', end, 'group'), ('unterminated', start + vi(98, 0), 'group'),
            ('mismatch', start + varint(98 << 3 | 4), 'group'),
            ('depth', start * 101 + end * 101, 'depth|recursion'),
            ('unknown-at-limit', start * 100 + vi(98, 0) + end * 100, 'depth|recursion'),
        )
        for name, raw, reason in variants:
            with self.subTest(defect=name), self.assertRaisesRegex(ValueError, reason):
                self.module.fields(raw, {})

    def test_cursor_inherits_batch_recursion_budget(self):
        # Review counterexample: Batch.cursors enters prost message depth 1, not a new root.
        start, end = varint(99 << 3 | 3), varint(99 << 3 | 4)
        for name, groups, leaf, accepted in (
                ('boundary', 99, b'', True), ('too-deep', 100, b'', False),
                ('scalar-at-limit', 99, vi(98, 0), False)):
            raw = cursor(len(PENDING)) + start * groups + leaf + end * groups
            original = batch(cursor_bytes=raw)
            with self.subTest(cursor=name):
                if accepted:
                    got = self.snapshot('cursor-depth-' + name, [original])
                    self.assertEqual(got['cursors'][SOURCE]['offset'], len(PENDING))
                    self.assertEqual(got['sources'][0]['payload'], original)
                else:
                    with self.assertRaisesRegex(ValueError, 'depth|recursion'):
                        self.snapshot('cursor-depth-' + name, [original])

    def test_btrfs_inherits_cursor_recursion_budget(self):
        # Batch.cursors.btrfs_identity enters depth 2; scalar skipping also spends budget.
        self.logs[SOURCE]['btrfs_identity'] = dict(uuid=NODE.hex(), subvolume_id=7)
        start, end = varint(99 << 3 | 3), varint(99 << 3 | 4)
        for name, groups, leaf, accepted in (
                ('boundary', 98, b'', True), ('too-deep', 99, b'', False),
                ('scalar-at-limit', 98, vi(98, 0), False)):
            identity = ld(1, NODE) + vi(2, 7) + start * groups + leaf + end * groups
            original = batch(cursor_bytes=cursor(len(PENDING), identity=identity))
            with self.subTest(btrfs=name):
                if accepted:
                    got = self.snapshot('btrfs-depth-' + name, [original])
                    self.assertEqual(got['cursors'][SOURCE]['btrfs_identity'],
                                     self.logs[SOURCE]['btrfs_identity'])
                    self.assertEqual(got['sources'][0]['payload'], original)
                else:
                    with self.assertRaisesRegex(ValueError, 'depth|recursion'):
                        self.snapshot('btrfs-depth-' + name, [original])

    def test_packed_numeric_reader_forms_and_refusals(self):
        # The reader spec names element wire-kind, not an opaque bytes-as-message guess.
        for wire, width in ((1, 8), (5, 4)):
            for raw in (varint(1 << 3 | wire) + b'\x00' * width,
                        ld(1, b'\x00' * (2 * width)), ld(1, b'')):
                with self.subTest(wire=wire, bytes=len(raw)):
                    self.assertEqual(self.module.one(self.module.fields(
                        raw + vi(2, 7), {1: ('packed', wire), 2: 0}), 2), 7)
            with self.subTest(width=width), self.assertRaisesRegex(ValueError, 'packed|width'):
                self.module.fields(ld(1, b'\x00' * (width - 1)), {1: ('packed', wire)})
        for raw in (ld(1, varint(2**64 - 1) + b'\x80\x00'), vi(1, 2**64 - 1)):
            with self.subTest(varint=raw):
                self.module.fields(raw, {1: ('packed', 0)})
        with self.assertRaisesRegex(ValueError, 'varint'):
            self.module.fields(ld(1, b'\x80' * 10 + b'\x00'), {1: ('packed', 0)})

    def test_logs_common_schema_and_utf8_refusals(self):
        variants = (
            ('record-wire', export_record(vi(3, 0)), 'wire'),
            ('body-string', export_record(ld(5, ld(1, b'\xff'))), 'UTF|utf'),
            ('event-name', export_record(ld(12, b'\xff')), 'UTF|utf'),
            ('resource-key', export_record(b'', resource=ld(1, ld(1, b'\xff'))), 'UTF|utf'),
            ('scope-name', export_record(b'', scope=ld(1, b'\xff')), 'UTF|utf'),
            ('entity-type', export_record(b'', resource=ld(3, ld(2, b'\xff'))), 'UTF|utf'),
            ('array-body', export_record(ld(5, ld(5, ld(1, ld(1, b'\xff'))))), 'UTF|utf'),
            ('kv-list', export_record(ld(5, ld(6, ld(1, ld(1, b'\xff'))))), 'UTF|utf'),
            ('scope-url', ld(1, ld(2, ld(2, b'') + ld(3, b'\xff'))), 'UTF|utf'),
            ('resource-url', ld(1, ld(2, ld(2, b'')) + ld(3, b'\xff')), 'UTF|utf'),
        )
        for name, logs, reason in variants:
            with self.subTest(defect=name), self.assertRaisesRegex(ValueError, reason):
                self.snapshot('schema-' + name, [batch(logs_bytes=logs)])

    def test_logs_common_and_message_depth_boundaries(self):
        kv = ld(1, b'key') + ld(2, ld(1, b'value')) + vi(3, 2**64 - 1)
        resource = ld(1, kv) + vi(2, 0) + ld(3, ld(1, b'url') + ld(2, b'type')
                    + ld(3, b'id') + ld(4, b'description'))
        scope = ld(1, b'scope') + ld(2, b'version') + ld(3, kv) + vi(4, 0)
        for index, value in enumerate((vi(2, 2), ld(7, b'\x00\xff'), vi(8, 2**64 - 1),
                                       ld(5, ld(1, ld(1, b'array'))), ld(6, ld(1, kv)))):
            with self.subTest(value=index):
                got = self.snapshot(f'common-{index}', [batch(logs_bytes=export_record(
                    ld(5, value), resource=resource, scope=scope))])
                self.assertEqual(got['sources'][0]['rows'][0]['body'], '')
        # Export→ResourceLogs→ScopeLogs→LogRecord→AnyValue starts depth4.
        for layers, leaf, accepted in ((48, ld(1, b'deep'), True),
                                      (49, ld(1, b'deep'), False), (48, vi(99, 0), False)):
            value = leaf
            for _ in range(layers):
                value = ld(5, ld(1, value))  # AnyValue.array→ArrayValue.values→AnyValue.
            payload = batch(logs_bytes=export_record(ld(5, value)))
            with self.subTest(layers=layers, unknown=leaf[:1]):
                if accepted:
                    self.assertEqual(self.snapshot('depth-message-positive', [payload])
                                     ['sources'][0]['rows'][0]['body'], '')
                else:
                    with self.assertRaisesRegex(ValueError, 'depth|recursion'):
                        self.snapshot(f'depth-message-{layers}-{leaf[0]}', [payload])

    def test_metrics_traces_container_scope(self):
        # Literal host fields: CPU cumulative Sum and memory Gauge; no Rust codec used.
        times = b'\x11' + struct.pack('<Q', 120) + b'\x19' + struct.pack('<Q', 123)
        gauge = ld(1, b'system.memory.total') + ld(3, b'By') + ld(5, ld(1,
                    times + b'\x31' + struct.pack('<q', 4096)))
        total = ld(1, b'system.cpu.time') + ld(3, b's') + ld(7, ld(1,
                    times + b'\x21' + struct.pack('<d', 1.25)) + vi(2, 2) + vi(3, 1))
        metrics = ld(1, ld(2, ld(2, gauge) + ld(2, total)))
        span = ld(1, b'\x01' * 16) + ld(2, b'\x02' * 8) + ld(5, b'literal-span')
        payload = batch() + ld(5, metrics) + ld(9, export_record(span))
        got = self.snapshot('host-gauge-sum-trace', [payload])
        self.assertEqual(got['sources'][0]['payload'], payload)
        self.assertEqual(got['sources'][0]['rows'][0]['body'], PENDING[:-1].decode())
        variants = (
            ('metric-export-length', 5, b'\x0a\x03ab', 'length|truncat'),
            ('trace-export-varint', 9, varint(99 << 3) + b'\x80' * 10 + b'\x00', 'varint'),
            ('metric-resource-field', 5, ld(1, b'\x00\x00'), 'protobuf (field|tag)'),
            ('trace-scope-tag', 9, ld(1, ld(2, varint((1 << 29) << 3) + b'\x00')), 'protobuf (field|tag)'),
            ('metric-record-field', 5, export_record(b'\x00\x00'), 'protobuf (field|tag)'),
            ('trace-record-field', 9, export_record(b'\x00\x00'), 'protobuf (field|tag)'),
            ('metric-container-wire', 5, vi(1, 0), 'wire'),
        )
        for name, signal, raw, reason in variants:
            with self.subTest(defect=name), self.assertRaisesRegex(ValueError, reason):
                self.snapshot(name, [batch() + ld(signal, raw)])
        # These malformed nested leaves must remain opaque to this limited witness.
        # Product ingest must independently reject them; this is not semantic admission.
        opaque = batch() + ld(5, export_record(ld(5, b'\xff'))) + ld(9, export_record(ld(11, b'\xff')))
        self.assertEqual(self.snapshot('opaque-product-leaves', [opaque])['sources'][0]['payload'], opaque)

    def test_original_payload_log_projection_binding(self):
        stamp = b'\x59' + struct.pack('<Q', 123)
        unknown = varint(99 << 3 | 3) + ld(98, b'\x00\xff') + varint(99 << 3 | 4)
        variants = (
            ('unknown-group', stamp + ld(5, ld(1, b'kept')) + unknown, 'kept', {}),
            ('last-bool', stamp + ld(5, ld(1, b'discarded') + vi(2, 1)), '', {}),
            ('last-string', stamp + ld(5, vi(2, 1) + ld(1, b'kept')), 'kept', {}),
            ('merged-body', stamp + ld(5, ld(1, b'kept')) + ld(5, vi(99, 0)), 'kept', {}),
            ('merged-value', stamp + ld(5, ld(1, b'kept')) + ld(6, ld(1, b'key')
                + ld(2, ld(1, b'value')) + ld(2, b'')), 'kept', {'key': 'value'}),
        )
        for name, record, body, attrs in variants:
            original = batch(logs_bytes=export_record(record)) + ld(99, b'\x00\xff')
            with self.subTest(projection=name):
                source = self.snapshot('projection-' + name, [original])['sources'][0]
                self.assertEqual((source['payload'], source['sha256']),
                                 (original, hashlib.sha256(original).hexdigest()))
                self.assertEqual(source['rows'], [dict(node='spindle-1', node_id=NODE.hex(),
                    sequence=4, index=0, observed_ns=123, body=body, attributes=attrs)])

    def test_full_cursor_prefix_identity_and_sequence_order(self):
        older = (vi(1, 1) + ld(2, NODE) + vi(3, 1) + vi(4, 4)
                 + ld(7, cursor(len(PENDING + UNREAD))) + ld(8, b'older cursor fixture'))
        snapshot = self.snapshot('latest', [older, batch(5)])
        self.assertEqual(snapshot['cursors'][SOURCE]['offset'], len(PENDING))
        self.assertEqual(snapshot['cursors'][SOURCE]['prefix_crc'], zlib.crc32(PENDING))
        for index, raw in enumerate((cursor(len(PENDING), prefix_crc=1),
                                     cursor(len(PENDING), skipping=1),
                                     cursor(len(PENDING), identity=ld(1, b'x') + vi(2, 1)))):
            with self.subTest(cursor=index), self.assertRaises(ValueError):
                self.snapshot(f'cursor-{index}', [batch(cursor_bytes=raw)])
        self.logs[SOURCE]['btrfs_identity'] = dict(uuid=NODE.hex(), subvolume_id=7)
        full = ld(1, NODE) + vi(2, 7)
        valid = self.snapshot('btrfs', [batch(cursor_bytes=cursor(len(PENDING), identity=full))])
        self.assertEqual(valid['cursors'][SOURCE]['btrfs_identity'], self.logs[SOURCE]['btrfs_identity'])

    def test_stop_and_path_admission_refusals(self):
        mutations = [lambda receipt: receipt.update(stop_confirmed=False),
                     lambda receipt: receipt.update(extra_project_units=['unexpected.service']),
                     lambda receipt: receipt['units'].pop('fabrico11y-node.service'),
                     lambda receipt: receipt['units']['fabrico11y-node.service'].update(main_pid=55),
                     lambda receipt: receipt['units']['fabrico11y-node.service'].update(query_exit=1)]
        for index, mutation in enumerate(mutations):
            with self.subTest(stop=index), self.assertRaises(ValueError):
                self.snapshot(f'stop-{index}', receipt_change=mutation)
        target = self.root / 'outside-identity'; target.write_bytes(b'FAI1' + NODE + struct.pack('<Q', 1))
        def symlink(root):
            (root / 'identity').unlink(); (root / 'identity').symlink_to(target)
        with self.assertRaises(ValueError): self.snapshot('symlink', change=symlink)
        self.assertEqual(target.read_bytes(), b'FAI1' + NODE + struct.pack('<Q', 1))

    def test_install_ack_query_and_reboot_conservation_refusals(self):
        chain = self.chain()
        mutations = [lambda args: args[0].update(ack=4),
                     lambda args: args[1]['binding'].update(run_id='other-run'),
                     lambda args: args[1]['binding'].update(candidate_package_sha256='00' * 32),
                     lambda args: args[1].update(node_id='00' * 16),
                     lambda args: args[1]['sources'].clear(),
                     lambda args: args[1]['sources'][0].update(sha256='00' * 32),
                     lambda args: args[1]['cursors'][SOURCE].update(offset=len(PENDING + UNREAD)),
                     lambda args: args[2].update(ack=0),
                     lambda args: args[4].clear(),
                     lambda args: args[4][0].update(pid=44),
                     lambda args: args[4][0].update(unit='other.service'),
                     lambda args: args[4][0].update(boot_id='predecessor-boot'),
                     lambda args: args[4][0].update(at_ns=99),
                     lambda args: args[4][0].update(message=args[4][0]['message'].replace(hashlib.sha256(batch()).hexdigest(), '00' * 32)),
                     lambda args: args[3]['after_drain'].append(copy.deepcopy(args[3]['after_drain'][0])),
                     lambda args: args[3]['after_reboot'][0].update(observed_ns=124),
                     lambda args: args[3].update(boot_after='literal-boot-a')]
        for index, mutation in enumerate(mutations):
            args = copy.deepcopy(chain); mutation(args)
            with self.subTest(conservation=index), self.assertRaises(ValueError):
                self.module.conserve(*args)


if __name__ == '__main__':
    unittest.main()
