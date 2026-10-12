"""Independent admission accounting, whole-query timing, and refusal preservation."""
import json
from pathlib import Path
import unittest
import urllib.error
from unittest.mock import patch

from query_reader import PacedQueryReader
from release_runtime import WorkloadReader
from test_console_admission import refused_requests

FIXTURE = Path(__file__).resolve().parents[2] / 'docs/experiments/formal/fixtures/query-page-admission.json'


class Response:
    def __init__(self, page):
        self.raw = json.dumps(page).encode()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False

    def read(self, _maximum):
        return self.raw


class PageAdmissionTests(unittest.TestCase):
    def exercise(self, paced):
        case = json.loads(FIXTURE.read_text())['independent_schedule']
        clock = [10_000_000_000]
        attempts = []
        reader = WorkloadReader('https://localhost:1', None, 'synthetic-reader-fixture')
        if paced:
            reader = PacedQueryReader(reader)
        pages_left = [0]

        def request(_request, **_kwargs):
            attempts.append(clock[0] / 1_000_000_000)
            self.assertEqual(_request.get_header('Authorization'), 'Bearer synthetic-reader-fixture')
            self.assertEqual(_request.full_url, 'https://localhost:1/v1/console/query')
            clock[0] += case['synthetic_page_duration_ns']
            pages_left[0] -= 1
            return Response({'rows': [], 'next_page': 'synthetic-page' if pages_left[0] else None})

        with patch('query_reader.time.monotonic_ns', side_effect=lambda: clock[0]), \
                patch('query_reader.time.monotonic', side_effect=lambda: clock[0] / 1_000_000_000), \
                patch('query_reader.time.sleep', side_effect=lambda seconds: clock.__setitem__(0, clock[0] + round(seconds * 1_000_000_000))), \
                patch('release_runtime.urllib.request.urlopen', side_effect=request):
            for count in case['page_counts'] * case['repeats']:
                pages_left[0] = count
                _, pages = reader.pages({'kind': 'metrics', 'limit': 1000})
                self.assertEqual(len(pages), count)
                clock[0] += case['logical_query_pause_ns']
        ui = [attempts[0] + second + .05
              for second in range(0, int(attempts[-1] - attempts[0]) + 1, case['ui_poll_interval_seconds'])]
        return refused_requests(attempts + ui, case['peer_requests_per_second'], case['peer_burst'])

    def test_unpaced_continuation_defect_is_rejected_by_independent_budget(self):
        self.assertGreater(self.exercise(False), 0)
        self.assertEqual(self.exercise(True), 0)

    def test_inherited_query_timer_includes_first_and_continuation_waits(self):
        clock = [10_000_000_000]
        reader = PacedQueryReader(WorkloadReader('https://localhost:1', None, 'synthetic-reader-fixture'))
        remaining = [1]

        def response(*_, **__):
            clock[0] += 1_000_000
            remaining[0] -= 1
            return Response({'rows': [], 'next_page': 'synthetic-page' if remaining[0] else None})

        with patch('query_reader.time.monotonic_ns', side_effect=lambda: clock[0]), \
                patch('query_reader.time.monotonic', side_effect=lambda: clock[0] / 1_000_000_000), \
                patch('query_reader.time.sleep', side_effect=lambda seconds: clock.__setitem__(0, clock[0] + round(seconds * 1_000_000_000))), \
                patch('release_runtime.urllib.request.urlopen', side_effect=response):
            reader.pages({'kind': 'metrics'})
            remaining[0] = 5
            elapsed, pages = reader.pages({'kind': 'metrics'})
        self.assertEqual(len(pages), 5)
        self.assertGreater(elapsed, 1.24)
        self.assertAlmostEqual(elapsed, 1.25, places=9)
        self.assertEqual(len(reader.page_schedule), 6)

    def test_http_refusal_escapes_once_and_is_retained(self):
        reader = PacedQueryReader(WorkloadReader('https://localhost:1', None, 'synthetic-reader-fixture'))
        error = urllib.error.HTTPError('https://localhost:1/v1/console/query', 429, 'synthetic refusal', {}, None)
        with patch('release_runtime.urllib.request.urlopen', side_effect=error) as request:
            with self.assertRaises(urllib.error.HTTPError) as caught:
                reader.pages({'kind': 'metrics'})
        self.assertIs(caught.exception, error)
        self.assertEqual(request.call_count, 1)
        self.assertEqual(reader.page_schedule[0]['status'], 429)
        self.assertEqual(reader.page_schedule[0]['outcome'], 'http_error')


if __name__ == '__main__':
    unittest.main()
