"""Finite lifecycle/observation controls; never native exposure evidence."""
import copy
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import soak_r3
from soak_ui_continuity import validate

NS = 1_000_000_000


def ui_fixture():
    events = [{'seq': 1, 'event': 'timer', 'timer': 1, 'delay': 5000, 'atMs': 0},
              {'seq': 2, 'event': 'polling', 'active': True, 'atMs': 1}]
    for tick in range(1, 6):
        events += [{'event': 'tick', 'timer': 1, 'tick': tick, 'atMs': tick * 5000,
                    'visibility': 'visible', 'running': True, 'connected': True, 'busy': False},
                   {'event': 'attempt', 'id': tick, 'tick': tick, 'atMs': tick * 5000 + 1},
                   {'event': 'headers', 'id': tick, 'status': 200, 'atMs': tick * 5000 + 2},
                   {'event': 'complete', 'id': tick, 'failed': False, 'atMs': tick * 5000 + 3}]
    for seq, event in enumerate(events, 1):
        event['seq'] = seq
    common = {'documentId': 'document-01', 'webdriver_session': 'session-01',
              'timeOriginMs': 100000, 'visibility': 'visible', 'eventOverflow': False, 'active': True}
    return [dict(common, nowMs=2, mono_before_ns=2_000_000, mono_after_ns=2_000_100,
                 eventCount=2, events=events[:2]),
            dict(common, nowMs=26000, mono_before_ns=26 * NS, mono_after_ns=26 * NS + 100,
                 eventCount=len(events), events=events[2:])]


class RuntimeControls(unittest.TestCase):
    def test_outer_stop_and_command_identity_are_not_self_asserted(self):
        expected = {'unit': 'fabric-work-owned.service', 'command': ['grader'], 'cgroup': '/owned'}
        actual = dict(expected, runtime_max_seconds=1800, stop_confirmed=True,
                      exit=0, wrapper_exit=0, command_receipt={'cgroup': '/owned', 'state': 'completed', 'exit': 0})
        self.assertIsNone(soak_r3.stopped_outer(actual, expected, 1800))
        for key, value in [('stop_confirmed', False), ('unit', 'other'), ('command', ['replacement']),
                           ('runtime_max_seconds', 7500), ('exit', True)]:
            bad = dict(actual, **{key: value})
            with self.subTest(key=key), self.assertRaises(ValueError):
                soak_r3.stopped_outer(bad, expected, 1800)

    def test_narrow_r3_control_keeps_freshness_for_other_mutations(self):
        from console_bridge import ConsoleBridge
        bridge = ConsoleBridge.__new__(ConsoleBridge)
        bridge.lock = __import__('threading').RLock()
        bridge.r3_control_session = True
        bridge._private_cadence = mock.Mock()
        bridge.session_view = lambda: {'csrf': 'fixture', 'fresh_until_unix_s': 0}
        bridge._request = mock.Mock(return_value=(200, {}))
        bridge.login = mock.Mock(return_value={'csrf': 'fresh'})
        bridge.request('/v1/console/nodes/sim0000/config', 'PUT', {})
        bridge.login.assert_not_called()
        bridge.request('/v1/console/readers', 'POST', {})
        bridge.login.assert_called_once()

    def test_phase_order_and_no_borrowing(self):
        now = [100 * NS]
        with mock.patch.object(soak_r3.signal, 'setitimer'):
            clock = soak_r3.PhaseClock(0, now=lambda: now[0])
            clock.enter('setup')
            self.assertEqual(clock.remaining(), 70 * NS)
            now[0] = 170 * NS - 1
            clock.enter('exposure')
            self.assertEqual(clock.records[0]['elapsed_ns'], 170 * NS - 1)
            with self.assertRaisesRegex(ValueError, 'ordered'):
                clock.enter('shutdown')

    def test_one_ns_beyond_phase_and_global_is_refused(self):
        with mock.patch.object(soak_r3.signal, 'setitimer'):
            clock = soak_r3.PhaseClock(0, now=lambda: 170 * NS)
            with self.assertRaisesRegex(ValueError, 'phase budget'):
                clock.enter('setup')

    def test_progressive_blocking_operation_checked_after_return(self):
        now = [0]
        with mock.patch.object(soak_r3.signal, 'setitimer'):
            clock = soak_r3.PhaseClock(0, now=lambda: now[0])
            clock.enter('setup')
            def blocking():
                now[0] = 171 * NS
            with self.assertRaisesRegex(ValueError, 'phase budget'):
                clock.call(blocking)

    def test_changed_admission_and_exclusive_output_refused(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            path = Path(directory) / 'input.json'
            soak_r3.write_new(path, {'protocol': 'R3'})
            before = soak_r3.digest(path)
            path.write_text('{}')
            with self.assertRaisesRegex(ValueError, 'pinned input changed'):
                soak_r3.pinned(path, before)
            with self.assertRaises(FileExistsError):
                soak_r3.write_new(path, {})

    def test_missing_prerequisites_refused_before_any_launch(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            path = Path(directory) / 'admission.json'
            soak_r3.write_new(path, {'protocol': 'R3', 'seed': 0xA11FA001, 'identities': 100,
                                   'workers': 100, 'seconds': 7200, 'smoke': False, 'inputs': {}})
            with mock.patch.object(soak_r3.subprocess, 'Popen', side_effect=AssertionError('launched')):
                with self.assertRaisesRegex(ValueError, 'missing certified native_publication'):
                    soak_r3.admission(path, soak_r3.digest(path))


class BrowserObservationControls(unittest.TestCase):
    def test_complete_native_events(self):
        result = validate(ui_fixture(), 3 * NS, 23 * NS)
        self.assertEqual(result['attempts'], 4)
        self.assertEqual(result['max_attempt_gap_ms'], 5000)

    def test_changed_document_session_clock_and_short_coverage(self):
        for field, value in [('documentId', 'replacement'), ('webdriver_session', 'new-session'),
                             ('nowMs', 30000), ('mono_before_ns', 22 * NS), ('visibility', 'hidden')]:
            bad = ui_fixture()
            bad[-1][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                validate(bad, 3 * NS, 23 * NS)

    def test_lost_duplicate_or_failed_native_event(self):
        for mutate in [lambda events: events.pop(2),
                       lambda events: events.insert(2, copy.deepcopy(events[2])),
                       lambda events: events[2].update(status=429),
                       lambda events: events[3].update(failed=True),
                       lambda events: events[0].update(timer=99)]:
            bad = ui_fixture()
            mutate(bad[-1]['events'])
            with self.assertRaises(ValueError):
                validate(bad, 3 * NS, 23 * NS)

    def test_asserted_active_cannot_hide_actual_pause(self):
        bad = ui_fixture()
        event = {'event': 'polling', 'active': False, 'atMs': 10000.5}
        bad[-1]['events'].insert(4, event)
        for seq, value in enumerate(bad[-1]['events'], 3):
            value['seq'] = seq
        bad[-1]['eventCount'] += 1
        with self.assertRaises(ValueError):
            validate(bad, 3 * NS, 23 * NS)

    def test_eligible_timer_callback_without_request_is_refused(self):
        bad = ui_fixture()
        # Keep a syntactically complete ledger while deleting a whole request.
        del bad[-1]['events'][1:4]
        for seq, value in enumerate(bad[-1]['events'], 3):
            value['seq'] = seq
        bad[-1]['eventCount'] -= 3
        with self.assertRaisesRegex(ValueError, 'lost its request'):
            validate(bad, 3 * NS, 23 * NS)


if __name__ == '__main__':
    unittest.main()
