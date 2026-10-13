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
    _check_session_rows(rows, sessions, {key: target_pid for key in ('A', 'B', 'C')})


def _check_session_rows(rows, sessions, targets):
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
        require(row['cell']==cell and row['session_hash']==session['hash'] and row['pid']==targets[key], 'session-binding')
        require(row['issued_s']==session['issued_s'] and row['expires_s']==session['expires_s'] and row['last_seen_s']==seen[key], 'session-baseline')
        require(row['request_s']==second, 'session-boundary')
        require(row['server_call_s']==second, 'session-boundary-inconclusive')
        require(row['status']==status and row['error']==('sign in required' if status==401 else None), 'session-refusal')
        if key=='C':require(0<=second-seen[key]<1800, 'session-idle-confound')
        else:require(second<session['expires_s'], 'session-absolute-confound')
        if status==200:seen[key]=second


def _integer(value, minimum=0):
    return type(value) is int and value >= minimum


def _hex_identity(value, length=64):
    return type(value) is str and len(value) == length and all(c in '0123456789abcdef' for c in value)


def _absolute_path(value):
    return (type(value) is str and value.startswith('/') and '\0' not in value
            and all(part not in ('', '.', '..') for part in value.split('/')[1:]))


def check_session_fixtures(rows, sessions, fixtures):
    """Check receipt consistency; native identity and freshness need actual admission."""
    require(type(fixtures) is dict and set(fixtures) == {'AB', 'C'}, 'session-fixture-keys')
    require(all(type(fixture) is dict for fixture in fixtures.values()), 'session-fixture-keys')
    images = ('candidate_commit', 'package_sha256', 'executable_sha256', 'helper_sha256')
    for field in images:
        values = [fixtures[key].get(field) for key in ('AB', 'C')]
        require(all(_hex_identity(value, 40 if field == 'candidate_commit' else 64) for value in values)
                and values[0] == values[1], 'session-fixture-images')

    targets, intervals, states, stops = {}, {}, {}, {}
    common_deadline = None
    for key in ('AB', 'C'):
        fixture = fixtures[key]
        binding = fixture.get('process_binding')
        require(type(binding) is dict, 'session-fixture-process')
        pid = binding.get('pid')
        require(_integer(pid, 1) and pid not in targets.values(), 'session-fixture-pid')
        targets[key] = pid
        mappings = binding.get('helper_maps')
        require(_integer(binding.get('start_ticks'), 1)
                and type(binding.get('cgroup')) is str and bool(binding['cgroup'].strip())
                and binding.get('executable_sha256') == fixture['executable_sha256']
                and binding.get('helper_sha256') == fixture['helper_sha256']
                and _absolute_path(binding.get('libc')) and _hex_identity(binding.get('libc_sha256'))
                and type(mappings) is list and bool(mappings), 'session-fixture-process')
        for mapping in mappings:
            require(isinstance(mapping, (list, tuple)) and len(mapping) == 3
                    and type(mapping[0]) is str and len(mapping[0].split(':')) == 2
                    and all(part and all(c in '0123456789abcdef' for c in part) for part in mapping[0].split(':'))
                    and type(mapping[1]) is str and bool(mapping[1])
                    and all(c in '0123456789' for c in mapping[1]) and any(c != '0' for c in mapping[1])
                    and _absolute_path(mapping[2]), 'session-fixture-process')

        run, deadline, start, stop = (fixture.get(field) for field in
                ('run_started_monotonic_ns', 'deadline_monotonic_ns', 'started_monotonic_ns', 'stopped_monotonic_ns'))
        require(all(_integer(value) for value in (run, deadline, start, stop))
                and deadline == run + 240_000_000_000 and run <= start < stop <= deadline,
                'session-fixture-deadline')
        require(common_deadline is None or common_deadline == (run, deadline), 'session-fixture-deadline')
        common_deadline = (run, deadline)
        intervals[key] = (start, stop)

    require(isinstance(rows, (list, tuple)) and len(rows) == 32, 'session-completeness')
    require(type(sessions) is dict and set(sessions) == {'A', 'B', 'C'}, 'session-identities')
    session_fields = {'hash', 'issued_s', 'expires_s', 'baseline_seen_s'}
    require(all(type(session) is dict and session_fields <= session.keys()
                and _hex_identity(session['hash']) for session in sessions.values()), 'session-identities')
    row_fields = {'cell', 'session_hash', 'pid', 'issued_s', 'expires_s', 'last_seen_s',
                  'request_s', 'server_call_s', 'status', 'error'}
    last_dispatch, last_request = None, {}
    for index, row in enumerate(rows):
        key = 'AB' if index < 2 else 'C'
        require(type(row) is dict and _integer(row.get('pid'), 1)
                and row['pid'] == targets[key], 'session-fixture-row')
        require(row_fields <= row.keys(), 'session-binding')
        before, call, after = (row.get(field) for field in
                ('request_before_monotonic_ns', 'server_call_monotonic_ns', 'request_after_monotonic_ns'))
        start, stop = intervals[key]
        require(all(_integer(value) for value in (before, call, after))
                and start <= before <= call <= after <= stop, 'session-fixture-window')
        require(last_dispatch is None or before - last_dispatch >= 550_000_000, 'session-fixture-pacing')
        last_dispatch = before
        last_request[key] = max(last_request.get(key, 0), after)

    for key in ('AB', 'C'):
        fixture = fixtures[key]
        binding = fixture['process_binding']
        pid = targets[key]
        run = common_deadline[0]
        start, stop = intervals[key]
        before, after = fixture.get('state_before'), fixture.get('state_after')
        require(type(before) is dict and type(after) is dict, 'session-fixture-state')
        for state in (before, after):
            require(_absolute_path(state.get('path')) and _integer(state.get('device'))
                    and _integer(state.get('inode'), 1), 'session-fixture-state')
        identity = (before['path'], before['device'], before['inode'])
        require(identity == (after['path'], after['device'], after['inode'])
                and _integer(after.get('observed_monotonic_ns'))
                and start <= after['observed_monotonic_ns'] <= stop, 'session-fixture-state')
        if states:
            previous = states['AB']
            require(identity[0] != previous[0] and identity[1:] != previous[1:], 'session-fixture-state')
        states[key] = identity
        created, observed = before.get('created_monotonic_ns'), before.get('observed_monotonic_ns')
        require(_integer(created) and _integer(observed) and run <= created < observed < start
                and type(before.get('initial_entries')) is list and before['initial_entries'] == []
                and 'copy_source' in before and before['copy_source'] is None
                and before.get('descriptor_held') is True, 'session-fixture-fresh')

        groups, observations = fixture.get('owned_groups'), fixture.get('stop_observations')
        require(type(groups) is list and bool(groups) and type(observations) is list
                and len(groups) == len(observations), 'session-fixture-stop')
        identities = set()
        for group in groups:
            require(type(group) is dict and _integer(group.get('pgid'), 1)
                    and _integer(group.get('start_ticks'), 1), 'session-fixture-stop')
            group_identity = (group['pgid'], group['start_ticks'])
            require(group_identity not in identities, 'session-fixture-stop')
            identities.add(group_identity)
        require((pid, binding['start_ticks']) in identities, 'session-fixture-stop')
        matched, observed_stops = set(), []
        for observation in observations:
            require(type(observation) is dict and _integer(observation.get('pgid'), 1)
                    and _integer(observation.get('start_ticks'), 1), 'session-fixture-stop')
            group_identity = (observation['pgid'], observation['start_ticks'])
            require(group_identity in identities and group_identity not in matched
                    and type(observation.get('population')) is int and observation['population'] == 0
                    and observation.get('confirmed') is True
                    and _integer(observation.get('observed_monotonic_ns'))
                    and start <= observation['observed_monotonic_ns'] <= stop, 'session-fixture-stop')
            matched.add(group_identity)
            observed_stops.append(observation['observed_monotonic_ns'])
        stops[key] = observed_stops

    require(intervals['AB'][1] < intervals['C'][0], 'session-fixture-order')
    for key in ('AB', 'C'):
        require(all(observed >= last_request[key] for observed in stops[key]), 'session-fixture-stop')
    _check_session_rows(rows, sessions, {'A': targets['AB'], 'B': targets['AB'], 'C': targets['C']})
