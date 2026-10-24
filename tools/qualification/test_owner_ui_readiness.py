"""Run13: a missing Connect control must not silently consume the action."""
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from owner_walkthrough import Walkthrough


class DeferredControl:
    def __init__(self, *, setup=False, mount_after=2):
        self.setup, self.mount_after = setup, mount_after
        self.ticks, self.clicks, self.skipped = 0, 0, 0

    def _js(self, script):
        present = not self.setup and self.ticks >= self.mount_after
        if script.startswith('return {ready:'):
            return dict(ready='complete', connect=present, setup=self.setup,
                        working=False, requests=[])
        if script.startswith('return ('):
            return self.setup or (' || ' in script and present)
        if present:
            self.clicks += 1
            self.setup = True
        else:
            self.skipped += 1


class OwnerUiReadinessTests(unittest.TestCase):
    def walk(self, bridge, out=None):
        walk = Walkthrough.__new__(Walkthrough)
        walk.bridge, walk.out, walk.record = bridge, out, {}
        walk.commands = SimpleNamespace(remaining=lambda: 20)
        return walk

    def test_delayed_real_control_is_clicked_once_after_it_exists(self):
        bridge = DeferredControl()
        walk = self.walk(bridge)
        with patch('owner_walkthrough.time.sleep', side_effect=lambda _: setattr(bridge, 'ticks', bridge.ticks + 1)):
            walk.connect_owner_setup()
        self.assertTrue(bridge.setup)
        self.assertEqual((bridge.clicks, bridge.skipped), (1, 0))
        # The former producer loses the same action before the control mounts.
        legacy = DeferredControl()
        legacy._js("const e=document.querySelector('[data-testid=connect-server]');if(e)e.click();")
        legacy.ticks = 2
        self.assertFalse(legacy.setup)
        self.assertEqual(legacy.skipped, 1)

    def test_already_open_setup_needs_no_connect_control(self):
        bridge = DeferredControl(setup=True)
        self.walk(bridge).connect_owner_setup()
        self.assertEqual(bridge.clicks, 0)

    def test_timeout_retains_public_state_and_still_fails(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            out = Path(directory)
            (out / 'protected-transcript').mkdir()
            walk = self.walk(DeferredControl(mount_after=1000), out)
            with patch('owner_walkthrough.time.monotonic', side_effect=[0, 21]):
                with self.assertRaisesRegex(TimeoutError, 'documented browser action'):
                    walk.connect_owner_setup()
            files = list((out / 'protected-transcript').glob('ui-timeout-*.json'))
            self.assertEqual(len(files), 1)
            value = json.loads(files[0].read_text())
            self.assertEqual(set(value['observation']), {'ready', 'connect', 'setup', 'working', 'requests'})
            self.assertFalse(value['observation']['connect'])
            self.assertEqual(files[0].stat().st_mode & 0o777, 0o600)

    def test_failed_diagnostic_does_not_replace_timeout(self):
        walk = self.walk(DeferredControl(mount_after=1000), Path('/unused'))
        with patch('owner_walkthrough.time.monotonic', side_effect=[0, 21]), patch('owner_walkthrough.private_write', side_effect=OSError('fixture')):
            with self.assertRaisesRegex(TimeoutError, 'documented browser action'):
                walk.connect_owner_setup()
        self.assertEqual(walk.record['ui_timeout_observation_error'], 'OSError')
