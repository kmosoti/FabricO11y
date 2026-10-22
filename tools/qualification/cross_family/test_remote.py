"""Lifecycle refusal controls: transport output is not completion evidence."""
import argparse
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import remote
import remote_worker


class RemoteControls(unittest.TestCase):
    def test_wrong_guest_or_receipt_cannot_be_transport_success(self):
        response = dict(event='stopped', run_id='one', boot_id='boot-one', qemu_stopped=True, exit=0)
        receipt = dict(unit='unit-one', command=['worker', 'one'], exit=0, stop_confirmed=True)
        kwargs = dict(run_id='one', boot_id='boot-one', unit='unit-one', command=['worker', 'one'], transport_exit=0)
        self.assertTrue(remote.completed_remote(response, receipt, **kwargs))
        for field, value in [('boot_id', 'another-guest'), ('run_id', 'another-run'),
                             ('exit', False), ('qemu_stopped', False)]:
            with self.subTest(response=field):
                self.assertFalse(remote.completed_remote({**response, field: value}, receipt, **kwargs))
        for field, value in [('unit', 'another-unit'), ('command', ['other']),
                             ('exit', 1), ('exit', False), ('stop_confirmed', False)]:
            with self.subTest(receipt=field, value=value):
                self.assertFalse(remote.completed_remote(response, {**receipt, field: value}, **kwargs))
        self.assertFalse(remote.completed_remote(response, None, **kwargs))
        self.assertFalse(remote.completed_remote(response, receipt, **{**kwargs, 'transport_exit': 255}))

    def test_partial_placement_and_unbounded_controller_refuse(self):
        args = argparse.Namespace(edge_host='olo@olo-g15', edge_storage_root=None,
                                  edge_source_root=None, edge_input_manifest=None)
        with self.assertRaises(ValueError):
            remote.validate_placement(args)
        args.edge_storage_root = args.edge_source_root = args.edge_input_manifest = Path('/fixture')
        with patch.dict(os.environ, {'FABRIC_RESOURCE_JOB_PROFILE': 'source'}):
            with self.assertRaisesRegex(ValueError, 'split-server'):
                remote.validate_placement(args)
        with self.assertRaises(ValueError):
            remote.remote_command('other-host', ['true'])

    def worker(self, action, *, defect=None):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'scratch/remote-control').mkdir(parents=True)
            (root / 'inputs').mkdir()
            key = root / 'scratch/remote-control/id_ed25519'
            key.write_text('private test fixture')
            key.chmod(0o600)
            source = root / 'inputs/source'
            source.write_text('pinned input')
            manifest = root / 'inputs/release-inputs.json'
            manifest.write_text(json.dumps({'files': {'inputs/source': hashlib.sha256(source.read_bytes()).hexdigest()}}))
            expected = hashlib.sha256(manifest.read_bytes()).hexdigest()
            if defect == 'wrong-hash':
                source.write_text('changed after staging')
            calls = []

            class FixtureGuest:
                boot_id = '11111111-1111-1111-1111-111111111111'
                port = 12345
                command = ['qemu', 'fixture']

                def __init__(self, *args, **kwargs):
                    pass

                def start(self):
                    calls.append('start')
                    if defect == 'startup':
                        raise RuntimeError('injected startup failure')

                def ready(self):
                    calls.append('ready')

                def stop(self):
                    calls.append('stop')
                    return True

            argv = ['worker', '--run-id', 'control', '--family', 'debian', '--manifest-sha256', expected]
            with patch.object(remote_worker.Q, 'DATA', root), \
                 patch.object(remote_worker, 'Guest', FixtureGuest), \
                 patch('resource_group.require_limits', return_value=Path('/sys/fs/cgroup/fabric-work-control.service')), \
                 patch.dict(os.environ, {'FABRIC_RESOURCE_JOB_PROFILE': 'edge'}), \
                 patch.object(sys, 'argv', argv), patch.object(sys, 'stdin', io.StringIO(action)), \
                 contextlib.redirect_stdout(io.StringIO()) as output:
                try:
                    status = remote_worker.main()
                except (RuntimeError, ValueError) as error:
                    status = str(error)
            return status, calls, [json.loads(line) for line in output.getvalue().splitlines()]

    def test_explicit_stop_records_original_success_and_shutdown(self):
        status, calls, events = self.worker('{"action":"stop"}\n')
        self.assertEqual(status, 0)
        self.assertEqual(calls, ['start', 'ready', 'stop'])
        self.assertEqual(events[-1]['event'], 'stopped')
        self.assertTrue(events[-1]['qemu_stopped'])
        self.assertEqual(events[-1]['exit'], 0)

    def test_disconnect_stops_guest_but_preserves_original_failure(self):
        status, calls, events = self.worker('')
        self.assertIn('disconnected', status)
        self.assertEqual(calls, ['start', 'ready', 'stop'])
        self.assertEqual(events[-1]['exit'], 1)
        self.assertTrue(events[-1]['qemu_stopped'])

    def test_startup_failure_still_stops_owned_guest(self):
        status, calls, events = self.worker('', defect='startup')
        self.assertIn('startup', status)
        self.assertEqual(calls, ['start', 'stop'])
        self.assertEqual(events[-1]['exit'], 1)

    def test_wrong_staged_hash_refuses_before_guest_start(self):
        status, calls, _ = self.worker('', defect='wrong-hash')
        self.assertIn('immutable input changed', status)
        self.assertEqual(calls, [])

    def test_stream_timeout_and_partial_evidence_refuse(self):
        with tempfile.TemporaryDirectory() as temporary:
            guest = object.__new__(remote.RemoteGuest)
            guest.buffer, guest.messages, guest.results = b'', [], Path(temporary)
            for program, expected in [
                ('import time; time.sleep(1)', TimeoutError),
                ('print(\'{"event":"ready"\',end="",flush=True)', RuntimeError),
            ]:
                child = subprocess.Popen([sys.executable, '-c', program], stdout=subprocess.PIPE)
                guest.child = child
                try:
                    with self.assertRaises(expected):
                        guest.message(time.monotonic() + .2)
                finally:
                    child.terminate()
                    child.wait(timeout=2)
                    child.stdout.close()
                    guest.buffer = b''


if __name__ == '__main__':
    unittest.main()
