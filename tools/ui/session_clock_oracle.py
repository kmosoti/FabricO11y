"""Independent literal checks for the disposable controlled-time fixture."""


def require(condition, name):
    if not condition:raise ValueError(name)


def check_frames(frames):
    require(0 < len(frames) <= 64, 'frame-count')
    previous=0
    for sequence, frame in enumerate(frames):
        require(isinstance(frame, bytes) and len(frame)==24, 'frame-length')
        require(frame[:8]==b'FCLKv1\0\0', 'frame-version')
        require(int.from_bytes(frame[8:16], 'little')==sequence, 'frame-sequence')
        offset=int.from_bytes(frame[16:24], 'little')
        require(previous<=offset<=28860 and (sequence!=0 or offset==0), 'frame-offset')
        previous=offset


def check_probe_clocks(rows, target_pid):
    expected=[(seq, offset, clock) for seq, offset in enumerate((0,1,1799,1800,28799,28800)) for clock in (0,1,4)]
    require(len(rows)==len(expected), 'clock-completeness')
    for row, (sequence, offset, clock) in zip(rows, expected):
        require(set(row)=={'pid','sequence','offset_s','clock_id','raw_ns','returned_ns','syscall_before_ns','syscall_after_ns'}, 'clock-fields')
        require(all(type(v) is int and v>=0 for v in row.values()), 'clock-integer')
        require(row['pid']==target_pid and target_pid>0, 'clock-pid')
        require((row['sequence'],row['offset_s'],row['clock_id'])==(sequence,offset,clock), 'clock-binding')
        before,after=row['syscall_before_ns'],row['syscall_after_ns']
        require(0<=after-before<=1_000_000_000, 'clock-bracket-inconclusive')
        require(before<=row['raw_ns']<=after, 'clock-raw-bracket')
        require(row['returned_ns']==row['raw_ns']+(offset*1_000_000_000 if clock==0 else 0), 'clock-realtime-only')


def check_sleep(row):
    require(set(row)=={'start_before_ns','start_after_ns','end_before_ns','end_after_ns','instant_elapsed_ns'}, 'sleep-fields')
    require(all(type(v) is int and v>=0 for v in row.values()), 'sleep-integer')
    a,b,c,d=(row[k] for k in ('start_before_ns','start_after_ns','end_before_ns','end_after_ns'))
    require(a<=b<=c<=d and 500_000_000<=c-b<=d-a<=1_000_000_000, 'sleep-bracket-inconclusive')
    require(c-b<=row['instant_elapsed_ns']<=d-a, 'sleep-monotonic')


def check_isolation(row, target_pid, helper_sha256, executable_sha256):
    require(row['target_pid']==target_pid and row['target_executable_sha256']==executable_sha256, 'isolation-target')
    require(row['target_helper_maps']==[helper_sha256], 'isolation-interception')
    require(row['target_control_cloexec'] is True and row['target_control_type']=='AF_UNIX/SOCK_SEQPACKET', 'isolation-control')
    require(set(row['ordinary_processes'])=={'controller','browser','driver','companion'}, 'isolation-processes')
    pids={target_pid}
    for process in row['ordinary_processes'].values():
        require(process['pid'] not in pids and process['pid']>0, 'isolation-pid');pids.add(process['pid'])
        require(process['helper_maps']==[] and process['control_env_names']==[] and process['control_fd_present'] is False, 'isolation-escape')
        before,after=process['syscall_before_ns'],process['syscall_after_ns']
        require(0<=after-before<=1_000_000_000 and before<=process['clock_ns']<=after, 'isolation-host-clock')
    fork=row['fork']
    require(fork['pid'] not in pids and fork['pid']>0 and fork['target_offset_s']==1, 'isolation-fork-pid')
    require(fork['returned_ns']==fork['raw_ns'] and fork['control_sequence_after']==fork['control_sequence_before'], 'isolation-fork-consumption')
    require(fork['exec_helper_maps']==[] and fork['exec_control_env_names']==[] and fork['exec_control_fd_present'] is False, 'isolation-exec-escape')


def check_sessions(rows, sessions, target_pid):
    require(set(sessions)=={'A','B','C'} and len({s['hash'] for s in sessions.values()})==3, 'session-identities')
    for session in sessions.values():
        require(type(session['hash']) is str and len(session['hash'])==64 and all(c in '0123456789abcdef' for c in session['hash']), 'session-identities')
        require(all(type(session[k]) is int and session[k]>=0 for k in ('issued_s','expires_s','baseline_seen_s')), 'session-baseline')
        require(session['expires_s']==session['issued_s']+28800 and session['issued_s']<=session['baseline_seen_s']<session['expires_s'], 'session-baseline')
    expected=[('idle_before','A',sessions['A']['baseline_seen_s']+1799,200),('idle_at','B',sessions['B']['baseline_seen_s']+1800,401)]
    expected += [('absolute_keepalive','C',sessions['C']['issued_s']+step,200) for step in range(1000,28001,1000)]
    expected += [('absolute_before','C',sessions['C']['issued_s']+28799,200),('absolute_at','C',sessions['C']['issued_s']+28800,401)]
    require(len(rows)==len(expected), 'session-completeness')
    seen={key:session['baseline_seen_s'] for key,session in sessions.items()}
    for row,(cell,key,second,status) in zip(rows,expected):
        session=sessions[key]
        require(row['cell']==cell and row['session_hash']==session['hash'] and row['pid']==target_pid, 'session-binding')
        require(row['issued_s']==session['issued_s'] and row['expires_s']==session['expires_s'] and row['last_seen_s']==seen[key], 'session-baseline')
        require(row['request_s']==second, 'session-boundary')
        require(row['server_call_s']==second, 'session-boundary-inconclusive')
        require(row['status']==status and row['error']==('sign in required' if status==401 else None), 'session-refusal')
        if key=='C':require(0<=second-seen[key]<1800, 'session-idle-confound')
        else:require(second<session['expires_s'], 'session-absolute-confound')
        if status==200:seen[key]=second
