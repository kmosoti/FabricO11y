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
    require(identity['unit'] == outer['unit']
            and identity['cgroup'] == outer['command_receipt']['cgroup']
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
