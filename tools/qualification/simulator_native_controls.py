"""Registered finite native diagnostics; explicit CLI only, never auto-run.

The loopback TLS sink ACKs volatile test bytes. It is not a Fabric server,
durability oracle, full-rate workload, S1 trial or qualification result.
"""
import argparse
import base64
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import signal
import ssl
import stat
import subprocess
import sys
import threading
import time

from query_oracle import decode_batch, decode_logs_request, decode_metrics_request, parse_fields

DATA = Path('/run/media/kmosoti/data/FabricO11y')
SEALED = 'be82dabd621e59383aa296754ba8ee18f5ab28fc6dfa8b2fb69e59d847d351cc'
SEALED_SOURCE = '28bcdf7ac833ab634d3b130b85cb2a0aa47eb7a8'
SEALED_BUILD = 'f2a06e16fd8f82e27517eee12a1d8176d052765ac2e75959a85f70cdaeb6fd85'
CANONICAL = dict(binary_sha256=SEALED, source_commit=SEALED_SOURCE,
    source_tree='a82fc425751829bb7ec8acd0db0268b3e67225e1',
    source_manifest_sha256='c09f94d52368f6b84ed390086f8a8e2a1f638aaa4ff575c87ab12f385c8f3957',
    source_bundle_sha256='c9200fafd3e7fb08a327c23beecc63f4d48d2b6b6c242dd52fc52b751d708ba3',
    build_receipt_sha256=SEALED_BUILD,
    canonical_helper_manifest_sha256='e23a9b50599e2330b89335d1abbb8fe8789f2757936fdc1f0450aba699903c4d')
CASES = ('canonical-red', 'positive', 'legacy', 'duplicate', 'existing', 'workers',
         'pending', 'early', 'changed')


def require(ok, message):
    if not ok:
        raise ValueError(message)


def file_hash(path):
    with open(path, 'rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def read_json(path, limit=131072):
    with open(path, 'rb') as stream:
        raw = stream.read(limit + 1)
    require(len(raw) <= limit, 'bounded JSON exceeded')
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, 'duplicate JSON key')
            result[key] = value
        return result
    return json.loads(raw, object_pairs_hook=unique)


def binding(authority, case):
    keys = ('binary_sha256', 'source_commit', 'source_tree', 'source_manifest_sha256',
            'source_bundle_sha256', 'build_receipt_sha256', 'canonical_helper_manifest_sha256')
    require(set(authority) == set(keys), 'expected authority fields')
    for key in keys:
        require(type(authority[key]) is str and re.fullmatch(
                '[0-9a-f]{' + str(40 if key in ('source_commit', 'source_tree') else 64) + '}',
                authority[key]) is not None, 'expected authority digest')
    if case == 'canonical-red':
        require(authority == CANONICAL, 'sealed canonical binding')
    else:
        require(authority['binary_sha256'] != SEALED and authority['canonical_helper_manifest_sha256']
                == CANONICAL['canonical_helper_manifest_sha256'], 'separate helper required')
    return authority


def batch_identity(raw):
    require(0 < len(raw) <= 131072, 'bounded Batch')
    fields = parse_fields(raw)
    require(set(fields) <= {1, 2, 3, 4, 5, 6} and all(len(values) == 1 for values in fields.values()),
            'short fixture singular fields')
    batch = decode_batch(raw)
    require(batch['version'] == 1 and batch['generation'] == 1
            and len(batch['node_id']) == 16 and batch['sequence'] in (1, 2), 'Batch identity')
    logs = decode_logs_request(batch['logs_bytes'])
    metrics = decode_metrics_request(batch['metrics_bytes'])
    require(len(logs) == 2 and all(len(log['body'].encode('ascii')) == 512 for log in logs),
            'two original 512-byte logs')
    require(len(metrics) == (32 if batch['sequence'] == 1 else 0)
            and all(metric['kind'] == 'gauge' and len(metric['points']) == 1 for metric in metrics),
            'original short metric shape')
    return batch['node_id'].hex(), 1, batch['sequence']


def process_identity(pid, expected_stat, expected_hash):
    proc = Path('/proc') / str(pid)
    first = (proc / 'stat').read_text()
    with open(proc / 'exe', 'rb') as stream:
        loaded_stat = os.fstat(stream.fileno())
        loaded_hash = hashlib.file_digest(stream, 'sha256').hexdigest()
    last = (proc / 'stat').read_text()
    # comm can contain spaces and ')'; starttime is field 22, after its last ')'.
    ticks = lambda text: int(text[text.rfind(')') + 2:].split()[19])
    require(ticks(first) == ticks(last) and loaded_hash == expected_hash
            and (loaded_stat.st_dev, loaded_stat.st_ino) == (expected_stat.st_dev, expected_stat.st_ino),
            'actual loaded ELF binding')
    return dict(pid=pid, start_ticks=ticks(last), boot_id=Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
                exe_device=loaded_stat.st_dev, exe_inode=loaded_stat.st_ino)


class Sink(ThreadingHTTPServer):
    daemon_threads = False
    def __init__(self, root, identities, pending):
        self.root, self.tokens, self.pending = root, {f'diagnostic-token-{i}' for i in range(identities)}, pending
        self.lock, self.raw, self.streams, self.errors, self.requests = threading.Lock(), {}, {}, [], 0
        super().__init__(('127.0.0.1', 0), Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.minimum_version = ssl.TLSVersion.TLSv1_2
        context.load_cert_chain(root / 'server.pem', root / 'server.key')
        self.socket = context.wrap_socket(self.socket, server_side=True)


class Handler(BaseHTTPRequestHandler):
    def setup(self):
        super().setup()
        self.connection.settimeout(3)

    def log_message(self, *args):
        pass

    def respond(self, code, value=None):
        raw = b'' if value is None else json.dumps(value).encode()
        self.send_response(code)
        self.send_header('Content-Length', str(len(raw)))
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        if self.path == '/v1/config' and self.headers.get('Authorization') in {'Bearer ' + token for token in self.server.tokens}:
            self.respond(304)
        else:
            self.server.errors.append('unexpected configuration request/auth')
            self.respond(400)

    def do_POST(self):
        try:
            token = self.headers.get('Authorization', '').removeprefix('Bearer ')
            require(self.path == '/v1/batches' and token in self.server.tokens
                    and self.headers.get('Authorization') == 'Bearer ' + token
                    and self.headers.get('Content-Type') == 'application/x-protobuf'
                    and not self.headers.get('Transfer-Encoding'), 'real authenticated Batch request')
            size = int(self.headers.get('Content-Length', '0'))
            require(0 < size <= 131072, 'bounded request')
            raw = self.rfile.read(size)
            require(len(raw) == size, 'complete request')
            identity = batch_identity(raw)
            with self.server.lock:
                self.server.requests += 1
                require(self.server.requests <= 10000, 'finite request allowance')
                stream, sequence = identity[:2], identity[2]
                node, through = self.server.streams.get(token, (stream, 0))
                require(node == stream and sequence in (through, through + 1), 'per-token stream and ordered ACK')
                require(identity not in self.server.raw or self.server.raw[identity] == raw, 'exact retry bytes')
                if identity not in self.server.raw:
                    self.server.raw[identity] = raw
                    (self.server.root / ('batch-' + identity[0] + '-' + str(sequence))).write_bytes(raw)
                self.server.streams[token] = (stream, through if self.server.pending else sequence)
            self.respond(503 if self.server.pending else 200,
                         {'status': 'unavailable'} if self.server.pending else {'status': 'ack', 'committed_through': sequence})
        except Exception as error:
            self.server.errors.append(str(error))
            self.respond(400)


def original_records(root, sink, identities, pending=False):
    def lines(name):
        with open(root / name) as stream:
            for n, line in enumerate(stream):
                require(n < 25000 and len(line) <= 16384, 'bounded original ledger')
                yield json.loads(line)
    events = list(lines('events.jsonl'))
    created = [(item['id'], item['seq']) for item in events if item.get('e') == 'created']
    require(all(type(i) is int and type(s) is int for i, s in created)
            and len(created) == identities * 2 and set(created) == {(i, s) for i in range(identities) for s in (1, 2)},
            'real finite original offers')
    streams = {stream for stream, _ in sink.streams.values()}
    require(len(sink.streams) == len(streams) == identities, 'unique authenticated streams')
    expected = {(*stream, sequence) for stream in streams for sequence in (1, 2)}
    attempted = {key for key in expected if not pending or key[2] == 1}
    require(set(sink.raw) == attempted, 'exact accepted wire identities')
    transcript = list(lines('transcript.jsonl'))
    sources, responses, states, attempts, outstanding = {}, [], {}, set(), {}
    for item in transcript:
        stream = item['node_id'], item['generation']
        require(type(item['generation']) is int and stream in streams, 'original stream identity')
        if item['type'] != 'node_state':
            require(type(item['sequence']) is int, 'original sequence type')
            key = (*stream, item['sequence'])
            require(key in expected, 'original source identity')
        if item['type'] in ('source', 'attempt'):
            raw = sink.raw.get(key)
            require(type(item['bytes']) is str and len(base64.b64decode(item['bytes'], validate=True)) == 32,
                    'original digest encoding')
            if item['type'] == 'source':
                require(key not in sources, 'duplicate original source')
                sources[key] = item['bytes']
            else:
                require(key in attempted and sources.get(key) == item['bytes']
                        and item.get('injected_conflict') is False, 'original attempt/source association')
                attempts.add(key)
                outstanding[key] = outstanding.get(key, 0) + 1
            if raw is not None:
                require(item['bytes'] == base64.b64encode(hashlib.sha256(raw).digest()).decode(), 'original wire hash')
        elif item['type'] == 'response':
            require(outstanding.get(key, 0) > 0, 'original response/attempt association')
            outstanding[key] -= 1
            require(pending or type(item.get('committed_through')) is int, 'original ACK cursor type')
            responses.append(item)
        elif item['type'] == 'node_state':
            require(stream not in states and type(item['ack_cursor']) is int
                    and all(type(seq) is int for seq in item['retained_sequences']), 'unique original state identity')
            states[stream] = item
        else:
            raise ValueError('unexpected original transcript record')
    require(set(sources) == expected and attempts == attempted and not any(outstanding.values())
            and set(states) == streams and not sink.errors, 'complete original streams/setup')
    if pending:
        require(len(sink.raw) == identities and responses and all(item['kind'] == 'unavailable' for item in responses)
                and all(item['ack_cursor'] == 0 and item['retained_sequences'] == [1, 2] for item in states.values()), 'real undelivered fixture')
    else:
        require(len(sink.raw) == identities * 2 and len(responses) == identities * 2
                and all(item['kind'] == 'ack' and item['committed_through'] == item['sequence'] for item in responses)
                and all(item['ack_cursor'] == 2 and item['retained_sequences'] == [] for item in states.values()), 'real finite ACK prerequisite')
    return events


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case', choices=CASES, required=True)
    for name in ('binary', 'authority', 'build-receipt', 'out'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--authority-sha256', required=True)
    args = parser.parse_args()
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    from resource_group import require_limits, require_storage, STORAGE
    from delivery_faults import make_certs
    group = require_limits()
    require(STORAGE == DATA and require_storage() == DATA, 'mounted DATA storage only')
    require(not any(name in os.environ for name in ('LD_PRELOAD', 'LD_AUDIT', 'LD_LIBRARY_PATH')), 'unpreloaded native process required')
    require(file_hash(args.authority) == args.authority_sha256, 'external expected authority hash')
    authority = binding(read_json(args.authority), args.case)
    require(file_hash(args.build_receipt) == authority['build_receipt_sha256'], 'actual build receipt bytes')
    root = args.out.absolute()
    require(root.parent.resolve().is_relative_to(DATA) and root.parent.resolve() == root.parent and not root.exists(), 'fresh owned DATA path')
    root.mkdir(mode=0o700)
    report = dict(case=args.case, classification='volatile TLS diagnostic; no candidate qualification',
                  authority=authority, authority_sha256=args.authority_sha256, cgroup=str(group), status='inconclusive')
    sink = thread = proc = None
    fd = witness_fd = None
    result = 2
    try:
        fd = os.open(args.binary, os.O_RDONLY | os.O_NOFOLLOW)
        binary_stat = os.fstat(fd)
        with os.fdopen(os.dup(fd), 'rb') as stream:
            require(stat.S_ISREG(binary_stat.st_mode) and hashlib.file_digest(stream, 'sha256').hexdigest()
                    == authority['binary_sha256'], 'held binary bytes')
        identities, workers = (5, 4) if args.case == 'workers' else (2, 2)
        make_certs(root)
        tokens = root / 'tokens'
        tokens.write_text(''.join(f'diagnostic-token-{i}\n' for i in range(identities)))
        tokens.chmod(0o600)
        sink = Sink(root, identities, args.case == 'pending')
        thread = threading.Thread(target=sink.serve_forever, kwargs={'poll_interval': 0.05})
        thread.start()
        begin = root / 'begin.json'
        if args.case == 'existing':
            begin.write_bytes(b'never replace this destination\n')
            begin.chmod(0o400)
        command = [f'/proc/self/fd/{fd}', '--server-url', f'https://127.0.0.1:{sink.server_port}',
                   '--ca', str(root / 'ca.pem'), '--tokens', str(tokens), '--seed', '0xA11FA001',
                   '--seconds', '2', '--workers', str(workers), '--out', str(root)]
        if args.case not in ('legacy', 'canonical-red'):
            command += ['--begin-witness', str(begin)]
        if args.case == 'duplicate':
            command += ['--begin-witness', str(root / 'other.json')]
        report['command'], report['launch_monotonic_ns'] = command, time.monotonic_ns()
        with open(root / 'stdout', 'wb') as stdout, open(root / 'stderr', 'wb') as stderr:
            proc = subprocess.Popen(command, pass_fds=(fd,), stdout=stdout, stderr=stderr)
        if args.case not in ('duplicate', 'existing', 'workers'):
            report['process'] = process_identity(proc.pid, binary_stat, authority['binary_sha256'])
        if args.case in ('positive', 'pending', 'early', 'changed'):
            until = time.monotonic() + 1
            while not (begin.exists() and sink.raw) and proc.poll() is None and time.monotonic() < until:
                time.sleep(0.002)
            require(begin.exists() and sink.raw and proc.poll() is None, 'live witness/offer prerequisite')
            witness_fd = os.open(begin, os.O_RDONLY | os.O_NOFOLLOW)
            live_stat = os.fstat(witness_fd)
            live_raw = os.read(witness_fd, 131073)
            require(len(live_raw) <= 131072 and stat.S_ISREG(live_stat.st_mode) and live_stat.st_nlink == 1
                    and stat.S_IMODE(live_stat.st_mode) == 0o400, 'live complete private witness')
            live_value = read_json(Path('/proc/self/fd') / str(witness_fd))
            report['live_begin'] = dict(sha256=hashlib.sha256(live_raw).hexdigest(), value=live_value,
                observed_monotonic_ns=time.monotonic_ns(), device=live_stat.st_dev, inode=live_stat.st_ino)
            if args.case == 'early':
                proc.send_signal(signal.SIGTERM)
            elif args.case == 'changed':
                begin.chmod(0o600)
                with open(begin, 'ab') as stream:
                    stream.write(b'changed-by-registered-control')
                begin.chmod(0o400)
        code = proc.wait(timeout=75 if args.case == 'pending' else 10)
        report['exit'], report['stop_confirmed'], report['stopped_monotonic_ns'] = code, True, time.monotonic_ns()
        sink.shutdown(); thread.join(timeout=5); sink.server_close()
        require(not thread.is_alive(), 'test sink stopped')
        if args.case in ('duplicate', 'existing', 'workers'):
            require(code in (1, 2) and not sink.raw and not sink.errors, 'refusal before any native request')
            require(not (root / 'other.json').exists(), 'no duplicate-option publication')
            if args.case == 'existing':
                require(begin.read_bytes() == b'never replace this destination\n' and stat.S_IMODE(begin.stat().st_mode) == 0o400, 'destination preserved')
            else:
                require(not begin.exists(), 'no refused witness publication')
        elif args.case in ('early', 'changed'):
            require(code != 0, 'real native failure must refuse success')
        else:
            events = original_records(root, sink, identities, args.case == 'pending')
            summary = read_json(root / 'sim-summary.json')
            require(summary['identities'] == identities and summary['seconds'] == 2
                    and summary['seed'] == 0xA11FA001, 'original summary parameters')
            if args.case == 'pending':
                require(code == 1 and summary['undelivered'] == 4, 'native undelivered refuses success')
            else:
                require(code == 0 and summary['undelivered'] == 0, 'successful finite ACK drain')
                if args.case == 'canonical-red':
                    require(not begin.exists(), 'sealed source changed: unexpected witness')
                    report['status'], result = 'expected-red: sealed-native-missing-begin-after-real-ACK', 1
                elif args.case == 'legacy':
                    require(set(summary) == {'identities', 'seconds', 'seed', 'began_unix_ns', 'undelivered'} and not begin.exists(), 'legacy summary compatibility')
                else:
                    witness_stat = begin.lstat()
                    witness = read_json(begin)
                    require(stat.S_ISREG(witness_stat.st_mode) and witness_stat.st_nlink == 1
                            and stat.S_IMODE(witness_stat.st_mode) == 0o400
                            and (witness_stat.st_dev, witness_stat.st_ino) == (live_stat.st_dev, live_stat.st_ino)
                            and file_hash(begin) == report['live_begin']['sha256'], 'unchanged complete private single-link witness')
                    require(witness['process'] == report['process'] and witness['binary_sha256'] == authority['binary_sha256']
                            and witness['parameters'] == dict(seconds=2, identities=2, workers=2, seed=0xA11FA001), 'actual native identity/parameters')
                    lo, hi = witness['mono_before_ns'], witness['mono_after_ns']
                    require(type(lo) is int and type(hi) is int and report['launch_monotonic_ns'] <= lo <= hi <= lo + 1000000
                            and hi <= report['live_begin']['observed_monotonic_ns'], 'native bounded bracket')
                    require(summary['began_unix_ns'] == witness['began_unix_ns'] and summary['process'] == report['process']
                            and summary['begin_witness_sha256'] == file_hash(begin) and summary['workers'] == 2
                            and summary['elapsed_ns'] >= 2000000000
                            and hi + 2000000000 <= summary['ended_monotonic_ns'] <= report['stopped_monotonic_ns'],
                            'real conservative native end')
                    require(all(item['t'] >= witness['began_unix_ns'] and item['generated_ns'] >= item['t']
                                for item in events if item.get('e') == 'created'), 'original offer anchor')
                    report['publication_certification'] = 'not established: separate atomic/no-replace/pre-offer controls required'
        if args.case != 'canonical-red':
            report['status'], result = 'diagnostic-control-satisfied', 0
    except Exception as error:
        report['error'] = str(error)
    finally:
        if proc is not None and proc.poll() is None:
            proc.kill(); proc.wait(timeout=10)
        if sink is not None and thread is not None and thread.is_alive():
            sink.shutdown(); thread.join(timeout=5); sink.server_close()
        if fd is not None:
            os.close(fd)
        if witness_fd is not None:
            os.close(witness_fd)
        # Retain original failures/raw batches/receipts; discard only fixture credentials.
        for name in ('ca.key', 'server.key', 'tokens', *(f'token-{i}' for i in range(5))):
            (root / name).unlink(missing_ok=True)
        report['helper_stopped'] = proc is None or proc.poll() is not None
        report['sink_stopped'] = thread is None or not thread.is_alive()
        (root / 'control-receipt.json').write_text(json.dumps(report, indent=2) + '\n')
    return result


if __name__ == '__main__':
    try:
        sys.exit(main())
    except Exception as error:
        print(json.dumps({'status': 'inconclusive-preflight-refusal', 'error': str(error)}), file=sys.stderr)
        sys.exit(2)
