"""Independent grading cannot start until both clock receipts are complete."""
import json
from pathlib import Path
import tempfile
import types
import unittest
from unittest.mock import patch

import grade


class ClockCheckpoint(unittest.TestCase):
    def exercise(self, edge_ok):
        order=[]
        def guest(role, ok):
            def stop():order.append('stop-'+role);return ok
            return types.SimpleNamespace(role=role,stop_clock_channel=stop)
        class Observation(dict):
            def __getitem__(self, key):
                if key=='offers':
                    self_test.assertEqual(order[:3],['collect','stop-edge','stop-server'])
                    if 'independent-grading' not in order:order.append('independent-grading')
                    return []
                return super().__getitem__(key)
        self_test=self
        def collect(*args):order.append('collect');return Observation()
        with tempfile.TemporaryDirectory() as directory, patch('measure.collect',side_effect=collect), \
                patch.object(grade.query_oracle,'check') as oracle:
            result=Path(directory)
            args=(guest('server',True),guest('edge',edge_ok),None,{},result,1234)
            if edge_ok:
                gates=grade.measure_and_grade(*args)
                self.assertFalse(gates['producer']['passed'])  # intentionally empty diagnostic data
                self.assertIn('independent-grading',order)
            else:
                with self.assertRaisesRegex(RuntimeError,'completion required before independent grading'):
                    grade.measure_and_grade(*args)
                self.assertEqual(order,['collect','stop-edge','stop-server'])
            oracle.assert_not_called()
            checkpoint=json.loads((result/'clock-completion-checkpoint.json').read_text())
            self.assertEqual(checkpoint,dict(before_independent_grading=True,
                                            stopped=dict(edge=edge_ok,server=True)))

    def test_both_original_completions_precede_any_gate_work(self):self.exercise(True)

    def test_failed_completion_stops_both_and_refuses_grading(self):self.exercise(False)
