"""Controls for the retained smoke's grouped-bracket and blocked-loop defects."""
import json
from pathlib import Path
import tempfile
import threading
import types
import unittest
import urllib.error

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
    def test_independent_thread_covers_all_signals_with_individual_brackets(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            calls = []
            def call(route, body, method):
                calls.append((body['node'], body['kind'], body.get('name')))
                return {'rows': []}
            observer = measure.VisibilityObserver(Guest('server'), Guest('edge'), call, out)
            observer.start({'monotonic_ns': 0})
            observer.finish()
            self.assertFalse(observer.thread.is_alive())
            self.assertTrue(observer.completed)
            self.assertEqual(set(calls), {('edge', k, None) for k in ('logs', 'spans', 'metrics')}
                | {(measure.SELF_LABEL, k, None) for k in ('logs', 'spans', 'metrics')}
                | {('controlled', 'metrics', 'cross.counter')})
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
            def call(*args):
                raise urllib.error.HTTPError('http://fixture', 503, 'injected refusal', {}, None)
            observer = measure.VisibilityObserver(Guest('server'), Guest('edge'), call, Path(directory))
            observer.start({'monotonic_ns': 0})
            with self.assertRaisesRegex(RuntimeError, 'observer incomplete'):
                observer.finish()
            self.assertFalse(observer.thread.is_alive())
            self.assertEqual(observer.error['status'], 503)
            self.assertFalse(observer.completed)

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
