"""Typed operational-control records; no native attestation."""
import json, time, unittest
import custody_native as native

class TransitionTypes(unittest.TestCase):
    def test_noninteger_control_numbers_refuse(self):
        for key in ('desired_revision', 'applied_revision', 'metric_interval_s'):
            with self.subTest(field=key):
                row=dict(name='spindle-1', status='paused', desired_revision=2,
                    applied_revision=2, config_error=None, logs=native.NATIVE_PROFILE['logs'], metric_interval_s=15)
                row[key]=float(row[key])
                guest=native.Guest.__new__(native.Guest)
                guest.deadline=time.monotonic()+1
                guest.command=lambda argv, check=True: dict(stdout=json.dumps(dict(nodes=[row])))
                with self.assertRaises(ValueError):
                    guest.transition(2, True, require_applied=True, issue=False)

    def test_lagging_application_does_not_repeat_action(self):
        row=dict(name='spindle-1', status='paused', desired_revision=2,
            applied_revision=1, config_error=None, logs=native.NATIVE_PROFILE['logs'], metric_interval_s=15)
        guest=native.Guest.__new__(native.Guest)
        guest.deadline=time.monotonic()+0.01
        calls=[]
        def command(argv, check=True):
            calls.append(argv)
            return dict(stdout=json.dumps(dict(nodes=[row])))
        guest.command=command
        with self.assertRaisesRegex(ValueError, 'native applied profile deadline'):
            guest.transition(2, True, require_applied=True)
        self.assertEqual(sum('pause' in argv for argv in calls),1)

    def test_unrecognized_inspect_failure_refuses(self):
        guest=native.Guest.__new__(native.Guest)
        guest.deadline=time.monotonic()+1
        guest.command=lambda argv, check=True: dict(exit=1,stderr='fabricctl: unrelated disk failure\n')
        with self.assertRaisesRegex(ValueError,'native spool inspection failed'):
            guest.drain_prefix()
