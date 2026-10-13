"""Prospective independent controls; no shim/controller transition is imported."""
import copy
from pathlib import Path
import unittest

import session_clock_oracle as oracle

ROOT=Path(__file__).resolve().parents[2]


def clock_rows():
    rows=[]
    for sequence,offset in enumerate((0,1,1799,1800,28799,28800)):
        for clock in (0,1,4):
            raw=100_500_000_000 if clock==0 else 200_500_000_000
            rows.append({'pid':700,'sequence':sequence,'offset_s':offset,'clock_id':clock,
                         'raw_ns':raw,'returned_ns':raw+(offset*1_000_000_000 if clock==0 else 0),
                         'syscall_before_ns':raw-1_000_000,'syscall_after_ns':raw+1_000_000})
    return rows


def isolation():
    ordinary={role:{'pid':pid,'helper_maps':[],'control_env_names':[],'control_fd_present':False,
                    'syscall_before_ns':100_000_000_000,'clock_ns':100_001_000_000,'syscall_after_ns':100_002_000_000}
              for pid,role in enumerate(('controller','browser','driver','companion'),701)}
    return {'target_pid':700,'target_executable_sha256':'a'*64,'target_helper_maps':['b'*64],
            'target_control_cloexec':True,'target_control_type':'AF_UNIX/SOCK_SEQPACKET','ordinary_processes':ordinary,
            'fork':{'pid':705,'target_offset_s':1,'raw_ns':100_000_000_000,'returned_ns':100_000_000_000,
                    'control_sequence_before':1,'control_sequence_after':1,'exec_helper_maps':[],
                    'exec_control_env_names':[],'exec_control_fd_present':False}}


def sessions():
    metadata={'A':{'hash':'a'*64,'issued_s':1000,'expires_s':29800,'baseline_seen_s':1000},
              'B':{'hash':'b'*64,'issued_s':1000,'expires_s':29800,'baseline_seen_s':1000},
              'C':{'hash':'c'*64,'issued_s':10000,'expires_s':38800,'baseline_seen_s':10000}}
    rows=[]
    def add(cell,key,second,status,last_seen):
        anchor=metadata[key]
        rows.append({'cell':cell,'session_hash':anchor['hash'],'pid':700,'issued_s':anchor['issued_s'],
                     'expires_s':anchor['expires_s'],'last_seen_s':last_seen,'request_s':second,
                     'server_call_s':second,'status':status,'error':'sign in required' if status==401 else None})
    add('idle_before','A',2799,200,1000);add('idle_at','B',2800,401,1000)
    previous=10000
    for second in range(11000,38001,1000):add('absolute_keepalive','C',second,200,previous);previous=second
    add('absolute_before','C',38799,200,38000);add('absolute_at','C',38800,401,38799)
    return rows,metadata


class SessionClockTests(unittest.TestCase):
    def test_fixture_mechanism_sources_are_available_for_native_binding_admission(self):
        # This initial red is mechanism absence, never a reproduced product bug.
        for name in ('session_clock.py','session_clock.c','session_clock_probe.rs'):
            self.assertTrue((ROOT/'tools/ui'/name).is_file(), 'missing fixture mechanism: '+name)

    def test_literal_frames_and_bounds_reject_bad_wire_records(self):
        frames=[bytes.fromhex(line) for line in (
            '46434c4b7631000000000000000000000000000000000000',
            '46434c4b7631000001000000000000000100000000000000',
            '46434c4b7631000002000000000000000707000000000000',
            '46434c4b7631000003000000000000000807000000000000',
            '46434c4b7631000004000000000000007f70000000000000',
            '46434c4b7631000005000000000000008070000000000000')]
        oracle.check_frames(frames)
        maximum=[b'FCLKv1\0\0'+i.to_bytes(8,'little')+i.to_bytes(8,'little') for i in range(64)]
        oracle.check_frames(maximum)
        for bad,error in [([], 'frame-count'),(maximum+[maximum[-1]],'frame-count'),
                          ([frames[0][:-1]],'frame-length'),([frames[0]+b'x'],'frame-length'),
                          ([b'wrong-v1'+frames[0][8:]],'frame-version'),
                          ([frames[0][:16]+(1).to_bytes(8,'little')],'frame-offset'),
                          ([frames[0],frames[0]],'frame-sequence'),([frames[0],frames[2]],'frame-sequence'),
                          ([frames[0],frames[1],frames[2][:16]+bytes(8)],'frame-offset'),
                          ([frames[0],frames[1][:16]+(28861).to_bytes(8,'little')],'frame-offset'),
                          ([frames[0],frames[1][:16]+(2**64-1).to_bytes(8,'little')],'frame-offset')]:
            with self.subTest(error=error),self.assertRaisesRegex(ValueError,error):oracle.check_frames(bad)

    def test_independent_raw_clock_and_sleep_controls_reject_shifted_or_unbound_evidence(self):
        oracle.check_probe_clocks(clock_rows(),700)
        for index,field,value,error in [(3,'returned_ns',100_500_000_000,'clock-realtime-only'),
                                       (1,'returned_ns',201_500_000_000,'clock-realtime-only'),
                                       (3,'returned_ns',101_500_000_001,'clock-realtime-only'),
                                       (3,'pid',701,'clock-pid'),(3,'sequence',0,'clock-binding'),
                                       (0,'syscall_after_ns',102_000_000_000,'clock-bracket-inconclusive'),
                                       (0,'raw_ns',100_000_000_000,'clock-raw-bracket')]:
            changed=clock_rows();changed[index][field]=value
            with self.subTest(field=field),self.assertRaisesRegex(ValueError,error):oracle.check_probe_clocks(changed,700)
        with self.assertRaisesRegex(ValueError,'clock-completeness'):oracle.check_probe_clocks(clock_rows()[:-1],700)
        sleep={'start_before_ns':200_000_000_000,'start_after_ns':200_001_000_000,
               'end_before_ns':200_502_000_000,'end_after_ns':200_503_000_000,'instant_elapsed_ns':502_000_000}
        oracle.check_sleep(sleep)
        with self.assertRaisesRegex(ValueError,'sleep-monotonic'):oracle.check_sleep(dict(sleep,instant_elapsed_ns=1_502_000_000))

    def test_target_maps_fork_and_child_escape_controls_are_independent(self):
        def check(row):oracle.check_isolation(row,700,'b'*64,'a'*64)
        check(isolation())
        for field,value,error in [('target_pid',701,'isolation-target'),('target_helper_maps',[],'isolation-interception'),
                                  ('target_helper_maps',['c'*64],'isolation-interception'),('target_control_cloexec',False,'isolation-control'),
                                  ('target_executable_sha256','d'*64,'isolation-target')]:
            changed=isolation();changed[field]=value
            with self.subTest(field=field),self.assertRaisesRegex(ValueError,error):check(changed)
        for role in ('controller','browser','driver','companion'):
            for field,value in [('helper_maps',['b'*64]),('control_env_names',['FABRIC_CLOCK_FD']),('control_fd_present',True)]:
                changed=isolation();changed['ordinary_processes'][role][field]=value
                with self.subTest(role=role,field=field),self.assertRaisesRegex(ValueError,'isolation-escape'):check(changed)
            changed=isolation();changed['ordinary_processes'][role]['clock_ns']+=1_000_000_000
            with self.subTest(role=role),self.assertRaisesRegex(ValueError,'isolation-host-clock'):check(changed)
        for field,value,error in [('returned_ns',101_000_000_000,'isolation-fork-consumption'),
                                  ('control_sequence_after',2,'isolation-fork-consumption'),
                                  ('exec_helper_maps',['b'*64],'isolation-exec-escape'),
                                  ('exec_control_env_names',['LD_PRELOAD'],'isolation-exec-escape'),
                                  ('exec_control_env_names',['FABRIC_CLOCK_FD'],'isolation-exec-escape'),
                                  ('exec_control_fd_present',True,'isolation-exec-escape')]:
            changed=isolation();changed['fork'][field]=value
            with self.subTest(field=field),self.assertRaisesRegex(ValueError,error):check(changed)

    def test_literal_session_boundaries_reject_wrong_refusal_baseline_or_token(self):
        rows,metadata=sessions();oracle.check_sessions(rows,metadata,700)
        for index,field,value,error in [(1,'status',200,'session-refusal'),(1,'status',429,'session-refusal'),
                                       (1,'status',403,'session-refusal'),(1,'status',None,'session-refusal'),
                                       (1,'last_seen_s',1001,'session-baseline'),(1,'request_s',2801,'session-boundary'),
                                       (-1,'server_call_s',38801,'session-boundary-inconclusive'),
                                       (-1,'session_hash','d'*64,'session-binding'),(-1,'issued_s',10001,'session-baseline'),
                                       (-1,'error','access denied','session-refusal')]:
            changed=copy.deepcopy(rows);changed[index][field]=value
            with self.subTest(field=field),self.assertRaisesRegex(ValueError,error):oracle.check_sessions(changed,metadata,700)
        with self.assertRaisesRegex(ValueError,'session-completeness'):oracle.check_sessions(rows[:2]+rows[3:],metadata,700)


if __name__=='__main__':unittest.main()
