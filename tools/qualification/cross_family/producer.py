#!/usr/bin/env python3
"""Registered independent guest offers and durable controlled-metric custody.

Protobuf fields: published OTLP metrics/trace v1 plus Fabric Batch v1 contract.
No Rust codec, query implementation or verifier is imported. Commands run in
an admitted disposable guest; this producer never starts or installs services.
"""
from __future__ import annotations
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import queue
import ssl
import struct
import threading
import time
import urllib.error
import urllib.request

SEED = 0xA11FA001
SECONDS = 195  # 15 warmup +120 measured +60 interruption; then drain.


def varint(n):
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


def entropy_body(tick):
    # Unchanged workload.py entropy coordinate: node0 and registered seed.
    source = f'fabric-alpha-v1:{SEED}:0:{tick}'.encode()
    raw = b''.join(hashlib.sha256(source + b':' + i.to_bytes(2, 'big')).digest() for i in range(13))
    return base64.b85encode(raw)[:512].decode('ascii')


def metrics(k, start_ns, observed_ns):
    points = []
    for point in range(32):
        attribute = message(1, b'point_id') + message(2, message(1, str(point).encode()))
        points.append(message(7, attribute) + fixed(2, start_ns) + fixed(3, observed_ns)
                      + fixed(6, k * (point + 1), signed=True))
    total = b''.join(message(1, point) for point in points) + integer(2, 2) + integer(3, 1)
    metric = message(1, b'cross.counter') + message(7, total)
    return message(1, message(2, message(2, metric)))


def trace(tick, observed_ns):
    trace_id = hashlib.sha256(f'cross-trace:{SEED}:{tick}'.encode()).digest()[:16]
    spans = []
    rows = []
    identifiers = [hashlib.sha256(trace_id + bytes([index])).digest()[:8] for index in range(3)]
    for index, identifier in enumerate(identifiers):
        parent = b'' if index == 0 else identifiers[0]
        name = f'cross-span-{index}'
        start = observed_ns + index
        end = start + 100
        span = (message(1, trace_id) + message(2, identifier) + message(5, name.encode())
                + integer(6, 1) + fixed(7, start) + fixed(8, end))
        if parent:
            span += message(4, parent)
        spans.append(message(2, span))
        rows.append({'trace_id': trace_id.hex(), 'span_id': identifier.hex(),
                     'parent_span_id': parent.hex(), 'name': name, 'start_ns': start, 'end_ns': end})
    return message(1, message(2, b''.join(spans))), rows


def batch(node, sequence, payload):
    return integer(1, 1) + message(2, bytes.fromhex(node)) + integer(3, 1) + integer(4, sequence) + message(5, payload)


class Journal:
    def __init__(self, path):
        self.stream = path.open('x')
        self.lock = threading.Lock()

    def append(self, record):
        with self.lock:
            self.stream.write(json.dumps(record, separators=(',', ':')) + '\n')
            self.stream.flush()
            os.fsync(self.stream.fileno())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--config', required=True, type=Path)
    args = parser.parse_args()
    config = json.loads(args.config.read_text())
    root = Path(config['root'])
    journal = Journal(root / 'offers.jsonl')
    controlled = Journal(root / 'controlled-custody.jsonl')
    pending = queue.Queue(maxsize=32)
    stop_at = time.monotonic() + SECONDS + 120
    context = ssl.create_default_context(cafile=config['ca'])
    token = Path(config['token_file']).read_text().strip()
    failures = []
    acknowledged = 0
    offered_sequences = []
    began_mono = time.monotonic_ns()
    began_wall = time.time_ns()
    journal.append({'type': 'begin', 'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
                    'monotonic_ns': began_mono, 'unix_ns': began_wall, 'seconds': SECONDS})

    def deliver():
        nonlocal acknowledged
        while True:
            item = pending.get()
            if item is None:
                return
            sequence, payload = item
            identity = {'node_id': config['node_id'], 'generation': 1, 'sequence': sequence}
            while time.monotonic() < stop_at:
                before = time.monotonic_ns()
                controlled.append({'type': 'attempt', **identity,
                                   'bytes': base64.b64encode(payload).decode(), 'injected_conflict': False})
                try:
                    request = urllib.request.Request(config['server_url'] + '/v1/batches', data=payload,
                            headers={'authorization': 'Bearer ' + token, 'content-type': 'application/x-protobuf'})
                    with urllib.request.urlopen(request, context=context, timeout=2) as response:
                        answer = json.loads(response.read())
                    after = time.monotonic_ns()
                    if answer.get('status') != 'ack' or answer.get('committed_through') != sequence:
                        raise ValueError('controlled producer received an invalid ACK')
                    controlled.append({'type': 'response', **identity, 'kind': 'ack', 'committed_through': sequence})
                    acknowledged = sequence
                    journal.append({'type': 'metric_ack', **identity, 'before_ns': before, 'after_ns': after})
                    break
                except (OSError, ValueError, urllib.error.URLError) as error:
                    controlled.append({'type': 'response', **identity, 'kind': 'no_response'})
                    journal.append({'type': 'metric_retry', **identity, 'before_ns': before,
                                    'after_ns': time.monotonic_ns(), 'error': str(error)[:500]})
                    if isinstance(error, ValueError) or (isinstance(error, urllib.error.HTTPError) and error.code not in (429, 503)):
                        failures.append(str(error))
                        return
                    time.sleep(0.2)
            else:
                failures.append('controlled metric custody did not drain by deadline')
                return

    def guarded_deliver():
        try:
            deliver()
        except BaseException as error:
            failures.append(f'controlled custody worker failed: {error}')

    worker = threading.Thread(target=guarded_deliver)
    worker.start()
    offset = 0
    try:
        with Path(config['log']).open('ab', buffering=0) as source:
            for tick in range(SECONDS * 10):
                due = began_mono + tick * 100_000_000
                time.sleep(max(0, (due - time.monotonic_ns()) / 1e9))
                observed = time.time_ns()
                if tick % 5 == 0:
                    log_tick = tick // 5
                    body = 'R' * 512 if log_tick % 2 == 0 else entropy_body(log_tick)
                    before = time.monotonic_ns()
                    source.write((body + '\n').encode())
                    after = time.monotonic_ns()
                    journal.append({'type': 'log', 'tick': log_tick, 'body': body,
                                    'offset_start': offset, 'offset_end': offset + 513,
                                    'before_ns': before, 'after_ns': after, 'observed_ns': observed,
                                    'scheduled_monotonic_ns': due})
                    offset += 513
                if tick % 150 == 0:
                    sequence = tick // 150 + 1
                    payload = batch(config['node_id'], sequence, metrics(sequence, began_wall, observed))
                    controlled.append({'type': 'source', 'node_id': config['node_id'], 'generation': 1,
                                       'sequence': sequence, 'bytes': base64.b64encode(payload).decode()})
                    offered_sequences.append(sequence)
                    journal.append({'type': 'metric_committed', 'sequence': sequence,
                                    'monotonic_ns': time.monotonic_ns(), 'observed_ns': observed,
                                    'points': 32, 'k': sequence, 'scheduled_monotonic_ns': due})
                    pending.put_nowait((sequence, payload))
                payload, rows = trace(tick, observed)
                before = time.monotonic_ns()
                journal.append({'type': 'trace_offer', 'tick': tick, 'rows': rows, 'before_ns': before,
                                'scheduled_monotonic_ns': due})
                try:
                    request = urllib.request.Request('http://127.0.0.1:4318/v1/traces', data=payload,
                                                     headers={'content-type': 'application/x-protobuf'})
                    with urllib.request.urlopen(request, timeout=0.08) as response:
                        response_body = response.read()
                        status = response.status
                    journal.append({'type': 'trace_admitted', 'tick': tick, 'status': status,
                                    'before_ns': before, 'after_ns': time.monotonic_ns(),
                                    'response_base64': base64.b64encode(response_body).decode()})
                except (OSError, urllib.error.URLError) as error:
                    journal.append({'type': 'trace_uncertain', 'tick': tick, 'before_ns': before,
                                    'after_ns': time.monotonic_ns(), 'error': str(error)[:500]})
    finally:
        pending.put(None)
        worker.join(timeout=max(0.1, stop_at - time.monotonic()))
    if worker.is_alive():
        raise SystemExit('controlled producer deadline exceeded')
    controlled.append({'type': 'node_state', 'node_id': config['node_id'], 'generation': 1,
                       'retained_sequences': offered_sequences, 'ack_cursor': acknowledged})
    journal.append({'type': 'end', 'monotonic_ns': time.monotonic_ns(), 'failures': failures})
    return 1 if failures else 0


if __name__ == '__main__':
    raise SystemExit(main())
