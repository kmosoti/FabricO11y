"""Independent R3 phase and same-job joins over raw caller-read records."""
from soak_exposure import CEILINGS, NS


def require(value, reason):
    if not value:
        raise ValueError(reason)


def exact(left, right):
    if type(left) is not type(right):
        return False
    if type(left) is dict:
        return left.keys() == right.keys() and all(exact(left[key], right[key]) for key in left)
    if type(left) is list:
        return len(left) == len(right) and all(exact(a, b) for a, b in zip(left, right))
    return left == right


def validate_job(runtime, outer, expected):
    identity = runtime['outer_identity']
    actual_group = outer['command_receipt']['cgroup']
    if 'workload_cgroup' in runtime:
        require(actual_group == runtime['workload_cgroup'], 'different workload command receipt')
        actual_group = outer['final_kernel_observation']['outer_cgroup']
    require(identity['unit'] == outer['unit']
            and identity['cgroup'] == actual_group
            and type(identity['activation_monotonic_ns']) is int
            and identity['activation_monotonic_ns'] == outer['activation_monotonic_ns'],
            'S1 runtime/stop receipts are different jobs')
    require(all(exact(runtime[key], expected[key]) for key in
                ('output_identity', 'profile', 'input_sha256', 'headroom_binding')),
            'S1 output/helper/profile binding changed')
    output = expected['output_identity']
    require(type(output['path']) is str and output['path'].startswith('/')
            and all(type(output[key]) is int and output[key] > 0 for key in ('device', 'inode')),
            'S1 output identity missing')
    return True


def validate_phases(runtime, anchor):
    require(type(anchor) is int and anchor >= 0, 'independent launcher anchor missing')
    phases = runtime['phases']
    require(type(phases) is list and [item['phase'] for item in phases] == list(CEILINGS),
            'missing or reordered runtime phases')
    previous = anchor
    for item in phases:
        began, ended, elapsed = item['start_ns'], item['end_ns'], item['elapsed_ns']
        require(all(type(value) is int for value in (began, ended, elapsed))
                and anchor <= previous <= began < ended
                and elapsed == ended - began and elapsed < CEILINGS[item['phase']] * NS
                and ended < anchor + 7500 * NS, 'forged, overlapping or exhausted runtime phase')
        previous = ended
    require(phases[0]['start_ns'] == anchor, 'setup did not begin at launcher activation')
    interval = runtime['native_interval']
    require(type(interval['start_ns']) is int and type(interval['conservative_end_ns']) is int
            and phases[1]['start_ns'] <= interval['start_ns']
            and interval['conservative_end_ns'] <= phases[1]['end_ns']
            and interval['conservative_end_ns'] - interval['start_ns'] >= runtime['offered_seconds'] * NS,
            'phase does not enclose actual native exposure')
    return True


def validate_final_parent(runtime, outer):
    from resource_guardian import validate_observation
    observation = outer['final_kernel_observation']
    validate_observation(observation)
    require(observation.get('control_only') is False and outer['stop_confirmed'] is True
            and all(type(status) is int for status in
                    (outer['exit'], outer['wrapper_exit'], outer['command_receipt']['exit']))
            and outer['exit'] == outer['wrapper_exit'] == outer['command_receipt']['exit'] == 0
            and exact(observation['command_receipt'], outer['command_receipt'])
            and observation['command_receipt']['command'] == outer['command']
            and exact(observation['wrapper_identity'], runtime['wrapper_identity'])
            and observation['unit'] == outer['unit'] == runtime['outer_identity']['unit']
            and observation['outer_cgroup'] == runtime['outer_identity']['cgroup']
            and observation['workload_cgroup'] == runtime['workload_cgroup']
            and observation['activation_monotonic_ns'] == outer['activation_monotonic_ns']
            and observation['activation_monotonic_ns'] == runtime['outer_identity']['activation_monotonic_ns']
            and type(observation['before_ns']) is int and type(observation['after_ns']) is int
            and runtime['phases'][-1]['end_ns'] <= observation['before_ns'] <= observation['after_ns']
            and observation['after_ns'] < outer['activation_monotonic_ns'] + 7500 * NS,
            'final actual parent observation binding missing')
    require(exact({key: observation['outer'][key] for key in ('device', 'inode')}, runtime['parent_identity'])
            and exact({key: observation['workload'][key] for key in ('device', 'inode')}, runtime['workload_identity'])
            and exact({key: observation['workload_admission'][key] for key in ('device', 'inode')}, runtime['workload_identity']),
            'final outer/workload directory differs from admission')
    final_runtime_groups = runtime['parent_observations'][-1]['groups']
    require(set(observation['outer']['groups']) == set(final_runtime_groups), 'final parent subgroup observation missing')
    for name, group in observation['outer']['groups'].items():
        require(all(group[key] == final_runtime_groups[name][key] for key in
                    ('memory.high', 'memory.max', 'memory.swap.max', 'pids.max', 'cpu.max')),
                'final parent/subgroup cap changed')
    return True
