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

import test_custody_witness as legacy
from test_custody_witness import BINDING, NODE, PROCESS, SOURCE, adapter, frame, ld, vi

DIAG = '/var/lib/fabrico11y/node/spool/diagnostics/spindle.log'
OLD = (b'accept-line-before-start\n', b'accept-line-during-outage\n',
       b'accept-line-after-pressure\n')
PENDING = b'accept-upgrade-pending-literal\n'
UNREAD = b'accept-upgrade-unread-literal\n'
DIAG_OLD, DIAG_PENDING, DIAG_NEW = b'event starting\n', b'event pending\n', b'event sample\n'
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


def payload(sequence, rows, cursors, gaps=()):
    logs = ld(1, ld(2, b''.join(ld(2, record(row)) for row in rows)))
    return (vi(1, 1) + ld(2, NODE) + vi(3, 1) + vi(4, sequence) + ld(6, logs)
            + b''.join(ld(7, current) for current in cursors)
            + b''.join(ld(8, gap.encode()) for gap in gaps))


def signal_batch(signal, export):
    row = literal_row(4, 0, SOURCE, 7, 9, 0, PENDING)
    return payload(4, [row], []) + ld(signal, export)


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

    def snapshot(self, phase, payloads, ack, allowed, *, fixture_paths=(SOURCE,),
                 receipt_change=None, change=None, archive_change=None):
        raw = b''
        for item in payloads: raw += frame(item, len(raw))
        (self.spool / 'batches.faj').write_bytes(raw)
        (self.spool / 'acked').write_bytes(b'FAK1' + struct.pack('<QQ', 1, ack))
        logs = {}
        for index, (path, content, device, inode) in enumerate((
                (SOURCE, allowed, 7, 9),
                (DIAG, DIAG_OLD, 0, 0) if not self.active.exists() else
                (DIAG, self.active.read_bytes(), self.active.stat().st_dev, self.active.stat().st_ino))):
            archive = self.root / f'{phase}-{index}.log'; archive.write_bytes(content)
            logs[path] = dict(archive_path=str(archive), device=device, inode=inode,
                              btrfs_identity=None)
        if archive_change: archive_change(logs)
        receipt = dict(binding=dict(BINDING), spool_root=str(self.spool),
            spool_bytes=268435456, stop_confirmed=True, boot_id='literal-boot-a',
            extra_project_units=[], units={unit: dict(active_state='inactive', query_exit=3,
                main_pid=0, cgroup_empty=True, companions=[])
                for unit in ('fabrico11y-server.service', 'fabrico11y-node.service')},
            service_identity=dict(uid=os.geteuid(), gid=os.getegid()),
            candidate_process=copy.deepcopy(PROCESS) if phase == 'drain' else None)
        if receipt_change: receipt_change(receipt)
        if change: change(self.spool)
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

    def chain(self, *, rotate=False, rotation_full=True, rotation_size=None, rotated_old=None,
              rotated_replace=False, prior_old=False, old_rewrite=False, rewritten=False,
              lost_prefix=False, diag_pending=DIAG_PENDING, diag_line=DIAG_NEW,
              diag_only_pending=False, extra_pending=False, pending_gap=(), unread_gap=()):
        lost = b'accept-line-lost-by-reader\n' if lost_prefix else b''
        allowed = lost + b''.join(OLD) + PENDING + UNREAD
        prefix = b''.join(OLD)
        initial = DIAG_OLD + diag_pending + (b'event prior-sample\n' if diag_only_pending else b'')
        if rotate:
            if rotation_full:
                padding = (CAP if rotation_size is None else rotation_size) - len(initial)
                initial = b'x\n' * (padding // 2) + (b'\n' if padding % 2 else b'') + initial
        if rotate or prior_old:
            previous = self.diagnostics / 'spindle.log.1'
            previous.write_bytes(b'event previous-generation\n'); previous.chmod(0o600)
        self.active.write_bytes(initial)
        device, inode = self.active.stat().st_dev, self.active.stat().st_ino
        cursor_a = lambda offset: source_cursor(SOURCE, 7, 9, allowed, offset)
        cursor_d = lambda offset: source_cursor(DIAG, device, inode, initial, offset)
        diag_start = len(initial) - len(DIAG_OLD + diag_pending) - (
            len(b'event prior-sample\n') if diag_only_pending else 0)
        starting = literal_row(1, 0, DIAG, device, inode, diag_start, DIAG_OLD)
        payloads, old_rows, offset = [], [starting], len(lost)
        for sequence, line in enumerate(OLD, 1):
            row = literal_row(sequence, 1 if sequence == 1 else 0, SOURCE, 7, 9, offset, line)
            old_rows.append(row); offset += len(line)
            payloads.append(payload(sequence, [starting, row] if sequence == 1 else [row],
                [cursor_a(offset), cursor_d(diag_start + len(DIAG_OLD))]))
        pending_end = len(lost + prefix + PENDING)
        pending_rows = [literal_row(4, 0, SOURCE, 7, 9, len(lost + prefix), PENDING),
                       literal_row(4, 1, DIAG, device, inode, diag_start + len(DIAG_OLD), diag_pending)]
        if extra_pending:
            pending_rows.append(literal_row(4, 2, SOURCE, 7, 9, len(lost + prefix), b'extra-fixture-row\n'))
        payloads.append(payload(4, pending_rows, [cursor_a(pending_end),
            cursor_d(diag_start + len(DIAG_OLD + diag_pending))], pending_gap))
        if diag_only_pending:
            row = literal_row(5, 0, DIAG, device, inode, diag_start + len(DIAG_OLD + diag_pending), b'event prior-sample\n')
            pending_rows.append(row)
            payloads.append(payload(5, [row], [cursor_a(pending_end), cursor_d(len(initial))]))
        before = self.snapshot('before', payloads, 3, allowed)
        installed = self.snapshot('installed', payloads, 3, allowed)
        if rotate:
            self.active.replace(self.diagnostics / 'spindle.log.1')
            self.active.write_bytes(diag_line + STOP); self.active.chmod(0o600)
            if rotated_old is not None:
                (self.diagnostics / 'spindle.log.1').write_bytes(rotated_old)
            if rotated_replace:
                replacement = self.root / 'replacement.log'; replacement.write_bytes(initial)
                replacement.chmod(0o600); replacement.replace(self.diagnostics / 'spindle.log.1')
        else:
            self.active.write_bytes((b'event rewritten\n' if rewritten else initial) + diag_line + STOP)
        if old_rewrite:
            (self.diagnostics / 'spindle.log.1').write_bytes(b'event modified-old\n')
        after_diag = self.active.read_bytes()
        sequence = len(payloads) + 1
        unread_rows = [literal_row(sequence, 0, SOURCE, 7, 9, pending_end, UNREAD),
            literal_row(sequence, 1, DIAG, self.active.stat().st_dev, self.active.stat().st_ino,
                        len(after_diag) - len(diag_line + STOP), diag_line)]
        payloads.append(payload(sequence, unread_rows, [cursor_a(len(allowed)), source_cursor(
            DIAG, self.active.stat().st_dev, self.active.stat().st_ino, after_diag,
            len(after_diag) - len(STOP))], unread_gap))
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
        try:
            observed = self.chain()[0]
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

    def test_query_oracle_stays_byte_identical(self):
        oracle = Path(__file__).resolve().parents[1] / 'query_oracle.py'
        self.assertEqual(hashlib.sha256(oracle.read_bytes()).hexdigest(),
            'fc696ee1173093c258ff4451c4a796dd8e5832761f9e0ff49f89a922b411d23a')

    def test_collection_gap_remains_visible_in_decoded_batch(self):
        gap = 'log rotated; old tail unknown: ' + DIAG
        original = payload(4, [literal_row(4, 0, SOURCE, 7, 9, 0, PENDING)], [], [gap])
        self.assertEqual(adapter().envelope(original)[0].get('collection_gaps'), [gap])

    def test_malformed_diagnostic_row_cannot_hide_behind_fixture_selection(self):
        fixture = literal_row(4, 0, SOURCE, 7, 9, 0, PENDING)
        prefix = vi(1, 1) + ld(2, NODE) + vi(3, 1) + vi(4, 4)
        for name, malformed, reason in malformed_diagnostics():
            original = prefix + ld(6, ld(1, ld(2, ld(2, record(fixture)) + ld(2, malformed))))
            with self.subTest(diagnostic=name), self.assertRaisesRegex(ValueError, reason):
                adapter().envelope(original)

    def test_resource_metrics_resource_must_be_strictly_decoded(self):
        resource = ld(1, ld(1, b'host.name') + ld(2, ld(1, b'literal-host')))
        self.assertEqual(adapter().envelope(signal_batch(5, ld(1, ld(1, resource))))[2],
                         [literal_row(4, 0, SOURCE, 7, 9, 0, PENDING)])
        with self.assertRaisesRegex(ValueError, 'protobuf (field|tag)'):
            adapter().envelope(signal_batch(5, ld(1, ld(1, b'\x00\x00'))))

    def test_resource_spans_resource_must_be_strictly_decoded(self):
        resource = ld(1, ld(1, b'host.name') + ld(2, ld(1, b'literal-host')))
        self.assertEqual(adapter().envelope(signal_batch(9, ld(1, ld(1, resource))))[2],
                         [literal_row(4, 0, SOURCE, 7, 9, 0, PENDING)])
        with self.assertRaisesRegex(ValueError, 'protobuf (field|tag)'):
            adapter().envelope(signal_batch(9, ld(1, ld(1, b'\x00\x00'))))

    def test_scope_metrics_scope_must_be_strictly_decoded(self):
        scope = ld(1, b'literal-scope') + ld(2, b'1.0')
        self.assertEqual(adapter().envelope(signal_batch(5, ld(1, ld(2, ld(1, scope)))))[2],
                         [literal_row(4, 0, SOURCE, 7, 9, 0, PENDING)])
        with self.assertRaisesRegex(ValueError, 'protobuf (field|tag)'):
            adapter().envelope(signal_batch(5, ld(1, ld(2, ld(1, b'\x00\x00')))))

    def test_schema_url_utf8_is_required_at_every_common_placement(self):
        for signal, name in ((5, 'metrics'), (9, 'traces'), (6, 'logs')):
            for placement in ('resource', 'scope'):
                def export(url):
                    field = ld(3, url)
                    return ld(1, field if placement == 'resource' else ld(2, field))
                with self.subTest(signal=name, placement=placement):
                    self.assertEqual(adapter().envelope(signal_batch(
                        signal, export(b'https://schemas.example.test/1.0')))[2],
                        [] if signal == 6 else [literal_row(4, 0, SOURCE, 7, 9, 0, PENDING)])
                    with self.assertRaisesRegex(ValueError, 'UTF|utf'):
                        adapter().envelope(signal_batch(signal, export(b'\xff')))

    def test_common_resource_keys_and_scope_names_are_utf8(self):
        for signal in (5, 9):
            for placement in ('resource-key', 'scope-name'):
                def export(text):
                    return ld(1, ld(1, ld(1, ld(1, text) + ld(2, ld(1, b'host'))))
                        if placement == 'resource-key' else ld(2, ld(1, ld(1, text))))
                with self.subTest(signal=signal, placement=placement):
                    self.assertEqual(adapter().envelope(signal_batch(signal, export(b'literal')))[2],
                                     [literal_row(4, 0, SOURCE, 7, 9, 0, PENDING)])
                    with self.assertRaisesRegex(ValueError, 'UTF|utf'):
                        adapter().envelope(signal_batch(signal, export(b'\xff')))

    def test_legacy_mode_still_refuses_a_diagnostics_directory(self):
        fixture = legacy.CustodyWitnessControls('test_clean_raw_snapshot_and_two_line_conservation')
        fixture.module = adapter(); self.addCleanup(fixture.doCleanups); fixture.setUp()
        with self.assertRaisesRegex(ValueError, 'unexpected or failed Spool input'):
            fixture.snapshot('legacy-diagnostics', change=lambda root: (root / 'diagnostics').mkdir())

    def test_extra_allowed_path_row_in_pending_batch_is_refused(self):
        fixture = legacy.CustodyWitnessControls('test_clean_raw_snapshot_and_two_line_conservation')
        fixture.module = adapter(); self.addCleanup(fixture.doCleanups); fixture.setUp()
        baseline = fixture.chain(); fixture.module.conserve(*baseline)
        rows = [literal_row(4, 0, SOURCE, 7, 9, 0, PENDING),
                literal_row(4, 1, SOURCE, 7, 9, 0, b'extra-fixture-row\n')]
        pending = payload(4, rows, [legacy.cursor(len(PENDING))])
        unread = legacy.batch(5, body=UNREAD, offset=len(PENDING + UNREAD))
        before = fixture.snapshot('extra-before', [pending])
        installed = fixture.snapshot('extra-installed', [pending])
        drained = fixture.snapshot('extra-drain', [pending, unread], ack=5,
            receipt_change=lambda receipt: receipt.update(candidate_process=copy.deepcopy(PROCESS)))
        events = copy.deepcopy(baseline[-1])
        events[0]['message'] = ('delivery sequence=4 sha256=' + hashlib.sha256(pending).hexdigest()
                               + ' status=ack committed_through=4 elapsed_us=1')
        with self.assertRaisesRegex(ValueError, 'unexpected pending log row'):
            fixture.module.conserve(before, installed, drained, baseline[3], events)

    def test_predecessor_query_rows_cannot_substitute_label_or_strand(self):
        # Source deduction: queries.before currently becomes its own expected baseline.
        fixture = legacy.CustodyWitnessControls('test_clean_raw_snapshot_and_two_line_conservation')
        fixture.module = adapter(); self.addCleanup(fixture.doCleanups); fixture.setUp()
        chain = fixture.chain(); fixture.module.conserve(*chain)
        for field, value in (('node', 'unrelated-enrollment'), ('node_id', '00' * 16)):
            args = copy.deepcopy(chain)
            for phase in ('before', 'after_drain', 'after_reboot'):
                for row in args[3][phase][:3]: row[field] = value
            self.assertEqual(args[0]['node_id'], NODE.hex())
            self.assertEqual(args[0]['sources'], chain[0]['sources'])
            self.assertEqual(args[4], chain[4])
            with self.subTest(substitution=field), self.assertRaisesRegex(ValueError, 'old.row|baseline'):
                fixture.module.conserve(*args)


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
        self.assertEqual(before['diagnostics']['spindle.log']['bytes'], DIAG_OLD + DIAG_PENDING)
        self.assertEqual(drained['diagnostics']['spindle.log']['bytes'], DIAG_OLD + DIAG_PENDING + DIAG_NEW + STOP)
        self.assertEqual(drained['logs'][DIAG]['bytes'], DIAG_OLD + DIAG_PENDING + DIAG_NEW + STOP)
        self.assertEqual(drained['cursors'][DIAG]['offset'], len(DIAG_OLD + DIAG_PENDING + DIAG_NEW))
        self.assertEqual(len(queries['before']), 4)
        self.assertEqual(before['sources'][0]['rows'][0]['attributes']['log.file.path'], DIAG)
        for source in before['sources'][:3]:
            self.assertEqual({current['path'] for current in self.module.envelope(source['payload'])[1]},
                             {SOURCE, DIAG})
        self.assertIn(ld(7, source_cursor(DIAG, before['logs'][DIAG]['device'],
            before['logs'][DIAG]['inode'], DIAG_OLD + DIAG_PENDING, len(DIAG_OLD))),
            before['sources'][0]['payload'])
        self.assertEqual(len(queries['after_drain']), 8)
        self.assertEqual(sum(row['attributes']['log.file.path'] == SOURCE
                             for row in queries['after_drain']), 5)
        self.assertEqual(len(drained['sources'][-1]['rows']), 2)
        self.assertEqual(drained['sources'][3]['payload'], before['sources'][3]['payload'])
        self.assertEqual(drained['sources'][3]['sha256'], before['sources'][3]['sha256'])
        self.module.conserve(*chain)

    def test_source_path_not_body_selects_the_five_fixture_rows(self):
        for body in (PENDING, UNREAD):
            with self.subTest(body=body):
                chain = self.chain(diag_pending=body, diag_line=body)
                self.assertEqual(sum(row['body'] == body[:-1].decode()
                                     for row in chain[3]['after_drain']), 3)
                self.assertEqual(chain[0]['sources'][3]['rows'][1]['body'], body[:-1].decode())
                self.module.conserve(*chain)
            self.temp.cleanup(); self.setUp()

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
                        dict(rotate=True, rotated_old=b'event different-generation\n'),
                        dict(rotate=True, rotated_replace=True),
                        dict(rotate=True, rotation_size=CAP - len(DIAG_NEW)),
                        dict(rotate=True, diag_line=b'x' * 4096 + b'\n'),
                        dict(prior_old=True, old_rewrite=True)):
            with self.subTest(options=options), self.assertRaisesRegex(
                    ValueError, 'diagnostic.*(prefix|rotation|preserv)'):
                self.module.conserve(*self.chain(**options))
            self.temp.cleanup(); self.setUp()

    def test_unchanged_old_diagnostic_file_is_preserved_without_rotation(self):
        chain = self.chain(prior_old=True)
        self.assertEqual(chain[0]['diagnostics']['spindle.log.1'],
                         chain[2]['diagnostics']['spindle.log.1'])
        self.module.conserve(*chain)

    def test_every_pending_raw_log_row_is_in_both_complete_query_answers(self):
        baseline = self.chain(); self.module.conserve(*baseline)
        for phase in ('after_drain', 'after_reboot', 'both'):
            for mutation in ('drop', 'duplicate', 'rewrite'):
                args = copy.deepcopy(baseline)
                for selected in (('after_drain', 'after_reboot') if phase == 'both' else (phase,)):
                    rows = args[3][selected]
                    index = next(index for index, row in enumerate(rows)
                                 if (row['sequence'], row['index']) == (4, 1))
                    if mutation == 'drop': rows.pop(index)
                    elif mutation == 'duplicate': rows.append(copy.deepcopy(rows[index]))
                    else: rows[index]['body'] = 'rewritten-diagnostic'
                with self.subTest(phase=phase, mutation=mutation), self.assertRaisesRegex(
                        ValueError, 'exact row conservation'):
                    self.module.conserve(*args)

    def test_extra_allowed_path_row_in_native_pending_batch_is_refused(self):
        with self.assertRaisesRegex(ValueError, 'unexpected pending log row'):
            self.module.conserve(*self.chain(extra_pending=True))

    def test_fixture_source_gap_in_pending_or_unread_batch_is_refused(self):
        allowed = self.chain(pending_gap=('log source unavailable ' + DIAG + ': unavailable',))
        self.assertEqual(allowed[0]['sources'][3].get('collection_gaps'),
                         ['log source unavailable ' + DIAG + ': unavailable'])
        self.module.conserve(*allowed)
        for phase in ('pending_gap', 'unread_gap'):
            self.temp.cleanup(); self.setUp()
            with self.subTest(phase=phase), self.assertRaisesRegex(ValueError, 'fixture.*gap'):
                self.module.conserve(*self.chain(**{phase: ('log truncated; previous tail unknown: ' + SOURCE,)}))

    def test_legacy_frame_and_failure_markers_are_also_refused_in_native_mode(self):
        item = legacy.batch()
        changes = (
            lambda root: (root / 'batches.faj').write_bytes(frame(item)[:-1]),
            lambda root: (root / 'batches.faj').write_bytes(frame(item) + b'F'),
            lambda root: (root / 'batches.faj').write_bytes(frame(item)[:16] + b'X' + frame(item)[17:]),
            lambda root: (root / 'recovery-required').write_bytes(b'failure'),
            lambda root: (root / 'append-in-progress').write_bytes(b'interrupted'),
            lambda root: (root / 'acked').write_bytes(b'FAK1' + struct.pack('<QQ', 1, 2**64 - 1)),
            lambda root: (root / 'acked').write_bytes(b'FAK1' + struct.pack('<QQ', 2, 0)))
        for index, change in enumerate(changes):
            with self.subTest(change=index), self.assertRaises(ValueError):
                self.snapshot('native-marker', [item], 3, PENDING + UNREAD, change=change)
            self.temp.cleanup(); self.setUp()

    def test_native_root_requires_private_diagnostic_directory_and_active_file(self):
        item = legacy.batch()
        for variant in ('missing-directory', 'missing-active', 'mode', 'symlink'):
            if variant.startswith('missing'):
                self.active.unlink()
                if variant == 'missing-directory': self.diagnostics.rmdir()
            elif variant == 'mode': self.diagnostics.chmod(0o755)
            else:
                outside = self.root / 'outside-diagnostics'; self.diagnostics.replace(outside)
                self.diagnostics.symlink_to(outside, target_is_directory=True)
            with self.subTest(directory=variant), self.assertRaisesRegex(ValueError, 'diagnostic'):
                self.snapshot('required-directory', [item], 3, PENDING + UNREAD)
            self.temp.cleanup(); self.setUp()

    def test_diagnostic_archive_must_match_actual_in_root_bytes_and_identity(self):
        item = legacy.batch()
        for variant in ('bytes', 'device', 'inode'):
            def change(logs):
                if variant == 'bytes': Path(logs[DIAG]['archive_path']).write_bytes(b'forged-diagnostic\n')
                else: logs[DIAG][variant] += 1
            with self.subTest(archive=variant), self.assertRaisesRegex(ValueError, 'diagnostic.*(archive|source)'):
                self.snapshot('mismatched-archive', [item], 3, PENDING + UNREAD, archive_change=change)

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


class RebootRawControls(NativeFixture, unittest.TestCase):
    """Synthetic review counterexample: startup adds a genuine new raw log row."""
    def setUp(self):
        super().setUp()
        self.module = adapter()

    def reboot_chain(self):
        chain = list(self.chain())
        before, installed, drained, queries, events = chain
        raw = self.active.read_bytes()
        line = b'event candidate-reboot-started\n'
        self.active.write_bytes(raw + line)
        sequence = drained['sources'][-1]['sequence'] + 1
        info = self.active.stat()
        row = literal_row(sequence, 0, DIAG, info.st_dev, info.st_ino, len(raw), line)
        reboot_payload = payload(sequence, [row], [source_cursor(SOURCE, 7, 9,
            drained['logs'][SOURCE]['bytes'], drained['cursors'][SOURCE]['offset']),
            source_cursor(DIAG, info.st_dev, info.st_ino, raw + line, len(raw + line))])
        reboot_process = dict(PROCESS, pid=77, boot_id='literal-boot-b', start_ns=300, end_ns=400)
        reboot = self.snapshot('reboot', [item['payload'] for item in drained['sources']] + [reboot_payload],
            sequence, drained['logs'][SOURCE]['bytes'], receipt_change=lambda receipt: receipt.update(
                boot_id='literal-boot-b', candidate_process=reboot_process))
        queries['after_reboot'].append(row)
        reboot_events = [dict(unit=PROCESS['unit'], pid=77, boot_id='literal-boot-b', at_ns=350,
            message=f'delivery sequence={sequence} sha256={hashlib.sha256(reboot_payload).hexdigest()} '
                    f'status=ack committed_through={sequence} elapsed_us=1')]
        return chain, reboot, reboot_events

    def test_old_three_phase_rule_rejects_new_reboot_diagnostic(self):
        chain, _, _ = self.reboot_chain()
        with self.assertRaisesRegex(ValueError, 'exact row conservation'):
            self.module.conserve(*chain)

    def test_fourth_raw_snapshot_admits_exact_new_reboot_diagnostic(self):
        chain, reboot, events = self.reboot_chain()
        self.module.conserve(*chain, after_reboot=reboot, reboot_ack_events=events)

    def test_fourth_snapshot_rejects_unjustified_or_changed_rows_and_sender(self):
        for mutation in ('drop', 'duplicate', 'forge', 'early-answer', 'raw-conflict',
                         'same-boot', 'ack-pid', 'ack-hash', 'missing-ack', 'fixture-extra'):
            with self.subTest(mutation=mutation):
                chain, reboot, events = self.reboot_chain()
                if mutation == 'drop': chain[3]['after_reboot'].pop()
                elif mutation == 'duplicate': chain[3]['after_reboot'].append(copy.deepcopy(chain[3]['after_reboot'][-1]))
                elif mutation == 'forge': chain[3]['after_reboot'][-1]['body'] = 'forged'
                elif mutation == 'early-answer': chain[3]['after_drain'].append(chain[3]['after_reboot'][-1])
                elif mutation == 'raw-conflict': reboot['sources'][0]['sha256'] = '0' * 64
                elif mutation == 'same-boot': reboot['boot_id'] = chain[0]['boot_id']
                elif mutation == 'ack-pid': events[0]['pid'] = 55
                elif mutation == 'ack-hash': events[0]['message'] = events[0]['message'].replace(reboot['sources'][-1]['sha256'], '0' * 64)
                elif mutation == 'missing-ack': events.clear()
                elif mutation == 'fixture-extra': reboot['sources'][-1]['rows'][0]['attributes']['log.file.path'] = SOURCE
                with self.assertRaises(ValueError):
                    self.module.conserve(*chain, after_reboot=reboot, reboot_ack_events=events)
            self.temp.cleanup(); self.setUp()


if __name__ == '__main__':
    unittest.main()
