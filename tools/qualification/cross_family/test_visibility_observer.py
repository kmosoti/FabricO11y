"""Controls for the retained smoke's grouped-bracket and blocked-loop defects."""
import json
import io
from pathlib import Path
import tempfile
import threading
import types
import unittest
import urllib.error
import time
from concurrent.futures import ThreadPoolExecutor
from unittest.mock import Mock, patch

import measure


class Guest:
    def __init__(self, role):
        self.boot_id = role
        self.at = 15 * 10**9
        self.lock = threading.Lock()

    def correlation(self):
        with self.lock:
            self.at += 10**9
            return {'boot_id': self.boot_id, 'monotonic_before_ns': self.at,
                    'monotonic_after_ns': self.at, 'unix_ns': self.at + 10**18}


class ObserverControls(unittest.TestCase):
    def test_independent_thread_covers_named_signals_inside_guest_clock_envelopes(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            calls = []
            def call(route, body, method):
                if body['kind'] == 'metrics' and not isinstance(body.get('name'), str):
                    raise urllib.error.HTTPError('http://fixture', 400, 'metric name required', {}, io.BytesIO())
                calls.append((body['node'], body['kind'], body.get('name')))
                return {'rows': []}
            observer = measure.VisibilityObserver(Guest('server'), Guest('edge'), call, out)
            observer.start({'monotonic_ns': 0})
            observer.finish()
            self.assertFalse(observer.thread.is_alive())
            self.assertTrue(observer.completed)
            expected = {(node, kind, None) for node in ('edge', measure.SELF_LABEL) for kind in ('logs', 'spans')}
            expected |= {(node, 'metrics', name) for node in ('edge', measure.SELF_LABEL) for name in measure.NATIVE_METRICS}
            expected.add(('controlled', 'metrics', 'cross.counter'))
            self.assertEqual(set(calls), expected)
            for row in observer.queries:
                self.assertEqual(row['after']['monotonic_after_ns'] - row['before']['monotonic_before_ns'], 10**9)
                self.assertEqual(row['before']['boot_id'], row['after']['boot_id'])
                self.assertEqual(row['query']['from_ns'], row['before']['unix_ns'] - 10*10**9)
            saved = [json.loads(line) for line in (out/'visibility-observer.jsonl').read_text().splitlines()]
            self.assertEqual(saved, observer.queries)
            self.assertEqual((out/'visibility-observer.jsonl').stat().st_mode & 0o777, 0o600)
            self.assertTrue(json.loads((out/'visibility-observer-completion.json').read_text())['stopped'])

    def test_original_http_failure_cannot_be_completed_observation(self):
        with tempfile.TemporaryDirectory() as directory:
            response = io.BytesIO(b'original refusal body')
            def call(*args):
                raise urllib.error.HTTPError('http://fixture', 503, 'injected refusal', {}, response)
            observer = measure.VisibilityObserver(Guest('server'), Guest('edge'), call, Path(directory))
            observer.start({'monotonic_ns': 0})
            with self.assertRaisesRegex(RuntimeError, 'observer incomplete'):
                observer.finish()
            self.assertFalse(observer.thread.is_alive())
            self.assertEqual(observer.error['status'], 503)
            self.assertTrue(response.closed)
            self.assertFalse(observer.completed)
            error = json.loads((Path(directory)/'visibility-observer-error.json').read_text())
            self.assertEqual(error['status'], 503)

    def test_sweep_uses_bounded_concurrency_and_brackets_every_completed_answer(self):
        active = peak = 0
        lock, ready = threading.Lock(), threading.Event()
        completions = []
        class LiveGuest:
            def correlation(self):
                now = time.monotonic_ns()
                return {'boot_id': 'test', 'monotonic_before_ns': now,
                        'monotonic_after_ns': now, 'unix_ns': time.time_ns()}
        def call(route, body, method):
            nonlocal active, peak
            began = time.monotonic_ns()
            with lock:
                active += 1
                peak = max(peak, active)
                if active == 8:
                    ready.set()
            try:
                if not ready.wait(timeout=2):
                    raise AssertionError('original serial sweep cannot release the eight-request barrier')
                if body['kind'] == 'metrics' and not isinstance(body.get('name'), str):
                    raise AssertionError('legacy API requires a metric name')
                time.sleep(.01)
                return {'rows': []}
            finally:
                with lock:
                    active -= 1
                    completions.append((began, time.monotonic_ns()))
        with tempfile.TemporaryDirectory() as directory, ThreadPoolExecutor(max_workers=8) as pool:
            observer = measure.VisibilityObserver(None, None, call, Path(directory))
            output = io.StringIO()
            observer.sweep(LiveGuest(), 'edge', output, pool)
            self.assertEqual(peak, 8)
            self.assertEqual(active, 0)
            self.assertEqual(len(observer.queries), 19)
            first = observer.queries[0]
            self.assertTrue(all(first['before']['monotonic_before_ns'] <= start <= end
                <= first['after']['monotonic_after_ns'] for start, end in completions))
            self.assertTrue(all(row['before'] == first['before'] and row['after'] == first['after']
                                for row in observer.queries))

    def test_first_observer_error_is_persisted_and_refuses_next_original_request(self):
        def original(server, edge, call, config, result, port, observer):
            observer.fail({'type': 'HTTPError', 'error': 'original 400', 'status': 400})
            call('/v1/admin/query', {}, 'POST')
            return {'queries': []}
        with tempfile.TemporaryDirectory() as directory, patch.object(measure, 'collect_original', side_effect=original):
            client = Mock()
            with self.assertRaisesRegex(RuntimeError, 'observer failed'):
                measure.collect(None, None, client, {}, Path(directory), None)
            client.assert_not_called()
            saved = json.loads((Path(directory)/'visibility-observer-error.json').read_text())
            self.assertEqual(saved['status'], 400)

    def test_cancel_and_unstopped_worker_remain_distinct_from_completion(self):
        with tempfile.TemporaryDirectory() as directory:
            observer = measure.VisibilityObserver(Guest('server'), Guest('edge'), None, Path(directory))
            observer.finish(cancel=True)
            self.assertFalse(observer.completed)
            observer.thread = types.SimpleNamespace(join=lambda timeout: None, is_alive=lambda: True)
            with self.assertRaisesRegex(RuntimeError, 'observer incomplete'):
                observer.finish(cancel=True)
            value = json.loads((Path(directory)/'visibility-observer-completion.json').read_text())
            self.assertFalse(value['stopped'])
            self.assertFalse(value['completed'])
