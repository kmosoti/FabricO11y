"""Finite native enrollment states must not disguise credential identity drift."""
import copy
import unittest
import custody_native as native


def checkpoints():
    return [dict(enrollment=dict(desired=dict(logs=list(native.NATIVE_PROFILE['logs']), metric_interval_s=15),
        enrolled_unix_s=123, name='spindle-1', revision=revision, status=status, token_sha256='d'*64),
        files={'/etc/fabrico11y/node-token':dict(stat=[7,9,33184,0,987,1,65,11,11], sha256='a'*64,
            trimmed_sha256='d'*64, btrfs_identity=None)}, node_id='1'*32)
        for revision,status in zip((1,1,2,4),('active','active','paused','paused'))]


class CredentialCheckpointControls(unittest.TestCase):
    def test_registered_state_sequence_preserves_original_records(self):
        rows=checkpoints();before=copy.deepcopy(rows)
        native.credential_checkpoints(rows)
        self.assertEqual(rows,before)

    def test_wrong_states_and_immutable_field_drift_refuse(self):
        mutations={
            'wrong-revision':lambda r:r[2]['enrollment'].update(revision=3),
            'boolean-revision':lambda r:r[0]['enrollment'].update(revision=True),
            'wrong-status':lambda r:r[3]['enrollment'].update(status='active'),
            'wrong-desired':lambda r:r[2]['enrollment']['desired'].update(metric_interval_s=16),
            'time':lambda r:r[2]['enrollment'].update(enrolled_unix_s=124),
            'token':lambda r:r[2]['enrollment'].update(token_sha256='e'*64),
            'name':lambda r:r[2]['enrollment'].update(name='other'),
            'node':lambda r:r[3].update(node_id='2'*32),
            'bytes-hash':lambda r:r[2]['files']['/etc/fabrico11y/node-token'].update(sha256='e'*64),
            'file-stat':lambda r:r[2]['files']['/etc/fabrico11y/node-token']['stat'].__setitem__(1,10),
            'missing':lambda r:r[2]['enrollment'].pop('desired'),
            'cached':lambda r:r[2].update(enrollment={'revision':3}, enrollment_cached=copy.deepcopy(r[0]['enrollment'])),
        }
        for name,mutation in mutations.items():
            with self.subTest(name=name):
                rows=checkpoints();mutation(rows);before=copy.deepcopy(rows)
                with self.assertRaisesRegex(ValueError,'credential metadata/hash drift'):
                    native.credential_checkpoints(rows)
                self.assertEqual(rows,before)

    def test_btrfs_device_adjustment_requires_equal_stable_identity(self):
        rows=checkpoints()
        for row in rows:row['files']['/etc/fabrico11y/node-token']['btrfs_identity']=dict(uuid='a'*32,subvolume_id=257)
        rows[3]['files']['/etc/fabrico11y/node-token']['stat'][0]=99
        before=copy.deepcopy(rows);native.credential_checkpoints(rows);self.assertEqual(rows,before)
        rows[3]['files']['/etc/fabrico11y/node-token']['btrfs_identity']['subvolume_id']=258
        with self.assertRaisesRegex(ValueError,'credential metadata/hash drift'):native.credential_checkpoints(rows)


if __name__=='__main__':unittest.main()
