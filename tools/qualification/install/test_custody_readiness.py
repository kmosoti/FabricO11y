"""Actual Fedora03 refused-listener trace; synthetic timings never qualify a guest."""
import copy
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import custody_native as native

FIXTURE = Path(__file__).resolve().parents[3] / 'docs/experiments/formal/fixtures/installation-readiness-refusal.json'
REFUSAL = json.loads(FIXTURE.read_text())['commands'][-1]
ROW = dict(body=native.UNREAD, attributes={'log.file.path': native.SOURCE})


class ReadyReached(Exception):
    pass


class ReadinessControls(unittest.TestCase):
    def exercise(self, responses, *, phase='drain', late=False):
        clock, seen = [0.0], {}
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            class Guest(native.Guest):
                def __init__(self, config, actual_phase):
                    self.deadline, self.commands, self.calls = 200.0, [], 0
                    self.output = Path(directory)
                    self.responses = iter(responses)
                    seen['guest'] = self
                def command(self, argv, check=True):
                    if argv[0] != 'fabricctl':
                        result = dict(argv=argv, exit=0, stdout='', stderr='')
                    else:
                        seen.setdefault('query_deadlines', []).append(self.deadline)
                        result = copy.deepcopy(next(self.responses, REFUSAL))
                        result['argv'] = argv
                        if late: clock[0] += 41
                    self.commands.append(result)
                    native.witness.require(not check or result['exit'] == 0, 'observer command failed: '+argv[0])
                    return result
                def process(self):
                    self.calls += 1
                    if self.calls == 2: raise ReadyReached('readiness reached actual successful unread query')
                    return {}
                def save(self, name, value): seen[name] = value
                def finish(self): seen['finished'] = True
            def sleep(seconds): clock[0] += seconds
            with patch.object(native, 'Guest', Guest), patch.object(native.time, 'monotonic', lambda: clock[0]), patch.object(native.time, 'sleep', sleep):
                try: native.guest_main({}, phase)
                except BaseException as error: seen['error'] = error
            seen['clock'] = clock[0]
            seen['pages'] = [json.loads(p.read_text()) for p in Path(directory).glob('*query*.json')]
        return seen

    def success(self):
        return dict(exit=0, stderr='', stdout=json.dumps(dict(rows=[ROW], next_page=None, complete=True, gaps=[])))

    def test_exact_initial_refusal_then_success_both_phases(self):
        for phase in ('drain', 'reboot'):
            with self.subTest(phase=phase):
                result = self.exercise([REFUSAL, self.success()], phase=phase)
                self.assertIsInstance(result['error'], ReadyReached)
                self.assertEqual(result['clock'], 1)
                self.assertEqual(result['pages'], [json.loads(self.success()['stdout'])])
                self.assertEqual([r['exit'] for r in result['guest'].commands if r['argv'][0]=='fabricctl'], [2,0])
                self.assertEqual(result['query_deadlines'], [40,40])
                self.assertEqual(result['guest'].deadline, 200)

    def test_repeated_refusal_exhausts_one_deadline(self):
        result = self.exercise([])
        self.assertIsInstance(result['error'], ValueError)
        self.assertIn('native drain deadline', str(result['error']))
        self.assertEqual(result['clock'], 40)
        self.assertEqual(len(result['query_deadlines']), 40)
        self.assertEqual(result['guest'].deadline, 200)

    def test_late_success_cannot_bypass_absolute_deadline(self):
        result = self.exercise([self.success()], late=True)
        self.assertIsInstance(result['error'], ValueError)
        self.assertIn('native drain deadline', str(result['error']))
        self.assertEqual(result['guest'].deadline, 200)

    def test_unrelated_error_or_answer_bytes_remains_fatal(self):
        for change in (dict(stderr='fabricctl: unauthorized\n'), dict(exit=1), dict(stdout=self.success()['stdout']), dict(stderr=REFUSAL['stderr']+'extra\n')):
            with self.subTest(change=change):
                result = self.exercise([dict(REFUSAL, **change), self.success()])
                self.assertIsInstance(result['error'], ValueError)
                self.assertEqual(result['clock'], 0)
                self.assertEqual(len(result['query_deadlines']), 1)
                self.assertEqual(result['guest'].deadline, 200)

    def test_partial_page_then_refusal_is_fatal(self):
        page = dict(exit=0, stderr='', stdout=json.dumps(dict(rows=[], next_page='next', complete=True, gaps=[])))
        result = self.exercise([page, REFUSAL, self.success()])
        self.assertIsInstance(result['error'], ValueError)
        self.assertEqual(result['clock'], 0)
        self.assertEqual(len(result['query_deadlines']), 2)
        self.assertEqual(result['guest'].deadline, 200)
