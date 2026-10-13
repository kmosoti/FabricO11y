"""Pure qualification joins; callers authenticate files, observers and receipts."""
import re

BASE = '8fb6f413179b094288316e25845c137440ba23fa'
HELPER = 'examples/spindle_sim.rs'
NS = 1_000_000_000
U64 = 2**64 - 1


def require(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value, lower=0, upper=U64):
    return type(value) is int and lower <= value <= upper


def digest(value, width=64):
    return type(value) is str and re.fullmatch('[0-9a-f]{' + str(width) + '}', value) is not None


def same(actual, expected):
    if type(actual) is not type(expected):
        return False
    if type(actual) is dict:
        return actual.keys() == expected.keys() and all(same(actual[k], expected[k]) for k in actual)
    if type(actual) is list:
        return len(actual) == len(expected) and all(same(a, b) for a, b in zip(actual, expected))
    return actual == expected


def validate_source_build(source, build, canonical, expected):
    try:
        message = 'canonical helper authority'
        require(same(canonical, expected['canonical']) and canonical['candidate'] == BASE, message)
        require(digest(canonical['source_commit'], 40) and all(digest(canonical[k]) for k in
                ('source_manifest_sha256', 'source_bundle_sha256', 'helper_manifest_sha256')), message)
        helpers = canonical['helpers']
        require(type(helpers) is dict and set(helpers) == {'spool_dump', 'server_dump', 'spindle_sim'}
                and all(digest(value) for value in helpers.values()), message)
        message = 'compilation inputs'
        actual, original = source['compilation_inputs'], expected['canonical_inputs']
        require(type(actual) is dict and type(original) is dict and HELPER in original
                and actual.keys() == original.keys(), message)
        require(all(type(path) is str and path and not path.startswith('/')
                and '\x00' not in path and all(part not in ('', '.', '..') for part in path.split('/'))
                and digest(actual[path]) and digest(original[path]) for path in original), message)
        require(actual[HELPER] != original[HELPER] and all(actual[path] == original[path]
                for path in original if path != HELPER), message)
        message = 'separate simulator source/build'
        require(same(source, expected['source']) and source['base_commit'] == BASE
                and all(digest(source[k], 40) for k in ('commit', 'tree'))
                and all(digest(source[k]) for k in ('manifest_sha256', 'bundle_sha256')), message)
        require(same(build, expected['build']) and digest(build['sha256']), message)
        value = build['value']
        require(same(value['source'], source) and value['state'] == 'built'
                and integer(value['exit'], 0, 0) and value['stop_confirmed'] is True
                and digest(value['binary_sha256']), message)
        config, command = value['configuration'], value['command']
        require(type(command) is list and command
                and all(type(item) is str and item for item in command), message)
        require(all(type(config[k]) is str and config[k] for k in ('rustc', 'target', 'profile'))
                and all(type(config[k]) is list and all(type(item) is str for item in config[k])
                        for k in ('features', 'rustflags')), message)
        return True
    except (KeyError, TypeError, AttributeError) as error:
        raise ValueError(message) from error


def validate_native(begin, summary, offers, loaded, expected):
    message = 'begin witness'
    try:
        publication = loaded['publication']
        require(type(begin) is dict and digest(begin.get('sha256'))
                and begin['sha256'] == expected['begin_sha256']
                and publication.get('unchanged_sha256') == begin['sha256']
                and all(publication.get(k) is True for k in
                        ('atomic', 'exclusive', 'before_first_offer', 'regular'))
                and integer(publication.get('nlink'), 1, 1)
                and integer(publication.get('mode'), 0o400, 0o400), message)
        b, identity, parameters = begin['value'], expected['identity'], expected['parameters']
        message = 'clock brackets'
        lo, hi, wall, width = (b['mono_before_ns'], b['mono_after_ns'], b['began_unix_ns'],
                               expected['max_clock_bracket_ns'])
        require(integer(lo) and integer(hi) and integer(wall) and integer(width, 0, 1_000_000)
                and lo <= hi <= lo + width, message)
        message = 'summary/identity binding'
        require(type(parameters) is dict and set(parameters) == {'seconds', 'identities', 'workers', 'seed'}
                and integer(parameters['seconds'], 1, 7200)
                and integer(parameters['identities'], 1, 100)
                and integer(parameters['workers'], 1, 100) and integer(parameters['seed']), message)
        process = identity['process']
        require(type(process) is dict and set(process) ==
                {'pid', 'start_ticks', 'boot_id', 'exe_device', 'exe_inode'}
                and integer(process['pid'], 1, 2**31 - 1) and integer(process['start_ticks'])
                and integer(process['exe_device']) and integer(process['exe_inode'], 1)
                and type(process['boot_id']) is str and process['boot_id'], message)
        require(digest(identity['binary_sha256']) and digest(identity['source_commit'], 40)
                and digest(identity['build_receipt_sha256'])
                and all(same(loaded.get(k), value) for k, value in identity.items())
                and same(b['process'], process) and b['binary_sha256'] == identity['binary_sha256']
                and same(b['parameters'], parameters) and same(summary['process'], process)
                and all(same(summary.get(k), value) for k, value in parameters.items())
                and same(summary['began_unix_ns'], wall)
                and summary['begin_witness_sha256'] == begin['sha256'], message)
        message = 'native lifetime'
        seconds, identities = parameters['seconds'], parameters['identities']
        duration = seconds * NS
        require(integer(summary['elapsed_ns'], duration)
                and integer(summary['ended_monotonic_ns'], hi + duration)
                and integer(loaded['exit'], 0, 0) and loaded['stop_confirmed'] is True
                and integer(summary['undelivered'], 0, 0), message)
        message = 'worker offers'
        seen, count = bytearray(identities * seconds), 0
        for item in offers:
            require(type(item) is dict, message)
            if item.get('e') != 'created':
                continue
            index, sequence = item['id'], item['seq']
            require(integer(index, 0, identities - 1) and integer(sequence, 1, seconds)
                    and integer(item['t'], wall) and integer(item['generated_ns'], item['t']), message)
            slot = index * seconds + sequence - 1
            require(not seen[slot], message)
            seen[slot], count = 1, count + 1
        require(count == len(seen), message)
        return True
    except (KeyError, TypeError, AttributeError) as error:
        raise ValueError(message) from error
