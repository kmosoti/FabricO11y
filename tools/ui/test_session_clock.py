"""Independent literal oracle controls and separate fixture lifecycle regressions."""
import copy
from contextlib import contextmanager
import ctypes
import hashlib
import inspect
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import threading
from time import monotonic as real_monotonic, sleep
import unittest
from unittest.mock import Mock, patch

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


@contextmanager
def lifecycle_fixture():
    libc=ctypes.CDLL(None,use_errno=True);previous=ctypes.c_int()
    if libc.prctl(37,ctypes.byref(previous),0,0,0) or libc.prctl(36,1,0,0,0):raise RuntimeError('fixture subreaper unavailable')
    try:
        with tempfile.TemporaryDirectory(prefix='clock-lifecycle-',dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:yield Path(directory)
    finally:
        if libc.prctl(36,previous.value,0,0,0):raise RuntimeError('fixture subreaper restoration failed')


def marker_pid(marker):
    deadline=real_monotonic()+2
    while real_monotonic()<deadline:
        try:
            text=marker.read_text()
            if text.isdigit():return int(text)
        except FileNotFoundError:pass
        sleep(.005)
    raise RuntimeError('bounded fixture PID witness unavailable')


def reap_fixture(pid):
    deadline=real_monotonic()+5
    while real_monotonic()<deadline:
        try:
            if os.waitpid(pid,os.WNOHANG)[0]:return
        except ChildProcessError:
            if not Path(f'/proc/{pid}').exists():return
        sleep(.005)


def stop_fixture(pid):
    try:
        if os.waitpid(pid,os.WNOHANG)[0]:return
        os.kill(pid,signal.SIGKILL);os.waitpid(pid,0)
    except (ProcessLookupError,ChildProcessError):pass


class SessionClockLifecycleTests(unittest.TestCase):
    def test_exited_leader_does_not_leave_term_resistant_group_member(self):
        import session_clock as sut
        with lifecycle_fixture() as root:
            marker=root/'child.pid'
            program='import os,signal,sys,time;from pathlib import Path\nchild=os.fork()\nif not child:\n signal.signal(signal.SIGTERM,signal.SIG_IGN);Path(sys.argv[1]).write_text(str(os.getpid()))\n while True:signal.pause()\nwhile not Path(sys.argv[1]).exists():time.sleep(.005)\nos._exit(0)\n'
            process=subprocess.Popen([sys.executable,'-c',program,str(marker)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,start_new_session=True)
            pid=marker_pid(marker);process.wait(timeout=2);receiver=threading.Thread(target=reap_fixture,args=(pid,),daemon=True);receiver.start()
            fixture=sut.SessionClock.__new__(sut.SessionClock);fixture.process=process;fixture.receiver=receiver;fixture.parent=Mock();fixture.stderr=Mock()
            try:
                error=None
                try:fixture.close()
                except ValueError as failure:error=str(failure)
                observed=(error,receiver.is_alive())
            finally:
                stop_fixture(pid);receiver.join(timeout=2);process.stdin.close();process.stdout.close()
            self.assertEqual(observed,(None,False),'whole owned group must stop even after its leader has exited')

    def test_compiler_timeout_stops_descendant_and_preserves_output_and_partial_build(self):
        import session_clock as sut
        with lifecycle_fixture() as root:
            marker=root/'compiler-child.pid';compiler=root/'fixture-compiler';output=root/'receipt'
            compiler.write_text('#!'+sys.executable+'\nimport os,signal,sys,time\nfrom pathlib import Path\n'
                'if "-vV" in sys.argv:print("release: 1.99.0\\ncommit-hash: b940084d7");sys.exit(0)\n'
                'if "--version" in sys.argv:print("literal fixture compiler");sys.exit(0)\n'
                'Path(sys.argv[sys.argv.index("-o")+1]).write_bytes(b"partial-object-public-evidence")\n'
                'print("compiler-timeout-public-output",file=sys.stderr,flush=True)\nchild=os.fork()\n'
                'if not child:\n signal.signal(signal.SIGTERM,signal.SIG_IGN);Path(os.environ["FCLK_TEST_MARKER"]).write_text(str(os.getpid()))\n'
                'while True:signal.pause()\n');compiler.chmod(0o700)
            def watcher():
                reap_fixture(marker_pid(marker))
            receiver=threading.Thread(target=watcher,daemon=True);receiver.start();calls=0
            def clock():
                nonlocal calls
                calls+=1;return real_monotonic()+(0 if calls==1 else 239.25)
            try:
                with patch.dict(os.environ,FCLK_TEST_MARKER=str(marker)),patch.object(sut.time,'monotonic',clock),patch.object(sys,'argv',['session-clock','--output',str(output),'--rustc',str(compiler),'--cc',str(compiler)]):
                    with self.assertRaises(subprocess.TimeoutExpired):sut.main()
                receiver.join(timeout=1);pid=marker_pid(marker)
                blob=b''.join(path.read_bytes() for path in output.rglob('*') if path.is_file() and path.stat().st_size<=2*1024**2)
                observed=(Path(f'/proc/{pid}').exists(),b'compiler-timeout-public-output' in blob,b'partial-object-public-evidence' in blob)
            finally:
                if marker.exists():stop_fixture(marker_pid(marker))
                receiver.join(timeout=2)
            self.assertEqual(observed,(False,True,True),'timeout must stop descendants and archive captured output plus partial build evidence')

    def test_replaced_images_are_rejected_against_the_original_frozen_witness(self):
        import session_clock as sut
        with lifecycle_fixture() as root:
            executable=root/'probe';helper=root/'shim.so';libc=root/'libc.so.6';proc=root/'proc';proc.mkdir();libc.write_bytes(b'literal libc witness')
            (proc/'exe').symlink_to(executable);(proc/'stat').write_text('700 (fixture) S '+'0 '*18+'123\n');(proc/'cgroup').write_text('0::/literal-contained-fixture\n')
            for field,message in [('executable','isolation-target-frozen'),('helper','isolation-helper-frozen')]:
                with self.subTest(field=field):
                    executable.write_bytes(b'literal frozen executable');helper.write_bytes(b'literal frozen helper')
                    expected={'executable_sha256':hashlib.sha256(b'literal frozen executable').hexdigest(),'helper_sha256':hashlib.sha256(b'literal frozen helper').hexdigest(),
                              'executable_identity':{'device':executable.stat().st_dev,'inode':executable.stat().st_ino},'helper_identity':{'device':helper.stat().st_dev,'inode':helper.stat().st_ino}}
                    stat=helper.stat();(proc/'maps').write_text(f'1000-2000 r-xp 00000000 {os.major(stat.st_dev):02x}:{os.minor(stat.st_dev):02x} {stat.st_ino} {helper}\n2000-3000 r-xp 00000000 00:00 1 {libc}\n')
                    (executable if field=='executable' else helper).write_bytes(b'literal replacement after freeze')
                    # Legacy checker lacks the witness parameter and admits the real changed files.
                    options={'expected':expected} if 'expected' in inspect.signature(sut.process_binding).parameters else {}
                    refusal=None
                    with patch.object(sut,'Path',side_effect=lambda value:proc if str(value)=='/proc/700' else Path(value)):
                        try:sut.process_binding(700,executable,helper,**options)
                        except ValueError as failure:refusal=str(failure)
                    self.assertEqual(refusal,message,'current path agreement cannot replace the pre-launch frozen hashes and identities')

    def test_same_bytes_replacement_and_forged_mapping_reject_frozen_identity(self):
        import session_clock as sut
        with lifecycle_fixture() as root:
            executable=root/'probe';helper=root/'shim.so';libc=root/'libc.so.6';proc=root/'proc';proc.mkdir();libc.write_bytes(b'literal libc witness')
            (proc/'exe').symlink_to(executable);(proc/'stat').write_text('700 (fixture) S '+'0 '*18+'123\n');(proc/'cgroup').write_text('0::/literal-contained-fixture\n')
            for case in ('executable-replaced','helper-replaced','mapping-device','mapping-inode'):
                with self.subTest(case=case):
                    executable.write_bytes(b'literal frozen executable');helper.write_bytes(b'literal frozen helper')
                    expected={'executable_sha256':hashlib.sha256(b'literal frozen executable').hexdigest(),'helper_sha256':hashlib.sha256(b'literal frozen helper').hexdigest(),
                              'executable_identity':{'device':executable.stat().st_dev,'inode':executable.stat().st_ino},'helper_identity':{'device':helper.stat().st_dev,'inode':helper.stat().st_ino}}
                    if case.endswith('-replaced'):
                        selected=executable if case=='executable-replaced' else helper;frozen=selected.stat();replacement=root/'same-bytes-new-inode'
                        replacement.write_bytes(selected.read_bytes());self.assertNotEqual(replacement.stat().st_ino,frozen.st_ino)
                        replacement.replace(selected)
                        self.assertEqual(hashlib.sha256(selected.read_bytes()).hexdigest(),expected['executable_sha256' if selected==executable else 'helper_sha256'])
                        self.assertNotEqual(selected.stat().st_ino,frozen.st_ino)
                    stat=helper.stat();major=os.major(stat.st_dev)+(1 if case=='mapping-device' else 0);inode=stat.st_ino+(1 if case=='mapping-inode' else 0)
                    (proc/'maps').write_text(f'1000-2000 r-xp 00000000 {major:02x}:{os.minor(stat.st_dev):02x} {inode} {helper}\n2000-3000 r-xp 00000000 00:00 1 {libc}\n')
                    options={'expected':expected} if 'expected' in inspect.signature(sut.process_binding).parameters else {}
                    refusal=None
                    with patch.object(sut,'Path',side_effect=lambda value:proc if str(value)=='/proc/700' else Path(value)):
                        try:sut.process_binding(700,executable,helper,**options)
                        except ValueError as failure:refusal=str(failure)
                    self.assertEqual(refusal,'isolation-target-frozen' if case=='executable-replaced' else 'isolation-helper-frozen',
                                     'unchanged bytes cannot replace frozen file or loaded mapping device/inode witnesses')


def session_fixtures():
    # Independent literal receipts, not native process/state admission evidence.
    rows, metadata = sessions()
    fixtures = {}
    for key, pid, inode, start, stop in (
            ('AB', 700, 101, 2_000_000_000, 10_000_000_000),
            ('C', 701, 102, 11_000_000_000, 31_000_000_000)):
        state = {'path': '/owned/browser/state-' + key, 'device': 17, 'inode': inode}
        fixtures[key] = {
            'candidate_commit': '8fb6f413179b094288316e25845c137440ba23fa',
            'package_sha256': 'd' * 64, 'executable_sha256': 'e' * 64,
            'helper_sha256': 'f' * 64,
            'process_binding': {
                'pid': pid, 'start_ticks': pid + 100, 'cgroup': '0::/owned/browser-' + key + '\n',
                'executable_sha256': 'e' * 64, 'helper_sha256': 'f' * 64,
                'libc': '/owned/libc.so.6', 'libc_sha256': '1' * 64,
                'helper_maps': [['08:01', '99', '/owned/session_clock.so']]},
            'state_before': dict(state, created_monotonic_ns=start - 100_000_000,
                                 observed_monotonic_ns=start - 50_000_000,
                                 initial_entries=[], copy_source=None, descriptor_held=True),
            'state_after': dict(state, observed_monotonic_ns=stop - 1_000_000),
            'run_started_monotonic_ns': 1_000_000_000,
            'deadline_monotonic_ns': 241_000_000_000,
            'started_monotonic_ns': start, 'stopped_monotonic_ns': stop,
            'owned_groups': [{'pgid': pid, 'start_ticks': pid + 100}],
            'stop_observations': [{'pgid': pid, 'start_ticks': pid + 100,
                                   'observed_monotonic_ns': stop,
                                   'population': 0, 'confirmed': True}]}
    for index, row in enumerate(rows):
        row['pid'] = 700 if index < 2 else 701
        before = 3_000_000_000 + index * 600_000_000 if index < 2 else 12_000_000_000 + (index - 2) * 600_000_000
        row.update(request_before_monotonic_ns=before,
                   server_call_monotonic_ns=before + 1_000_000,
                   request_after_monotonic_ns=before + 2_000_000)
    return rows, metadata, fixtures


class SessionFixtureBindingTests(unittest.TestCase):
    def checker(self):
        checker = getattr(oracle, 'check_session_fixtures', None)
        self.assertTrue(callable(checker), 'missing explicit two-state session fixture checker')
        return checker

    def test_explicit_two_state_checker_accepts_literal_binding_without_rewriting_rows(self):
        checker = self.checker()
        inputs = session_fixtures()
        original = copy.deepcopy(inputs)
        self.assertIsNone(checker(*inputs))
        self.assertEqual(inputs, original, 'checker must not rewrite observed rows or receipts')

    def test_fixture_keys_images_and_process_receipts_are_independently_bound(self):
        checker = self.checker()
        self.assertIsNone(checker(*session_fixtures()))
        for keys in ({'AB'}, {'AB', 'C', 'extra'}, {'A', 'B', 'C'}):
            rows, metadata, fixtures = session_fixtures()
            fixtures = {key: fixtures.get(key, fixtures['AB']) for key in keys}
            with self.subTest(keys=keys), self.assertRaisesRegex(ValueError, '^session-fixture-keys$'):
                checker(rows, metadata, fixtures)
        cases = [
            (('C', 'candidate_commit'), '0' * 40, 'images'),
            (('C', 'package_sha256'), '0' * 64, 'images'),
            (('C', 'executable_sha256'), '0' * 64, 'images'),
            (('C', 'helper_sha256'), '0' * 64, 'images'),
            (('C', 'process_binding', 'pid'), 700, 'pid'),
            (('C', 'process_binding', 'pid'), 0, 'pid'),
            (('C', 'process_binding', 'pid'), True, 'pid'),
            (('C', 'process_binding', 'pid'), 701.0, 'pid'),
            (('C', 'process_binding', 'start_ticks'), 0, 'process'),
            (('C', 'process_binding', 'cgroup'), '', 'process'),
            (('C', 'process_binding', 'executable_sha256'), '0' * 64, 'process'),
            (('C', 'process_binding', 'helper_sha256'), '0' * 64, 'process'),
            (('C', 'process_binding', 'helper_maps'), [], 'process')]
        for path, value, error in cases:
            rows, metadata, fixtures = session_fixtures()
            target = fixtures
            for part in path[:-1]:
                target = target[part]
            target[path[-1]] = value
            with self.subTest(path=path), self.assertRaisesRegex(ValueError, '^session-fixture-' + error + '$'):
                checker(rows, metadata, fixtures)
        rows, metadata, fixtures = session_fixtures()
        del fixtures['C']['process_binding']
        with self.assertRaisesRegex(ValueError, '^session-fixture-process$'):
            checker(rows, metadata, fixtures)

    def test_fresh_distinct_state_and_serial_zero_population_stop_are_required(self):
        checker = self.checker()
        self.assertIsNone(checker(*session_fixtures()))
        cases = [
            (('C', 'state_before', 'path'), '/owned/browser/state-AB', 'state'),
            (('C', 'state_before', 'inode'), 101, 'state'),
            (('C', 'state_before', 'initial_entries'), ['access.state'], 'fresh'),
            (('C', 'state_before', 'copy_source'), '/owned/browser/state-AB', 'fresh'),
            (('C', 'state_before', 'descriptor_held'), False, 'fresh'),
            (('C', 'state_before', 'created_monotonic_ns'), 11_000_000_000, 'fresh'),
            (('C', 'state_before', 'observed_monotonic_ns'), 10_000_000_000, 'fresh'),
            (('C', 'state_after', 'inode'), 103, 'state'),
            (('C', 'state_after', 'observed_monotonic_ns'), 10_000_000_000, 'state'),
            (('C', 'started_monotonic_ns'), 10_000_000_000, 'order'),
            (('AB', 'stop_observations', 0, 'confirmed'), False, 'stop'),
            (('AB', 'stop_observations', 0, 'confirmed'), 1, 'stop'),
            (('AB', 'stop_observations', 0, 'population'), 1, 'stop'),
            (('AB', 'stop_observations', 0, 'population'), False, 'stop'),
            (('AB', 'stop_observations', 0, 'pgid'), 999, 'stop'),
            (('AB', 'stop_observations', 0, 'start_ticks'), 801, 'stop'),
            (('AB', 'stop_observations', 0, 'observed_monotonic_ns'), 11_000_000_000, 'stop'),
            (('AB', 'stop_observations'), [], 'stop'),
            (('AB', 'owned_groups'), [], 'stop'),
            (('C', 'run_started_monotonic_ns'), 2_000_000_000, 'deadline'),
            (('C', 'deadline_monotonic_ns'), 481_000_000_000, 'deadline'),
            (('C', 'deadline_monotonic_ns'), True, 'deadline'),
            (('C', 'stopped_monotonic_ns'), 241_000_000_001, 'deadline')]
        for path, value, error in cases:
            rows, metadata, fixtures = session_fixtures()
            target = fixtures
            for part in path[:-1]:
                target = target[part]
            target[path[-1]] = value
            if path in (('C', 'state_before', 'path'), ('C', 'state_before', 'inode')):
                fixtures['C']['state_after'][path[-1]] = value
            if path == ('C', 'started_monotonic_ns'):
                fixtures['C']['state_before']['created_monotonic_ns'] = value - 100_000_000
                fixtures['C']['state_before']['observed_monotonic_ns'] = value - 50_000_000
            with self.subTest(path=path), self.assertRaisesRegex(ValueError, '^session-fixture-' + error + '$'):
                checker(rows, metadata, fixtures)
        rows, metadata, fixtures = session_fixtures()
        fixtures['AB']['owned_groups'].append({'pgid': 702, 'start_ticks': 802})
        with self.assertRaisesRegex(ValueError, '^session-fixture-stop$'):
            checker(rows, metadata, fixtures)
        for field in ('owned_groups', 'stop_observations'):
            rows, metadata, fixtures = session_fixtures()
            fixtures['AB'][field].append(copy.deepcopy(fixtures['AB'][field][0]))
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, '^session-fixture-stop$'):
                checker(rows, metadata, fixtures)

    def test_exact_session_rows_keep_original_boundaries_and_actual_request_windows(self):
        checker = self.checker()
        self.assertIsNone(checker(*session_fixtures()))
        cases = [
            (0, 'pid', 701, 'session-fixture-row'),
            (2, 'pid', 700, 'session-fixture-row'),
            (2, 'pid', 999, 'session-fixture-row'),
            (2, 'pid', True, 'session-fixture-row'),
            (2, 'server_call_monotonic_ns', 11_999_999_999, 'session-fixture-window'),
            (2, 'request_before_monotonic_ns', 10_999_999_999, 'session-fixture-window'),
            (31, 'request_after_monotonic_ns', 31_000_000_001, 'session-fixture-window'),
            (3, 'request_before_monotonic_ns', 12_549_999_999, 'session-fixture-pacing'),
            (0, 'request_s', 2800, 'session-boundary'),
            (1, 'status', 429, 'session-refusal'),
            (1, 'error', 'rate limited', 'session-refusal'),
            (31, 'status', 200, 'session-refusal'),
            (31, 'last_seen_s', 38000, 'session-baseline'),
            (31, 'session_hash', 'b' * 64, 'session-binding')]
        for index, field, value, error in cases:
            rows, metadata, fixtures = session_fixtures()
            rows[index][field] = value
            with self.subTest(index=index, field=field), self.assertRaisesRegex(ValueError, '^' + error + '$'):
                checker(rows, metadata, fixtures)
        rows, metadata, fixtures = session_fixtures()
        with self.assertRaisesRegex(ValueError, '^session-completeness$'):
            checker(rows[:5] + rows[6:], metadata, fixtures)
        rows, metadata, fixtures = session_fixtures()
        del rows[2]['server_call_monotonic_ns']
        with self.assertRaisesRegex(ValueError, '^session-fixture-window$'):
            checker(rows, metadata, fixtures)
        rows, metadata, fixtures = session_fixtures()
        fixtures['AB']['stopped_monotonic_ns'] = 4_000_000_000
        fixtures['AB']['stop_observations'][0]['observed_monotonic_ns'] = 4_000_000_000
        fixtures['C']['started_monotonic_ns'] = 4_100_000_000
        fixtures['C']['state_before']['created_monotonic_ns'] = 4_050_000_000
        fixtures['C']['state_before']['observed_monotonic_ns'] = 4_075_000_000
        for index, row in enumerate(rows[2:]):
            before = 4_149_999_999 + index * 600_000_000
            row.update(request_before_monotonic_ns=before, server_call_monotonic_ns=before + 1_000_000,
                       request_after_monotonic_ns=before + 2_000_000)
        with self.assertRaisesRegex(ValueError, '^session-fixture-pacing$'):
            checker(rows, metadata, fixtures)


if __name__=='__main__':unittest.main()
