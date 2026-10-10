"""Independent, bounded R3 protobuf fixtures; no application codec imports.

Field numbers follow the pinned OTLP schema and Batch version-one contract.
The query/delivery oracles remain separate consumers of these bytes.
"""
import hashlib
import struct

from workload import SEEDS, entropy_body

NS = 1_000_000_000


def varint(n):
    if not 0 <= n < 2**64:
        raise ValueError('unsigned protobuf integer out of range')
    answer = bytearray()
    while n > 127:
        answer.append((n & 127) | 128)
        n >>= 7
    answer.append(n)
    return bytes(answer)


def message(field, value):
    return varint(field * 8 + 2) + varint(len(value)) + value


def integer(field, value):
    return varint(field * 8) + varint(value)


def fixed(field, value, signed=False):
    return varint(field * 8 + 1) + struct.pack('<q' if signed else '<Q', value)


def attribute(key, value):
    return message(1, key.encode()) + message(2, message(1, str(value).encode()))


def identity(seed, node):
    if seed not in SEEDS or not 0 <= node < 100:
        raise ValueError('unregistered query fixture coordinate')
    return hashlib.sha256(f'release-query:{seed}:{node}'.encode()).digest()[:16]


def traces(seed, tick, observed_ns):
    trace_id = hashlib.sha256(f'release-trace:{seed}:{tick}'.encode()).digest()[:16]
    ids = [hashlib.sha256(trace_id + bytes([i])).digest()[:8] for i in range(3)]
    encoded, rows = [], []
    for i, span_id in enumerate(ids):
        parent = b'' if i == 0 else ids[0]
        name = f'release-span-{i}'
        start, end = observed_ns + i, observed_ns + i + 100
        value = (message(1, trace_id) + message(2, span_id) + message(5, name.encode())
                 + integer(6, 1) + fixed(7, start) + fixed(8, end))
        if parent:
            value += message(4, parent)
        encoded.append(message(2, value))
        rows.append(dict(trace_id=trace_id.hex(), span_id=span_id.hex(),
                         parent_span_id=parent.hex(), name=name, start_ns=start,
                         end_ns=end, kind=1, status=0, attributes={}))
    return message(1, message(2, b''.join(encoded))), rows


def query_batch(seed, node, sequence, start_ns):
    """One of 10,000 Batches: 50 logs/50 counters, optional three-span trace."""
    node_id = identity(seed, node)
    if not 1 <= sequence <= 100 or start_ns <= 0:
        raise ValueError('unregistered batch coordinate')
    logs, points = [], []
    for j in range(50):
        coordinate = (sequence - 1) * 50 + j
        timestamp = start_ns + coordinate * (NS // 10)
        body = 'R' * 512 if coordinate % 2 == 0 else entropy_body(seed, node, coordinate)
        logs.append(message(2, fixed(11, timestamp) + message(5, message(1, body.encode()))))
        points.append(message(1, message(7, attribute('series', coordinate % 5))
                              + fixed(2, start_ns) + fixed(3, timestamp)
                              + fixed(6, coordinate // 5 + 1, signed=True)))
    log_payload = message(1, message(2, b''.join(logs)))
    counter = (message(1, b'release.counter') + message(3, b'1')
               + message(7, b''.join(points) + integer(2, 2) + integer(3, 1)))
    metric_payload = message(1, message(2, message(2, counter)))
    batch = (integer(1, 1) + message(2, node_id) + integer(3, 1)
             + integer(4, sequence) + message(5, metric_payload) + message(6, log_payload))
    # Ten traces per identity, 1,000 total; separately counted from the million.
    if sequence <= 10:
        trace_payload, _ = traces(seed, node * 10 + sequence - 1,
                                 start_ns + (sequence - 1) * NS)
        batch += message(9, trace_payload)
    return batch
