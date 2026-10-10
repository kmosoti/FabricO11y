"""Reconstruct simulator offers from its independent pre-send event/hash ledger."""
import base64
import hashlib

from release_fixture import attribute, fixed, integer, message
from workload import SEEDS, entropy_body


def simulator_batch(seed, index, node_id, sequence, timestamp):
    if seed not in SEEDS or index < 0 or len(node_id) != 16 or sequence < 1 or timestamp <= 0:
        raise ValueError('invalid simulator offer coordinate')
    second = sequence - 1
    resource = message(1, attribute('host.name', f'sim-{index:04d}')) + message(1, attribute('service.name', 'fabric-node-sim'))
    rows = []
    for tick in (second*2, second*2+1):
        body = 'R'*512 if tick % 2 == 0 else entropy_body(seed, index, tick)
        rows.append(message(2, message(5, message(1, body.encode())) + fixed(11, timestamp)))
    logs = message(1, message(1, resource) + message(2, b''.join(rows)))
    metrics = b''
    if second % 15 == 0:
        rows = []
        for point in range(32):
            value = int.from_bytes(hashlib.sha256(f'metric:{seed}:{index}:{second}:{point}'.encode()).digest()[:8], 'big') >> 1
            datum = fixed(3, timestamp) + fixed(6, value, signed=True)
            metric = message(1, f'sim.metric.{point}'.encode()) + message(3, b'1') + message(5, message(1, datum))
            rows.append(message(2, metric))
        metrics = message(1, message(1, resource) + message(2, b''.join(rows)))
    return (integer(1, 1) + message(2, node_id) + integer(3, 1) + integer(4, sequence)
            + (message(5, metrics) if metrics else b'') + message(6, logs))


def reconstruct(seed, identities, seconds, events, ledger):
    times = {}
    for event in events:
        if event['e'] == 'created':
            key = event['id'], event['seq']
            if key in times:
                raise ValueError('duplicate simulator creation event')
            times[key] = event['t']
    expected = {(i, s) for i in range(identities) for s in range(1, seconds+1)}
    if set(times) != expected:
        raise ValueError('incomplete independent simulator offer population')
    sources = {}
    for record in ledger:
        if record['type'] != 'source':
            continue
        key = record['node_id'], record['sequence']
        if record['generation'] != 1 or key in sources:
            raise ValueError('invalid independent simulator source identity')
        sources[key] = base64.b64decode(record['bytes'], validate=True)
    nodes = sorted({node for node, _ in sources})
    if len(nodes) != identities or set(sources) != {(n,s) for n in nodes for s in range(1,seconds+1)}:
        raise ValueError('incomplete independent simulator source hashes')
    # Random node IDs are joined to offered index/time by matching the exact
    # independent pre-send SHA, never by trusting the server's enrolled label.
    mapping = {}
    for node in nodes:
        matches = [i for i in range(identities) if hashlib.sha256(
            simulator_batch(seed, i, bytes.fromhex(node), 1, times[(i,1)])).digest() == sources[(node,1)]]
        if len(matches) != 1 or matches[0] in mapping:
            raise ValueError('source hash cannot uniquely join simulator identity')
        mapping[matches[0]] = node
    originals = []
    for (index, sequence), stamp in sorted(times.items()):
        raw = simulator_batch(seed, index, bytes.fromhex(mapping[index]), sequence, stamp)
        if hashlib.sha256(raw).digest() != sources[(mapping[index], sequence)]:
            raise ValueError('simulator source differs from independently specified offer')
        originals.append({'label': f'sim{index:04d}', 'bytes': base64.b64encode(raw).decode()})
    return originals, mapping
