"""Command-admission regression; the retained native HTTP 429 is its origin."""
import json
from pathlib import Path
import threading
import unittest
from unittest.mock import patch

from console_bridge import ConsoleBridge

FIXTURE = Path(__file__).resolve().parents[2] / 'docs/experiments/formal/fixtures/console-command-admission.json'


def refused_requests(times, rate, burst):
    """Independent offered-request token accounting, with no server output."""
    scale = 1_000_000_000
    timestamps = sorted(round(value * scale) for value in times)
    previous = timestamps[0]
    credit = burst * scale
    refused = 0
    for now in timestamps:
        credit = min(burst * scale, credit + rate * (now - previous))
        previous = now
        if credit < scale:
            refused += 1
        else:
            credit -= scale
    return refused


class ConsoleAdmissionTests(unittest.TestCase):
    def test_legacy_command_pairs_are_rejected_by_independent_schedule(self):
        fixture = json.loads(FIXTURE.read_text())
        case = fixture['independent_schedule']
        times = [i * case['legacy_command_interval_seconds'] + offset
                 for i in range(case['commands']) for offset in (0, .01)]
        self.assertGreater(refused_requests(times, case['peer_requests_per_second'], case['peer_burst']), 0)

    def test_commands_leave_capacity_for_registered_visibility_and_ui(self):
        case = json.loads(FIXTURE.read_text())['independent_schedule']
        clock = [10.0]
        times, sessions, requests = [], [], []
        bridge = ConsoleBridge.__new__(ConsoleBridge)
        bridge.lock = threading.RLock()

        def session():
            times.append(clock[0])
            clock[0] += .01
            csrf = f'synthetic-csrf-{len(sessions)}'
            sessions.append(csrf)
            return {'csrf': csrf, 'fresh_until_unix_s': 1000}

        def request(path, method, body, csrf):
            times.append(clock[0])
            clock[0] += .01
            requests.append((path, method, csrf))
            return 200, {'token': 'synthetic-fixture-only'}

        with patch('console_bridge.time.monotonic', side_effect=lambda: clock[0]), \
                patch('console_bridge.time.time', return_value=0), \
                patch('console_bridge.time.sleep', side_effect=lambda delay: clock.__setitem__(0, clock[0] + delay)), \
                patch.object(bridge, 'session_view', side_effect=session), \
                patch.object(bridge, '_request', side_effect=request):
            for i in range(case['commands']):
                self.assertEqual(bridge.request('/v1/console/nodes', 'POST', {'name': f'sim{i:04d}'})[0], 200)
        self.assertEqual(len(sessions), case['commands'])
        self.assertEqual([r[2] for r in requests], sessions)
        begin, end = times[0], times[-1]
        # The main protocol offers three reader probes per second; the real UI
        # contributes its separately declared five-second polling request.
        background = [begin + second + offset
                      for second in range(int(end - begin) + 1)
                      for offset in (.2, .3, .4)
                      if begin + second + offset <= end]
        background += [begin + second + .45
                       for second in range(0, int(end - begin) + 1, case['ui_poll_interval_seconds'])
                       if begin + second + .45 <= end]
        self.assertEqual(refused_requests(times + background, case['peer_requests_per_second'], case['peer_burst']), 0)

    def test_session_is_checked_and_refusal_is_returned_without_retry(self):
        bridge = ConsoleBridge.__new__(ConsoleBridge)
        bridge.lock = threading.RLock()
        with patch('console_bridge.time.time', return_value=0), \
                patch.object(bridge, 'session_view', return_value={'csrf': 'synthetic-current', 'fresh_until_unix_s': 1000}) as session, \
                patch.object(bridge, '_request', return_value=(429, {'error': 'synthetic admission refusal'})) as request:
            self.assertEqual(bridge.request('/v1/console/nodes', 'POST', {'name': 'probe'})[0], 429)
        session.assert_called_once_with()
        request.assert_called_once_with('/v1/console/nodes', 'POST', {'name': 'probe'}, 'synthetic-current')


if __name__ == '__main__':
    unittest.main()
