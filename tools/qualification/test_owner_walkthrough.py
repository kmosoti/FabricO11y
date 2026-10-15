"""Synthetic caller controls; actual native/browser phases remain separate."""
import base64
import copy
import hashlib
import json
import os
from pathlib import Path
import tempfile
import types
import unittest
from unittest.mock import patch

import owner_walkthrough as W
from owner_evidence import grade_queries, bounded_edge_records, grade_custody
from cross_family import test_snapshot as snapshot_fixtures


class WalkthroughCallerTests(unittest.TestCase):
    def test_original_spool_custody_rejects_missing_duplicate_and_altered_recovery(self):
        from cross_family.producer import batch, metrics
        encoded = base64.b64encode(batch('a' * 32, 1, metrics(1, 10, 20))).decode()
        spool = [{'type': 'source', 'node_id': 'a' * 32, 'generation': 1, 'sequence': 1, 'bytes': encoded},
                 {'type': 'node_state', 'node_id': 'a' * 32, 'generation': 1,
                  'ack_cursor': 1, 'retained_sequences': [1]}]
        recovered = [{'label': 'edge', 'received_ns': 30, 'bytes': encoded}]
        self.assertTrue(grade_custody([spool], recovered)['delivery_oracle']['passed'])
        altered = [dict(recovered[0], bytes=base64.b64encode(batch('a' * 32, 1, metrics(9, 10, 20))).decode())]
        for records in ([], recovered * 2, altered):
            with self.subTest(records=len(records)), self.assertRaises(ValueError):
                grade_custody([spool], records)

    def test_effective_guest_limits_and_real_process_privileges(self):
        import owner_observation as observation
        service = 'fabrico11y-node.service'
        path = '/system.slice/system-fabrico11y.slice/' + service
        value = {'boot_id': 'literal-boot', 'account': {'uid': 991, 'gid': 991, 'shell': '/sbin/nologin'},
                 'processes': [{'pid': 123, 'start_ticks': 456, 'uids': [991] * 4, 'gids': [991] * 4,
                                'cap_eff': 0, 'no_new_privs': 1, 'exe': '/usr/bin/fabric-node', 'cgroup': '0::' + path}],
                 'units': {}}
        for name, high, maximum, tasks in ((service, 128, 256, 128), ('system-fabrico11y.slice', 2816, 3328, 640)):
            value['units'][name] = {'properties': {'ActiveState': 'active', 'MainPID': '123',
                'User': 'fabricolly', 'Group': 'fabricolly', 'ControlGroup': path,
                'EffectiveMemoryHigh': str(high * 1024**2), 'EffectiveMemoryMax': str(maximum * 1024**2),
                'EffectiveTasksMax': str(tasks)}, 'kernel': {'memory.high': str(high * 1024**2),
                'memory.max': str(maximum * 1024**2), 'memory.swap.max': '0' if name == service else 'max',
                'pids.max': str(tasks)}}
        observation.verify(value, 'edge', 'literal-boot', active=True)
        for defect in ('uid', 'capability', 'kernel', 'effective', 'companion'):
            changed = copy.deepcopy(value)
            if defect == 'uid': changed['processes'][0]['uids'] = [0] * 4
            elif defect == 'capability': changed['processes'][0]['cap_eff'] = 1
            elif defect == 'kernel': changed['units'][service]['kernel']['memory.max'] = 'max'
            elif defect == 'effective': changed['units'][service]['properties']['EffectiveMemoryMax'] = 'infinity'
            else: changed['processes'].append(copy.deepcopy(changed['processes'][0]))
            with self.subTest(defect=defect), self.assertRaises(ValueError):
                observation.verify(changed, 'edge', 'literal-boot', active=True)

    def test_cleanup_attempts_second_guest_after_first_raises(self):
        stopped = []
        def fail():
            stopped.append('server')
            raise OSError('literal stop failure')
        walk = types.SimpleNamespace(bridge=None,
            server=types.SimpleNamespace(role='server', stop=fail),
            edge=types.SimpleNamespace(role='edge', stop=lambda: stopped.append('edge') or True))
        errors = W.stop_owned(walk, None)
        self.assertEqual(stopped, ['server', 'edge'])
        self.assertEqual(errors, ['server: OSError: literal stop failure'])

    def test_independent_ack_create_bounds_not_opaque_answer_select_prefix(self):
        from cross_family.producer import batch, metrics
        records = [{'label': 'edge', 'bytes': base64.b64encode(batch('a' * 32, i, metrics(i, 10, 20))).decode(),
                    'received_ns': 30 + i} for i in (1, 2, 3)]
        before = dict(boot_id='literal-boot', node_id='a' * 32, generation=1, acked_through=2,
                      next_sequence=3, started_ns=1, finished_ns=2)
        proof = dict(before=before, after=dict(before, next_sequence=4, started_ns=5, finished_ns=6),
                     attempted_ns=3, completed_ns=4)
        self.assertEqual(bounded_edge_records(records, proof)[:2], (2, 3))
        for defect in ('node', 'generation', 'boot', 'clock', 'empty', 'wide', 'missing'):
            p, r = copy.deepcopy(proof), copy.deepcopy(records)
            if defect == 'node': p['after']['node_id'] = 'b' * 32
            elif defect == 'generation': p['after']['generation'] = 2
            elif defect == 'boot': p['after']['boot_id'] = 'different'
            elif defect == 'clock': p['after']['started_ns'] = 3
            elif defect == 'empty': p['before']['acked_through'] = 0
            elif defect == 'wide': p['after']['next_sequence'] = 35
            else: r.pop(0)
            with self.subTest(defect=defect), self.assertRaises(ValueError):
                bounded_edge_records(r, p)

    def test_stopped_writer_inventory_cannot_be_substituted_by_inactive_unit(self):
        import owner_observation as observation
        value = dict(boot_id='literal-boot', account=dict(uid=991, gid=991, shell='/sbin/nologin'),
                     processes=[], units={name: {'properties': {'ActiveState': 'inactive', 'MainPID': '0'},
                                                'kernel': {}}
                     for name in ('fabrico11y-node.service', 'fabrico11y-server.service')})
        self.assertIs(observation.verify(value, 'server', 'literal-boot', active=False), value)
        value['processes'] = [{'pid': 123, 'exe': '/usr/bin/fabric-node'}]
        with self.assertRaisesRegex(ValueError, 'writers remain'):
            observation.verify(value, 'server', 'literal-boot', active=False)

    def test_wrong_candidate_source_refused_before_fixture_or_guest(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root = Path(directory)
            receipt = root / 'deb.json'
            receipt.write_text(json.dumps({'source': {'original_head': 'a' * 40, 'source_date_epoch': 17}}))
            artifact = root / 'payload'; artifact.write_bytes(b'literal candidate')
            identity = {'source_commit': 'b' * 40}
            for key, value in (('original_head', 'c' * 40), ('epoch', 18)):
                plan = {'harness_commit': 'd' * 40, 'guide_commit': 'e' * 40, 'harness_files': {},
                        'deb_receipt': str(receipt), 'rpm_receipt': str(receipt),
                        'original_head': 'a' * 40, 'epoch': 17, 'source_commit': 'b' * 40}
                plan[key] = value
                path = root / ('plan-' + key); path.write_text(json.dumps(plan))
                def git(argv, **kwargs):
                    return types.SimpleNamespace(stdout='d' * 40 if 'rev-parse' in argv else '')
                with patch.object(W.Q, 'run', side_effect=git), \
                     patch.object(W, 'validate_inputs', return_value={'identity': identity}), \
                     patch.object(W, 'Guest') as guest:
                    with self.assertRaisesRegex(ValueError, 'candidate source differs'):
                        W.admit(path, hashlib.sha256(path.read_bytes()).hexdigest())
                    guest.assert_not_called()

    def test_installed_payload_drift_cannot_reach_version_or_completed_stage(self):
        walk = W.Walkthrough.__new__(W.Walkthrough)
        walk.artifacts = {'identity': {'payload_identity': {'usr/bin/fabric-node': {'bytes': 3, 'sha256': 'a' * 64}}}}
        walk.record = {'events': []}
        walk.observed = lambda *args: json.dumps({'usr/bin/fabric-node': {'bytes': 3, 'sha256': 'b' * 64}})
        with self.assertRaisesRegex(ValueError, 'installed binaries/complete console differ'):
            walk.installed_payload(types.SimpleNamespace(role='edge', family='debian'), True)

    def test_corrupt_original_journal_cannot_supply_query_expectations(self):
        raw = snapshot_fixtures.IndependentSnapshots().fixture()
        for changed in (raw[:-1], b'BAD!' + raw[4:]):
            with self.subTest(size=len(changed)):
                with self.assertRaises(ValueError):
                    grade_queries([{'file': 'batches.faj', 'bytes': base64.b64encode(changed).decode()}], [], [], b'one\n')

    def test_replay_helper_cannot_replace_independent_original_bytes(self):
        raw = snapshot_fixtures.IndependentSnapshots().fixture()
        with self.assertRaisesRegex(ValueError, 'independent raw journal differs'):
            grade_queries([{'file': 'batches.faj', 'bytes': base64.b64encode(raw).decode()}], [], [], b'one\n')

    def test_unregistered_usernet_override_refused_before_guest_paths(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            path = Path(directory)
            with self.assertRaisesRegex(ValueError, 'registered walkthrough network'):
                W.Guest('server', 'debian', path, path, path / 'key', 4096, network='10.22.0.0/16')
            self.assertFalse((path / 'server').exists())


if __name__ == '__main__':
    unittest.main()
