"""Retained active-stream counterexample; paused final-prefix checks stay strict."""
import copy
import inspect
import json
from pathlib import Path
import unittest
from unittest.mock import patch
import custody_native as native

FIXTURE = Path(__file__).resolve().parents[3] / 'docs/experiments/formal/fixtures/installation-active-prefix.json'
LITERAL = json.loads(FIXTURE.read_text())


class ActivePrefixControls(unittest.TestCase):
    def exercise(self, *, active=True, stale=False, no_new=False, recovery=False):
        guest = native.Guest.__new__(native.Guest)
        guest.deadline = 100
        clock, calls = [0.0], []
        def command(argv, check=True):
            self.assertEqual(argv, ['fabricctl', 'inspect', '/etc/fabrico11y/node.conf'])
            fields = copy.deepcopy(LITERAL['observations'][0]['fields'])
            highwater = LITERAL['prior_ack'] if no_new else int(fields['next_sequence']) - 1 + len(calls)
            fields['next_sequence'] = str(highwater + 1)
            fields['acked_through'] = str(LITERAL['prior_ack'] if stale else highwater - 1 if not calls else highwater)
            if recovery: fields['recovery_required'] = 'true'
            record = dict(argv=argv, exit=0, stderr='', stdout=' '.join(k+'='+v for k,v in fields.items()))
            calls.append(record)
            return record
        guest.command = command
        def sleep(seconds): clock[0] += seconds
        # The old caller is exercised through its existing interface so the red
        # reaches its stable-prefix deadline rather than a missing keyword API.
        kwargs = {'active_prefix': active} if 'active_prefix' in inspect.signature(guest.drain_prefix).parameters else {}
        with patch.object(native.time, 'monotonic', lambda: clock[0]), patch.object(native.time, 'sleep', sleep):
            result = guest.drain_prefix(LITERAL['prior_ack'], **kwargs)
        return result, calls, clock[0]

    def test_advancing_active_stream_covers_first_observed_new_target(self):
        result, calls, elapsed = self.exercise()
        self.assertEqual(result, int(LITERAL['observations'][0]['fields']['next_sequence']) - 1)
        self.assertEqual(len(calls), 2)
        self.assertEqual(elapsed, .25)

    def test_stale_ack_and_no_new_commit_cannot_supply_progress(self):
        for mode in ('stale', 'no_new'):
            with self.subTest(mode=mode), self.assertRaisesRegex(ValueError, 'native fixed-prefix ACK deadline'):
                self.exercise(**{mode: True})

    def test_advancing_paused_prefix_still_refuses(self):
        with self.assertRaisesRegex(ValueError, 'native fixed-prefix ACK deadline'):
            self.exercise(active=False)

    def test_recovery_still_refuses(self):
        with self.assertRaisesRegex(ValueError, 'native spool requires recovery'):
            self.exercise(recovery=True)

    def test_invalid_scope_and_exact_target_combination_refuse(self):
        guest = native.Guest.__new__(native.Guest)
        for kwargs in (dict(active_prefix=1), dict(active_prefix=True, exact=5)):
            with self.subTest(kwargs=kwargs), self.assertRaisesRegex(ValueError, 'invalid active prefix scope'):
                guest.drain_prefix(**kwargs)


if __name__ == '__main__': unittest.main()
