#!/usr/bin/env python3
"""Small filesystem-only controls for A12's inherited cgroup-limit guard."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

FIXTURE = Path(__file__).with_name('a12-memory-pressure.py')
SPEC = importlib.util.spec_from_file_location('a12_memory_pressure', FIXTURE)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class EffectiveLimitTests(unittest.TestCase):
    def test_missing_root_controls_are_unlimited_and_slice_caps_inherit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            slice_group = root / 'system.slice/system-fabrico11y.slice'
            unit = slice_group / 'fabric-accept-slice.service'
            unit.mkdir(parents=True)
            for name, value in {
                'memory.high': '2818572288',
                'memory.max': '3489660928',
                'memory.swap.max': '0',
                'pids.max': '640',
            }.items():
                (slice_group / name).write_text(value)
            # The unit and cgroup root intentionally have no controller files.
            observed = MODULE.effective_limits(unit, root)
            self.assertEqual(observed, {
                'memory.high': 2818572288,
                'memory.max': 3489660928,
                'memory.swap.max': 0,
                'pids.max': 640,
            })

    def test_tighter_leaf_limit_is_not_hidden_by_slice_limit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            slice_group = root / 'system.slice/system-fabrico11y.slice'
            unit = slice_group / 'fabric-accept-mem.service'
            unit.mkdir(parents=True)
            for name, value in {
                'memory.high': '134217728',
                'memory.max': '268435456',
                'memory.swap.max': '0',
                'pids.max': '128',
            }.items():
                (unit / name).write_text(value)
            for name, value in {
                'memory.high': '2952790016',
                'memory.max': '3489660928',
                'pids.max': '640',
            }.items():
                (slice_group / name).write_text(value)
            observed = MODULE.effective_limits(unit, root)
            self.assertEqual(observed['memory.max'], 268435456)
            self.assertEqual(observed['memory.high'], 134217728)
            self.assertEqual(observed['pids.max'], 128)
            self.assertEqual(observed['memory.swap.max'], 0)


if __name__ == '__main__':
    unittest.main()
