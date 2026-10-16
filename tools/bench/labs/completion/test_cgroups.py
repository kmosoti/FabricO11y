"""Exact page-aligned cgroup limit controls for the completion harness."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import cgroups


class MemoryLimitValues(unittest.TestCase):
    def test_already_page_aligned_requests_are_unchanged(self):
        self.assertEqual(cgroups.memory_limit_values(8192, 4096, 4096), {
            'memory.max': '8192', 'memory.high': '4096',
        })

    def test_decimal_requests_round_down_to_full_pages(self):
        self.assertEqual(cgroups.memory_limit_values(4_000_000_000, 3_000_000_000, 4096), {
            'memory.max': '3999997952', 'memory.high': '2999996416',
        })

    def test_invalid_order_and_sub_page_limits_are_rejected(self):
        with self.assertRaises(ValueError):
            cgroups.memory_limit_values(4096, 8192, 4096)
        with self.assertRaises(ValueError):
            cgroups.memory_limit_values(4095, 1, 4096)

    def test_exact_readback_accepts_page_aligned_writes(self):
        with tempfile.TemporaryDirectory() as directory:
            group = Path(directory)
            for name in ('memory.max', 'memory.high', 'memory.swap.max', 'pids.max', 'cpu.max'):
                (group / name).write_text('')
            actual = cgroups.set_limits(group, 4_000_000_000, 3_000_000_000, 64, 2)
            self.assertEqual(actual['memory.max'], '3999997952')
            self.assertEqual(actual['memory.high'], '2999996416')

    def test_unexpected_higher_or_lower_readback_is_rejected(self):
        original_read_text = Path.read_text
        cases = (
            ('memory.max', 4096), ('memory.max', -4096),
            ('memory.high', 4096), ('memory.high', -4096),
        )
        for name, delta in cases:
            with self.subTest(name=name, delta=delta), tempfile.TemporaryDirectory() as directory:
                group = Path(directory)
                for leaf in ('memory.max', 'memory.high', 'memory.swap.max', 'pids.max', 'cpu.max'):
                    (group / leaf).write_text('')

                def wrong_readback(path, *args, **kwargs):
                    value = original_read_text(path, *args, **kwargs)
                    if path.name == name:
                        return str(int(value.strip()) + delta)
                    return value

                with patch.object(Path, 'read_text', wrong_readback):
                    with self.assertRaises(RuntimeError):
                        cgroups.set_limits(group, 4_000_000_000, 3_000_000_000, 64, 2)


class DelegationControllers(unittest.TestCase):
    def test_missing_io_fails_before_any_controller_write(self):
        with tempfile.TemporaryDirectory() as directory:
            group=Path(directory)
            (group/'cgroup.controllers').write_text('cpu memory pids')
            (group/'cgroup.subtree_control').write_text('')
            with self.assertRaisesRegex(RuntimeError,"unavailable:.*io"):
                cgroups.enable_controllers(group)
            self.assertEqual((group/'cgroup.subtree_control').read_text(),'')

    def test_all_four_required_and_kernel_readback_is_checked(self):
        for enabled in ('cpu io memory pids','cpu memory pids'):
            with self.subTest(enabled=enabled), tempfile.TemporaryDirectory() as directory:
                group=Path(directory)
                (group/'cgroup.controllers').write_text('cpu io memory pids cpuset')
                (group/'cgroup.subtree_control').write_text('')
                original=Path.write_text
                def kernel_write(path, value, *args, **kwargs):
                    self.assertEqual(set(value.split()),{'+cpu','+io','+memory','+pids'})
                    return original(path,enabled,*args,**kwargs)
                with patch.object(Path,'write_text',kernel_write):
                    if 'io' in enabled.split():cgroups.enable_controllers(group)
                    else:
                        with self.assertRaisesRegex(RuntimeError,'not enabled:.*io'):
                            cgroups.enable_controllers(group)


if __name__ == '__main__':
    unittest.main()
