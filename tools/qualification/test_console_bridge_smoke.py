import os
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import console_bridge_smoke as smoke

class SupervisedSmoke(unittest.TestCase):
    def setUp(self):
        self.fd, other = os.pipe()
        self.addCleanup(os.close, other)
        self.held = {'fd': self.fd, 'path': '/owned/browser'}
        self.bridge = SimpleNamespace(receipt={}, close=Mock())
        p = patch.object(smoke.cgroups, 'subgroup', return_value=(Path('/owned/browser'), {'memory.max':str(1024**3)}))
        self.subgroup = p.start(); self.addCleanup(p.stop)
        p = patch.object(smoke, 'browser_hold_directory', return_value=self.held)
        p.start(); self.addCleanup(p.stop)
        p = patch.object(smoke.time, 'monotonic', return_value=100.0)
        p.start(); self.addCleanup(p.stop)

    def context(self):
        return smoke.supervised_bridge('https://localhost:1', Path('/cert'), Path('/work'),
                                       Path('/out'), Path('/owned'), 300.0)

    def test_budget_and_borrowed_descriptor_lifetime(self):
        def construct(*args, **kwargs):
            os.fstat(kwargs['browser_parent']['fd'])
            self.assertTrue(kwargs['browser_parent']['exclusive'])
            self.assertEqual(kwargs['browser_deadline'], 300.0)
            self.assertEqual(kwargs['browser_group'], Path('/owned/browser'))
            return self.bridge
        with patch.object(smoke, 'ConsoleBridge', side_effect=construct):
            with self.context() as bridge:
                self.assertIs(bridge, self.bridge)
                with self.assertRaises(OSError): os.fstat(self.fd)
        self.subgroup.assert_called_once_with(Path('/owned'), 'browser', 1024**3, 850*1024**2, 512, 2)
        self.bridge.close.assert_called_once_with()
        self.assertEqual(self.bridge.shutdown_deadline, 120.0)

    def test_constructor_refusal_releases_original_descriptor(self):
        with patch.object(smoke, 'ConsoleBridge', side_effect=RuntimeError('refused')):
            with self.assertRaisesRegex(RuntimeError, 'refused'):
                with self.context(): self.fail('constructor should refuse')
        with self.assertRaises(OSError): os.fstat(self.fd)
        self.bridge.close.assert_not_called()

    def test_body_failure_still_closes_supervised_browser(self):
        with patch.object(smoke, 'ConsoleBridge', return_value=self.bridge):
            with self.assertRaisesRegex(ValueError, 'body'):
                with self.context(): raise ValueError('body')
        self.bridge.close.assert_called_once_with()

    def test_unconfirmed_cleanup_refuses_smoke_success(self):
        self.bridge.close.side_effect = RuntimeError('unconfirmed stop')
        with patch.object(smoke, 'ConsoleBridge', return_value=self.bridge):
            with self.assertRaisesRegex(RuntimeError, 'unconfirmed stop'):
                with self.context(): pass

if __name__ == '__main__': unittest.main()
