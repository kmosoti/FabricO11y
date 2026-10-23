"""Independent controls for the registered native configuration lifecycle.

The fixture reuses the existing hand-encoded RebootRawControls, snapshots, raw
Spool decoder, and query/ACK oracle. It is synthetic source evidence only.
"""
import hashlib
import json
import unittest
import test_custody_witness as legacy

from test_custody_native import NativeFixture, SOURCE, DIAG, PENDING, UNREAD, APPLIED, literal_row, payload, source_cursor
from test_custody_witness import PROCESS

DENIED = '/var/log/fabric-accept/denied.log'
LOGS = [SOURCE, DENIED]


def profile(revision, paused, *, logs=None, interval=15):
    return dict(revision=revision, paused=paused, logs=list(LOGS if logs is None else logs),
                metric_interval_s=interval)


def raw_profile(value):
    return json.dumps(value, separators=(',', ':')).encode()


class NativeConfigurationLifecycleControls(NativeFixture, unittest.TestCase):
    def setUp(self):
        super().setUp()
        self.module = legacy.adapter()
        self.config = self.spool / 'applied-config.json'
        self.config_overrides = {}
        self.static_profile = False
        self.config.write_bytes(APPLIED)
        self.config.chmod(0o600)

    def snapshot(self, phase, payloads, ack, allowed, *, fixture_paths=(SOURCE,),
                 receipt_change=None, change=None, archive_change=None):
        if phase in self.config_overrides:
            raw = self.config_overrides[phase]
        elif self.static_profile:
            raw = APPLIED
        else:
            phase_profile = {
                'before': profile(1, False),
                'installed': profile(1, False),
                'drain': profile(2, True),
                'reboot': profile(4, True),
            }.get(phase, profile(1, False))
            raw = raw_profile(phase_profile)
        self.config.write_bytes(raw)
        self.config.chmod(0o600)
        return super().snapshot(phase, payloads, ack, allowed, fixture_paths=fixture_paths,
            receipt_change=receipt_change, change=change, archive_change=archive_change)

    def transition_chain(self):
        chain = list(self.chain())
        before, installed, drained, queries, _drain_events = chain
        raw = self.active.read_bytes()
        line = b'event candidate-reboot-started\n'
        self.active.write_bytes(raw + line)
        sequence = drained['sources'][-1]['sequence'] + 1
        info = self.active.stat()
        row = literal_row(sequence, 0, DIAG, info.st_dev, info.st_ino, len(raw), line)
        reboot_payload = payload(sequence, [row], [
            source_cursor(SOURCE, 7, 9, drained['logs'][SOURCE]['bytes'], drained['cursors'][SOURCE]['offset']),
            source_cursor(DIAG, info.st_dev, info.st_ino, raw + line, len(raw + line)),
        ])
        reboot_process = dict(PROCESS, pid=77, boot_id='literal-boot-b', start_ns=300, end_ns=400)
        reboot = self.snapshot('reboot', [item['payload'] for item in drained['sources']] + [reboot_payload],
            sequence, drained['logs'][SOURCE]['bytes'], receipt_change=lambda receipt: receipt.update(
                boot_id='literal-boot-b', candidate_process=reboot_process))
        queries['after_reboot'].append(row)
        reboot_events = [dict(unit=PROCESS['unit'], pid=77, boot_id='literal-boot-b', at_ns=350,
            message=f'delivery sequence={sequence} sha256={hashlib.sha256(reboot_payload).hexdigest()} '
                    f'status=ack committed_through={sequence} elapsed_us=1')]
        return chain, reboot, reboot_events

    def conserve_transition(self, chain, reboot, events):
        self.module.conserve(*chain, after_reboot=reboot, reboot_ack_events=events,
                             native_configuration_transition=True)

    def test_pause_marker_conservation_reparses_raw_proof(self):
        import copy
        import os
        import stat
        from test_custody_pause import proof
        chain, reboot, events = self.transition_chain()
        drained = chain[2]
        raw = b'115\n'
        info = (1, 2, stat.S_IFREG | 0o600, os.geteuid(), os.getegid(), 1, len(raw), 0, 0)
        drained['coverage_pause'] = dict(raw=raw, observation=proof())
        drained['file_observations']['coverage-unknown'] = info
        drained['files']['coverage-unknown'] = dict(sha256=hashlib.sha256(raw).hexdigest(),
            size=len(raw), mode=info[2], uid=info[3], gid=info[4])
        self.conserve_transition(chain, reboot, events)
        original = copy.deepcopy(drained['coverage_pause'])
        for mutation in ('bytes', 'proof', 'missing'):
            drained['coverage_pause'] = copy.deepcopy(original)
            if mutation == 'bytes': drained['coverage_pause']['raw'] = b'116\n'
            elif mutation == 'proof': drained['coverage_pause']['observation']['request']['exit'] = 1
            else: drained['coverage_pause'] = None
            with self.subTest(mutation=mutation), self.assertRaisesRegex(ValueError, 'native pause marker'):
                self.conserve_transition(chain, reboot, events)

    def test_exact_native_four_checkpoint_transition_is_accepted(self):
        chain, reboot, events = self.transition_chain()
        self.conserve_transition(chain, reboot, events)

    def test_native_scope_requires_exact_fixture_path_and_boolean_flag(self):
        chain, reboot, events = self.transition_chain()
        for snapshot in (chain[0], chain[1], chain[2], reboot):
            snapshot['fixture_paths'] = (DENIED,)
        with self.assertRaisesRegex(ValueError, 'native configuration transition fixture path'):
            self.conserve_transition(chain, reboot, events)
        chain, reboot, events = self.transition_chain()
        with self.assertRaisesRegex(ValueError, 'scope must be boolean'):
            self.module.conserve(*chain, after_reboot=reboot, reboot_ack_events=events,
                                 native_configuration_transition=1)

    def test_legacy_call_reaches_static_profile_mismatch_without_scope_keyword(self):
        chain, reboot, events = self.transition_chain()
        with self.assertRaisesRegex(ValueError, 'registered lifecycle applied configuration changed'):
            self.module.conserve(*chain, after_reboot=reboot, reboot_ack_events=events)

    def test_coherent_raw_change_with_forged_cached_view_is_refused(self):
        chain, reboot, events = self.transition_chain()
        raw = raw_profile(profile(3, True))
        reboot['applied_configuration'] = raw
        reboot['files']['applied-config.json']['sha256'] = hashlib.sha256(raw).hexdigest()
        reboot['files']['applied-config.json']['size'] = len(raw)
        info = list(reboot['file_observations']['applied-config.json'])
        info[6] = len(raw)
        reboot['file_observations']['applied-config.json'] = tuple(info)
        # A stale/forged cache must never override the retained raw configuration.
        reboot['applied_configuration_view'] = profile(4, True)
        with self.assertRaisesRegex(ValueError, 'native applied configuration profile transition'):
            self.conserve_transition(chain, reboot, events)

    def test_wrong_revision_state_logs_and_interval_are_refused(self):
        mutations = (
            ('revision', profile(3, True)),
            ('paused', profile(2, False)),
            ('logs', profile(2, True, logs=[SOURCE, DENIED, '/var/log/other.log'])),
            ('interval', profile(2, True, interval=30)),
        )
        for name, value in mutations:
            with self.subTest(mutation=name):
                self.config_overrides = {'drain': raw_profile(value)}
                chain, reboot, events = self.transition_chain()
                with self.assertRaisesRegex(ValueError, 'native applied configuration profile transition'):
                    self.conserve_transition(chain, reboot, events)
                self.temp.cleanup(); self.setUp()

    def test_noninteger_and_duplicate_json_fields_are_refused_at_snapshot(self):
        valid = b'{"revision":2,"paused":true,"logs":["' + SOURCE.encode() + b'","' + DENIED.encode() + b'"],"metric_interval_s":15}'
        malformed = (
            valid.replace(b'"revision":2', b'"revision":2.0'),
            valid.replace(b'{"revision":2,', b'{"revision":1,"revision":2,'),
            valid.replace(b'"metric_interval_s":15', b'"metric_interval_s":15.0'),
        )
        for raw in malformed:
            with self.subTest(raw=raw):
                self.config_overrides = {'drain': raw}
                with self.assertRaisesRegex(ValueError, 'applied configuration'):
                    self.transition_chain()
                self.temp.cleanup(); self.setUp()

    def test_raw_hash_and_metadata_join_mutations_are_refused(self):
        for mutation in ('digest', 'bytes', 'mode'):
            with self.subTest(mutation=mutation):
                chain, reboot, events = self.transition_chain()
                target = reboot
                if mutation == 'digest':
                    target['files']['applied-config.json']['sha256'] = 'f' * 64
                elif mutation == 'bytes':
                    target['applied_configuration'] += b' '
                else:
                    info = list(target['file_observations']['applied-config.json'])
                    info[2] = (info[2] & ~0o777) | 0o644
                    target['file_observations']['applied-config.json'] = tuple(info)
                with self.assertRaisesRegex(ValueError, 'native applied configuration bytes/hash/metadata join'):
                    self.conserve_transition(chain, reboot, events)
                self.temp.cleanup(); self.setUp()

    def test_legacy_static_configuration_path_remains_unchanged(self):
        self.static_profile = True
        chain, reboot, events = self.transition_chain()
        self.module.conserve(*chain, after_reboot=reboot, reboot_ack_events=events)
        self.assertEqual(chain[0]['files']['applied-config.json'], reboot['files']['applied-config.json'])


if __name__ == '__main__':
    unittest.main()
