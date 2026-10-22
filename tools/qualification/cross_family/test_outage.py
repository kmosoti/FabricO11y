"""A delayed rule installation must not shorten the actual interruption."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import outage


class OutageDuration(unittest.TestCase):
    def exercise(self, root, fail=False):
        clock = [10**9]
        commands = []
        def sleep(seconds):
            clock[0] += round(seconds * 10**9)
        def run(command, **kwargs):
            commands.append(command)
            clock[0] += 2*10**9  # Each real installation/inspection has a cost.
            if fail and command[1:3] == ['add', 'chain']:
                raise subprocess.CalledProcessError(5, command, '', 'injected installation failure')
            return subprocess.CompletedProcess(command, 0, json.dumps({'nftables': []}), '')
        output = root / 'outage.jsonl'
        with patch.object(sys, 'argv',
                ['outage.py', '--begin-ns', '1000000000', '--port', '7443', '--out', str(output)]), \
                patch.object(outage.time, 'monotonic_ns', side_effect=lambda: clock[0]), \
                patch.object(outage.time, 'sleep', side_effect=sleep), \
                patch.object(outage.subprocess, 'run', side_effect=run):
            if fail:
                with self.assertRaises(subprocess.CalledProcessError) as raised:
                    outage.main()
                self.assertEqual(raised.exception.returncode, 5)
            else:
                outage.main()
        return [json.loads(line) for line in output.read_text().splitlines()], commands

    def test_rule_installation_does_not_consume_the_sixty_second_hold(self):
        with tempfile.TemporaryDirectory() as name:
            rows, _ = self.exercise(Path(name))
            start, end = rows
            self.assertEqual((start['type'], end['type']), ('outage_start', 'outage_end'))
            self.assertEqual(start['after_ns'] - start['before_ns'], 6*10**9)
            self.assertGreaterEqual(end['before_ns'] - start['after_ns'], 60*10**9)
            self.assertEqual(start['boot_id'], end['boot_id'])

    def test_partial_install_preserves_original_failure_and_deletes_owned_table(self):
        with tempfile.TemporaryDirectory() as name:
            rows, commands = self.exercise(Path(name), fail=True)
            self.assertEqual([r['type'] for r in rows], ['interrupted_cleanup'])
            self.assertEqual(commands[-1], ['nft', 'delete', 'table', 'inet', 'fabric_cross'])
