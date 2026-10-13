"""C-092 native-shaped inputs, hand-encoded independently of the product.

Temporary trees are synthetic controls, never native guest receipts or Spools.
"""
import copy
import hashlib
import inspect
import os
from pathlib import Path
import stat
import struct
import tempfile
import unittest
import zlib

from test_custody_witness import BINDING, NODE, PROCESS, SOURCE, adapter, frame, ld, vi

DIAG = '/var/lib/fabrico11y/node/spool/diagnostics/spindle.log'
OLD = (b'accept-line-before-start\n', b'accept-line-during-outage\n',
       b'accept-line-after-pressure\n')
PENDING = b'accept-upgrade-pending-literal\n'
UNREAD = b'accept-upgrade-unread-literal\n'
DIAG_OLD, DIAG_NEW = b'event starting\n', b'event sample\n'
STOP = b'event stopping\n'
CAP = 262144


def literal_row(sequence, index, path, device, inode, start, line):
    return dict(node='spindle-1', node_id=NODE.hex(), sequence=sequence, index=index,
        observed_ns=123, body=line[:-1].decode(), attributes={
            'log.file.path': path, 'log.file.device': str(device),
            'log.file.inode': str(inode), 'log.file.offset.start': str(start),
            'log.file.offset.end': str(start + len(line))})


def record(row):
    raw = b'\x59' + struct.pack('<Q', 123) + ld(5, ld(1, row['body'].encode()))
    return raw + b''.join(ld(6, ld(1, key.encode()) + ld(2, ld(1, value.encode())))
                         for key, value in row['attributes'].items())


def malformed_diagnostics():
    raw = record(literal_row(4, 1, DIAG, 7, 19, 0, DIAG_OLD))
    return (('wire', raw + vi(3, 0), 'wire'),
            ('UTF8', raw + ld(5, ld(1, b'\xff')), 'UTF|utf'))


def source_cursor(path, device, inode, raw, offset):
    prefix = raw[:min(offset, 64)]
    return (ld(1, path.encode()) + vi(2, device) + vi(3, inode) + vi(4, offset)
            + vi(6, len(prefix)) + vi(7, zlib.crc32(prefix)))


def payload(sequence, rows, cursors):
    logs = ld(1, ld(2, b''.join(ld(2, record(row)) for row in rows)))
    return (vi(1, 1) + ld(2, NODE) + vi(3, 1) + vi(4, sequence) + ld(6, logs)
            + b''.join(ld(7, current) for current in cursors))


class NativeFixture:
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT'])
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.spool = self.root / 'spool'; self.spool.mkdir(mode=0o700)
        self.diagnostics = self.spool / 'diagnostics'; self.diagnostics.mkdir(mode=0o700)
        self.active = self.diagnostics / 'spindle.log'
        self.active.write_bytes(DIAG_OLD); self.active.chmod(0o600)
        (self.spool / 'identity').write_bytes(b'FAI1' + NODE + struct.pack('<Q', 1))
        (self.spool / 'writer.lock').touch()

    def snapshot(self, phase, payloads, ack, allowed, *, fixture_paths=(SOURCE,), receipt_change=None):
        raw = b''
        for item in payloads: raw += frame(item, len(raw))
        (self.spool / 'batches.faj').write_bytes(raw)
        (self.spool / 'acked').write_bytes(b'FAK1' + struct.pack('<QQ', 1, ack))
        logs = {}
        for index, (path, content, device, inode) in enumerate((
                (SOURCE, allowed, 7, 9),
                (DIAG, self.active.read_bytes(), self.active.stat().st_dev, self.active.stat().st_ino))):
            archive = self.root / f'{phase}-{index}.log'; archive.write_bytes(content)
            logs[path] = dict(archive_path=str(archive), device=device, inode=inode,
                              btrfs_identity=None)
        receipt = dict(binding=dict(BINDING), spool_root=str(self.spool),
            spool_bytes=268435456, stop_confirmed=True, boot_id='literal-boot-a',
            extra_project_units=[], units={unit: dict(active_state='inactive', query_exit=3,
                main_pid=0, cgroup_empty=True, companions=[])
                for unit in ('fabrico11y-server.service', 'fabrico11y-node.service')},
            service_identity=dict(uid=os.geteuid(), gid=os.getegid()),
            candidate_process=copy.deepcopy(PROCESS) if phase == 'drain' else None)
        if receipt_change: receipt_change(receipt)
        def inventory():
            return {str(path.relative_to(self.root)): (path.lstat().st_mode,
                str(path.readlink()) if path.is_symlink() else
                path.read_bytes() if stat.S_ISREG(path.lstat().st_mode) else None)
                for path in self.root.rglob('*')}
        original = inventory()
        try:
            if 'fixture_paths' not in inspect.signature(self.module.stopped_snapshot).parameters:
                return self.module.stopped_snapshot(self.spool, receipt, logs)
            observed = self.module.stopped_snapshot(self.spool, receipt, logs, fixture_paths=fixture_paths)
            self.assertEqual([source['payload'] for source in observed['sources']], payloads,
                             'snapshot replaced or dropped an original payload')
            self.assertEqual([source['sha256'] for source in observed['sources']],
                             [hashlib.sha256(item).hexdigest() for item in payloads])
            return observed
        finally:
            self.assertEqual(inventory(), original, 'reader mutated the full raw input')

    def chain(self, *, rotate=False, rotation_full=True, rotated_old=None,
              rewritten=False, lost_prefix=False, diag_line=DIAG_NEW, diag_only_pending=False):
        lost = b'accept-line-lost-by-reader\n' if lost_prefix else b''
        allowed = lost + b''.join(OLD) + PENDING + UNREAD
        prefix = b''.join(OLD)
        initial = DIAG_OLD + (b'event prior-sample\n' if diag_only_pending else b'')
        if rotate:
            if rotation_full:
                padding = CAP - len(initial)
                initial = b'x\n' * (padding // 2) + (b'\n' if padding % 2 else b'') + initial
            previous = self.diagnostics / 'spindle.log.1'
            previous.write_bytes(b'event previous-generation\n'); previous.chmod(0o600)
        self.active.write_bytes(initial)
        device, inode = self.active.stat().st_dev, self.active.stat().st_ino
        cursor_a = lambda offset: source_cursor(SOURCE, 7, 9, allowed, offset)
        cursor_d = lambda offset: source_cursor(DIAG, device, inode, initial, offset)
        payloads, old_rows, offset = [], [], len(lost)
        for sequence, line in enumerate(OLD, 1):
            row = literal_row(sequence, 0, SOURCE, 7, 9, offset, line)
            old_rows.append(row); offset += len(line)
            payloads.append(payload(sequence, [row], [cursor_a(offset)]))
        pending_end = len(lost + prefix + PENDING)
        diag_start = len(initial) - len(DIAG_OLD) - (
            len(b'event prior-sample\n') if diag_only_pending else 0)
        pending_rows = [literal_row(4, 0, SOURCE, 7, 9, len(lost + prefix), PENDING),
                       literal_row(4, 1, DIAG, device, inode, diag_start, DIAG_OLD)]
        payloads.append(payload(4, pending_rows, [cursor_a(pending_end), cursor_d(diag_start + len(DIAG_OLD))]))
        if diag_only_pending:
            row = literal_row(5, 0, DIAG, device, inode, diag_start + len(DIAG_OLD), b'event prior-sample\n')
            pending_rows.append(row)
            payloads.append(payload(5, [row], [cursor_a(pending_end), cursor_d(len(initial))]))
        before = self.snapshot('before', payloads, 3, allowed)
        installed = self.snapshot('installed', payloads, 3, allowed)
        if rotate:
            self.active.replace(self.diagnostics / 'spindle.log.1')
            self.active.write_bytes(diag_line + STOP); self.active.chmod(0o600)
            if rotated_old is not None:
                (self.diagnostics / 'spindle.log.1').write_bytes(rotated_old)
        else:
            self.active.write_bytes((b'event rewritten\n' if rewritten else initial) + diag_line + STOP)
        after_diag = self.active.read_bytes()
        sequence = len(payloads) + 1
        unread_rows = [literal_row(sequence, 0, SOURCE, 7, 9, pending_end, UNREAD),
            literal_row(sequence, 1, DIAG, self.active.stat().st_dev, self.active.stat().st_ino,
                        len(after_diag) - len(diag_line + STOP), diag_line)]
        payloads.append(payload(sequence, unread_rows, [cursor_a(len(allowed)), source_cursor(
            DIAG, self.active.stat().st_dev, self.active.stat().st_ino, after_diag, len(after_diag) - len(STOP))]))
        drained = self.snapshot('drain', payloads, sequence, allowed)
        rows = old_rows + pending_rows + unread_rows
        self.assertEqual([row for source in drained['sources'] for row in source['rows']], rows)
        queries = dict(before=copy.deepcopy(old_rows), after_drain=copy.deepcopy(rows),
            after_reboot=copy.deepcopy(rows), boot_before='literal-boot-a', boot_after='literal-boot-b',
            pending_body=PENDING[:-1].decode(), unread_body=UNREAD[:-1].decode())
        events = [dict(unit=PROCESS['unit'], pid=55, boot_id=PROCESS['boot_id'], at_ns=150,
            message=f'delivery sequence={seq} sha256={hashlib.sha256(item).hexdigest()} '
                    f'status=ack committed_through={seq} elapsed_us=1')
            for seq, item in enumerate(payloads, 1) if seq > 3]
        return before, installed, drained, queries, events


class NativeShapeAvailability(NativeFixture, unittest.TestCase):
    def test_unfiltered_native_root_and_explicit_fixture_projection_are_supported(self):
        self.module = adapter()
        row = literal_row(4, 0, SOURCE, 7, 9, len(b''.join(OLD)), PENDING)
        raw = b''.join(OLD) + PENDING + UNREAD
        item = payload(4, [row], [source_cursor(SOURCE, 7, 9, raw, len(raw) - len(UNREAD))])
        try:
            observed = self.snapshot('availability', [item], 3, raw)
        except ValueError as error:
            self.fail(f'native reader shape unavailable (qualification harness, not product): {error}')
        self.assertEqual(observed.get('fixture_paths'), (SOURCE,), 'explicit fixture projection is absent')
        self.assertIn('diagnostics/spindle.log', observed['files'], 'full raw-root inventory is absent')


class LiteralNativeRecords(unittest.TestCase):
    def test_mixed_record_indices_and_original_payload_remain_literal(self):
        fixture = literal_row(4, 0, SOURCE, 7, 9, 0, PENDING)
        diagnostic = literal_row(4, 1, DIAG, 7, 19, 0, DIAG_OLD)
        original = payload(4, [fixture, diagnostic], [])
        self.assertEqual(adapter().envelope(original)[2], [fixture, diagnostic])
        self.assertEqual([row['index'] for row in (fixture, diagnostic)], [0, 1])
        self.assertIn(record(diagnostic), original)

    def test_malformed_diagnostic_row_cannot_hide_behind_fixture_selection(self):
        fixture = literal_row(4, 0, SOURCE, 7, 9, 0, PENDING)
        prefix = vi(1, 1) + ld(2, NODE) + vi(3, 1) + vi(4, 4)
        for name, malformed, reason in malformed_diagnostics():
            original = prefix + ld(6, ld(1, ld(2, ld(2, record(fixture)) + ld(2, malformed))))
            with self.subTest(diagnostic=name), self.assertRaisesRegex(ValueError, reason):
                adapter().envelope(original)


class NativeShapeControls(NativeFixture, unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.module = adapter()

    def setUp(self):
        if 'fixture_paths' not in inspect.signature(self.module.stopped_snapshot).parameters:
            raise unittest.SkipTest('NOT RUN: explicit native reader shape is not implemented')
        NativeFixture.setUp(self)

    def test_crc_valid_malformed_diagnostic_frame_is_rejected_before_projection(self):
        fixture = literal_row(4, 0, SOURCE, 7, 9, 0, PENDING)
        prefix = vi(1, 1) + ld(2, NODE) + vi(3, 1) + vi(4, 4)
        for name, bad, reason in malformed_diagnostics():
            original = prefix + ld(6, ld(1, ld(2, ld(2, record(fixture)) + ld(2, bad))))
            with self.subTest(diagnostic=name), self.assertRaisesRegex(ValueError, reason):
                self.snapshot('malformed', [original], 3, PENDING + UNREAD)

    def test_append_and_uncollected_stop_tail_preserve_full_authority(self):
        chain = self.chain()
        before, installed, drained, queries, _ = chain
        self.assertEqual(before['fixture_paths'], (SOURCE,))
        self.assertEqual(before['files'], installed['files'])
        self.assertEqual(before['diagnostics']['spindle.log']['bytes'], DIAG_OLD)
        self.assertEqual(drained['diagnostics']['spindle.log']['bytes'], DIAG_OLD + DIAG_NEW + STOP)
        self.assertEqual(drained['logs'][DIAG]['bytes'], DIAG_OLD + DIAG_NEW + STOP)
        self.assertEqual(drained['cursors'][DIAG]['offset'], len(DIAG_OLD + DIAG_NEW))
        self.assertEqual(len(queries['after_drain']), 7)
        self.assertEqual(sum(row['attributes']['log.file.path'] == SOURCE
                             for row in queries['after_drain']), 5)
        self.assertEqual(len(drained['sources'][-1]['rows']), 2)
        self.assertEqual(drained['sources'][3]['payload'], before['sources'][3]['payload'])
        self.assertEqual(drained['sources'][3]['sha256'], before['sources'][3]['sha256'])
        self.module.conserve(*chain)

    def test_source_path_not_body_selects_the_five_fixture_rows(self):
        chain = self.chain(diag_line=PENDING)
        self.assertEqual(sum(row['body'] == PENDING[:-1].decode()
                             for row in chain[3]['after_drain']), 2)
        self.module.conserve(*chain)

    def test_one_observed_rotation_preserves_prior_archived_inventory(self):
        chain = self.chain(rotate=True)
        before, _, drained, _, _ = chain
        self.assertEqual(len(before['diagnostics']['spindle.log']['bytes']), CAP)
        self.assertEqual(before['diagnostics']['spindle.log.1']['bytes'], b'event previous-generation\n')
        self.assertEqual(drained['diagnostics']['spindle.log.1']['bytes'],
                         before['diagnostics']['spindle.log']['bytes'])
        self.assertEqual(drained['diagnostics']['spindle.log.1']['inode'],
                         before['diagnostics']['spindle.log']['inode'])
        self.assertNotEqual(drained['diagnostics']['spindle.log']['inode'],
                            before['diagnostics']['spindle.log']['inode'])
        self.module.conserve(*chain)

    def test_rewritten_diagnostic_prefix_is_refused(self):
        with self.assertRaisesRegex(ValueError, 'diagnostic.*(prefix|rotation|preserv)'):
            self.module.conserve(*self.chain(rewritten=True))

    def test_unjustified_or_unobserved_rotation_cannot_replace_prior_prefix(self):
        for options in (dict(rotate=True, rotation_full=False),
                        dict(rotate=True, rotated_old=b'event different-generation\n')):
            with self.subTest(options=options), self.assertRaisesRegex(
                    ValueError, 'diagnostic.*(prefix|rotation|preserv)'):
                self.module.conserve(*self.chain(**options))
            self.temp.cleanup(); self.setUp()

    def test_diagnostic_only_pending_batch_still_needs_original_byte_ack(self):
        chain = self.chain(diag_only_pending=True)
        self.assertEqual(chain[0]['sources'][-1]['sequence'], 5)
        self.assertTrue(all(row['attributes']['log.file.path'] == DIAG
                            for row in chain[0]['sources'][-1]['rows']))
        self.module.conserve(*chain)
        events = [event for event in chain[-1] if not event['message'].startswith('delivery sequence=5 ')]
        with self.assertRaisesRegex(ValueError, 'missing candidate exact-byte ACK'):
            self.module.conserve(*chain[:-1], events)

    def test_fixture_prefix_gap_and_extra_fixture_query_row_are_refused(self):
        with self.assertRaisesRegex(ValueError, 'source prefix conservation'):
            self.module.conserve(*self.chain(lost_prefix=True))
        self.temp.cleanup(); self.setUp()
        chain = self.chain()
        extra = copy.deepcopy(chain[3]['after_drain'][0]); extra['body'] = 'unexpected-fixture-row'
        chain[3]['after_drain'].append(extra)
        with self.assertRaisesRegex(ValueError, 'exact row conservation'):
            self.module.conserve(*chain)

    def test_invalid_or_substituted_fixture_path_registration_is_refused(self):
        row = literal_row(4, 0, SOURCE, 7, 9, 0, PENDING)
        item = payload(4, [row], [source_cursor(SOURCE, 7, 9, PENDING + UNREAD, len(PENDING))])
        for paths in ((), [SOURCE], (SOURCE, SOURCE), ('relative.log',), ('/missing.log',)):
            with self.subTest(paths=paths), self.assertRaisesRegex(ValueError, 'fixture.*path'):
                self.snapshot('invalid', [item], 3, PENDING + UNREAD, fixture_paths=paths)
        chain = self.chain()
        chain[2]['fixture_paths'] = (DIAG,)
        with self.assertRaisesRegex(ValueError, 'fixture.*path'):
            self.module.conserve(*chain)

    def test_unexpected_aliased_or_nonprivate_diagnostic_input_is_refused(self):
        row = literal_row(4, 0, SOURCE, 7, 9, 0, PENDING)
        item = payload(4, [row], [source_cursor(SOURCE, 7, 9, PENDING + UNREAD, len(PENDING))])
        for variant in ('unexpected', 'symlink', 'hardlink', 'mode', 'size',
                        'uid', 'gid', 'uid-float', 'gid-float'):
            receipt_change = None
            with self.subTest(diagnostic=variant), self.assertRaisesRegex(ValueError, 'diagnostic'):
                if variant == 'unexpected':
                    (self.diagnostics / 'other.log').write_bytes(b'unregistered\n')
                elif variant == 'symlink':
                    outside = self.root / 'outside'; outside.write_bytes(DIAG_OLD)
                    self.active.unlink(); self.active.symlink_to(outside)
                elif variant == 'hardlink':
                    os.link(self.active, self.root / 'alias')
                elif variant == 'mode':
                    self.active.chmod(0o644)
                elif variant == 'size':
                    self.active.write_bytes(b'x' * (CAP + 1))
                else:
                    field = variant.removesuffix('-float')
                    value = os.geteuid() if field == 'uid' else os.getegid()
                    value = float(value) if variant.endswith('-float') else value + 1
                    receipt_change = lambda receipt: receipt['service_identity'].update(
                        {field: value})
                self.snapshot('unsafe', [item], 3, PENDING + UNREAD, receipt_change=receipt_change)
            self.temp.cleanup(); self.setUp()


if __name__ == '__main__':
    unittest.main()
