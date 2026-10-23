"""Literal pause-sidecar controls; no Rust encoder or product timing borrowed."""
import copy
import hashlib
import json
import os
import stat
import unittest
from test_custody_native import NativeFixture, SOURCE
from test_custody_witness import adapter
from test_custody_profile import profile, raw_profile


def proof():
    prefix = ['fabricctl', 'admin', '/root/accept/admin.conf', 'node']
    command = dict(exit=0, stderr='', stdout='', started_ns=100, finished_ns=110, elapsed_ns=10)
    row = dict(name='spindle-1', desired_revision=2, applied_revision=2, status='paused',
               config_error=None, logs=profile(2, True)['logs'], metric_interval_s=15)
    return dict(boot_id='literal-boot-a', request=dict(command, argv=prefix + ['pause', 'spindle-1']),
        confirmation=dict(command, argv=prefix + ['list'], started_ns=120, finished_ns=130,
                          stdout=json.dumps(dict(nodes=[row]))))


class PauseMarkerControls(NativeFixture, unittest.TestCase):
    def setUp(self):
        super().setUp()
        self.module = adapter()

    def marker_snapshot(self, *, observation=True, change=None, receipt_change=None):
        def prepare(root):
            (root / 'applied-config.json').write_bytes(raw_profile(profile(2, True)))
            (root / 'applied-config.json').chmod(0o600)
            (root / 'coverage-unknown').write_bytes(b'115\n')
            (root / 'coverage-unknown').chmod(0o600)
            if change: change(root)
        def receipt(value):
            if observation: value['pause_observation'] = proof()
            if receipt_change: receipt_change(value)
        return self.snapshot('drain', [], 0, b'', change=prepare, receipt_change=receipt)

    def test_literal_marker_without_pause_proof_remains_refused(self):
        with self.assertRaisesRegex(ValueError, 'unexpected or failed Spool input'):
            self.marker_snapshot(observation=False)

    def test_original_bounded_pause_marker_is_preserved(self):
        snapshot = self.marker_snapshot()
        self.assertEqual(snapshot['coverage_pause'], dict(raw=b'115\n', observation=proof()))
        self.assertEqual(snapshot['files']['coverage-unknown']['sha256'], hashlib.sha256(b'115\n').hexdigest())

    def test_staged_sidecar_remains_refused(self):
        with self.assertRaisesRegex(ValueError, 'unexpected or failed Spool input'):
            self.marker_snapshot(change=lambda root: (root / 'coverage-unknown.tmp').write_bytes(b'115\n'))

    def test_marker_link_and_mode_refuse(self):
        for mutation in ('mode', 'hardlink', 'symlink'):
            with self.subTest(mutation=mutation):
                def change(root):
                    p = root / 'coverage-unknown'
                    if mutation == 'mode': p.chmod(0o644)
                    elif mutation == 'hardlink': os.link(p, self.root / 'alias')
                    else:
                        p.unlink(); p.symlink_to(root / 'applied-config.json')
                with self.assertRaises(ValueError): self.marker_snapshot(change=change)
                alias = self.root / 'alias'
                if alias.exists(): alias.unlink()
                # Next case must start with a regular private file.
                (self.spool / 'coverage-unknown').unlink()

    def test_proof_time_metadata_and_profile_mutations_refuse(self):
        info = (1, 2, stat.S_IFREG | 0o600, os.geteuid(), os.getegid(), 1, 4, 0, 0)
        service = dict(uid=os.geteuid(), gid=os.getegid())
        mutations = {
            'missing': lambda p: p.clear(),
            'boot': lambda p: p.update(boot_id='other'),
            'request': lambda p: p['request'].update(argv=['true']),
            'exit': lambda p: p['request'].update(exit=1),
            'boolean-exit': lambda p: p['request'].update(exit=False),
            'late-start': lambda p: p['request'].update(started_ns=116, finished_ns=117),
            'early-end': lambda p: p['confirmation'].update(started_ns=111, finished_ns=114),
            'deadline': lambda p: p['confirmation'].update(finished_ns=30_000_000_101),
            'negative-elapsed': lambda p: p['request'].update(elapsed_ns=-1),
            'empty-nodes': lambda p: p['confirmation'].update(stdout='{"nodes":[]}'),
            'duplicate-node': lambda p: p['confirmation'].update(stdout=json.dumps(dict(nodes=[json.loads(p['confirmation']['stdout'])['nodes'][0]]*2))),
            'revision': lambda p: p['confirmation'].update(stdout=p['confirmation']['stdout'].replace('"applied_revision": 2', '"applied_revision": 1')),
        }
        for label, mutation in mutations.items():
            with self.subTest(label=label):
                value=proof(); mutation(value)
                with self.assertRaisesRegex(ValueError, 'native pause marker'):
                    self.module.pause_marker(b'115\n', info, service, profile(2, True), value, 'literal-boot-a')
        for raw in (b'0\n', b'-1\n', b'115', b'0115\n', b'18446744073709551616\n', b'114\nextra'):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                self.module.pause_marker(raw, (*info[:6], len(raw), *info[7:]), service, profile(2, True), proof(), 'literal-boot-a')
        for config in (profile(1, False), profile(3, True), profile(2, True, interval=16), profile(2, True, logs=[])):
            with self.subTest(config=config), self.assertRaises(ValueError):
                self.module.pause_marker(b'115\n', info, service, config, proof(), 'literal-boot-a')
        with self.assertRaises(ValueError):
            self.module.pause_marker(b'115\n', info, dict(uid=999999, gid=999999), profile(2, True), proof(), 'literal-boot-a')


if __name__ == '__main__': unittest.main()
