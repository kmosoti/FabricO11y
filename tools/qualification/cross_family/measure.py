"""Finite registered phases and preserved observations; no oracle decisions."""
import json
import base64
from pathlib import Path
import re
import time

NATIVE_METRICS = (
    'system.cpu.time', 'system.memory.total', 'system.memory.available',
    'system.filesystem.capacity', 'system.filesystem.available',
    'system.disk.read.bytes', 'system.disk.write.bytes',
    'system.network.receive.bytes', 'system.network.transmit.bytes',
    'fabric.spindle.committed.bytes', 'fabric.spindle.committed.records',
    'fabric.spindle.delivered.batches', 'fabric.spindle.delivered.bytes',
    'fabric.spindle.throttled.seconds', 'fabric.spindle.spool.bytes',
    'fabric.spindle.unacked.batches', 'fabric.spindle.log.backlog.bytes',
)
SELF_LABEL = 'fabric-server-self'


def json_lines(text):
    return [json.loads(line) for line in text.splitlines() if line.strip()]


def fetch(guest, path, result, name):
    text = guest.script('cat ' + path + '\n', timeout=30).stdout
    (result / name).write_text(text)
    return json_lines(text)


def inspect(guest, config):
    text = guest.script('/usr/bin/fabricctl inspect ' + config + '\n', timeout=30).stdout
    return dict(p.split('=', 1) for p in text.split())


def pages(call, body):
    query, answer, size = dict(body), [], 0
    for _ in range(1000):
        page = call('/v1/admin/query', query, 'POST')
        size += len(json.dumps(page))
        if size > 16 * 1024**2:
            raise ValueError('query pages exceed bounded fixture allowance')
        answer.append(page)
        if not page.get('next_page'):
            return answer
        query['page'] = page['next_page']
    raise ValueError('query page count exceeds finite allowance')


def fixed_queries(call, lower, upper):
    queries = []
    for node in ('edge', SELF_LABEL, 'controlled'):
        for kind in ('logs', 'spans'):
            if node == 'controlled':
                continue
            body = {'kind': kind, 'node': node, 'from_ns': lower, 'to_ns': upper, 'limit': 7}
            queries.append((body, pages(call, body)))
        for name in (('cross.counter',) if node == 'controlled' else NATIVE_METRICS):
            for kind in ('metrics', 'rate'):
                body = {'kind': kind, 'node': node, 'name': name, 'from_ns': lower, 'to_ns': upper}
                if kind == 'metrics':
                    body['limit'] = 7
                queries.append((body, pages(call, body)))
    return queries


def collect(server, edge, call, config, result, data_port):
    deadline = time.monotonic() + 20
    while True:
        read = edge.script('test -s /root/cross/offers.jsonl && head -n 1 /root/cross/offers.jsonl\n', check=False)
        if read.returncode == 0:
            begin = json.loads(read.stdout)
            break
        if time.monotonic() > deadline:
            raise RuntimeError('producer did not publish its beginning clock witness')
        time.sleep(.1)
    if begin['boot_id'] != edge.boot_id or begin['seconds'] != 195:
        raise ValueError('producer boot identity or registered duration differs')
    began, wall = begin['monotonic_ns'], begin['unix_ns']
    edge.script(f'systemd-run --unit=cross-outage --property=MemoryMax=64M --property=MemorySwapMax=0 --property=TasksMax=16 /usr/bin/python3 -B /root/cross/outage.py --begin-ns {began} --port {data_port} --out /root/cross/outage.jsonl\n', timeout=30)
    queries, correlations, phases = [], [], []
    cut = wall + 100 * 1_000_000_000
    outage = removed = prequery = changed = False
    applied = None
    desired_revision = None
    end_deadline = time.monotonic() + 330
    while time.monotonic() < end_deadline:
        probe_edge = edge.correlation()
        probe_server = server.correlation()
        elapsed = (probe_edge['monotonic_before_ns'] - began) / 1e9
        correlations.append({'edge': probe_edge, 'server': probe_server, 'elapsed': elapsed})
        if not changed and elapsed >= 60:
            before = edge.correlation()
            answer = call('/v1/admin/nodes/edge/config',
                          {'logs': [config['log']], 'metric_interval_s': 15}, 'PUT')
            after = edge.correlation()
            phases.append({'type': 'config_update', 'before': before, 'after': after, 'answer': answer})
            desired_revision = answer['revision']
            changed = True
        if changed and applied is None:
            inventory = call('/v1/admin/nodes')
            phases.append({'type': 'inventory', 'edge_clock': probe_edge, 'answer': inventory})
            records = [n for n in inventory['nodes'] if n['name'] == 'edge']
            if len(records) != 1:
                raise ValueError('one exact enrolled edge inventory witness required')
            if records[0]['applied_revision'] == desired_revision and records[0]['config_error'] is None:
                applied = edge.correlation()
                phases.append({'type': 'config_applied', 'clock': applied, 'revision': desired_revision})
        if not prequery and elapsed >= 120:
            snapshots = fixed_queries(call, wall + 15 * 1_000_000_000, cut)
            (result / 'queries-before-outage.json').write_text(json.dumps(snapshots))
            prequery = True
        if not outage and elapsed >= 135:
            outage = True
        if not removed and elapsed >= 195:
            witness = edge.script('systemctl show cross-outage.service -p ActiveState -p ExecMainStatus\n', timeout=15).stdout
            if 'ActiveState=inactive' not in witness or 'ExecMainStatus=0' not in witness:
                time.sleep(.2)
                continue
            phases.extend(fetch(edge, '/root/cross/outage.jsonl', result, 'outage.jsonl'))
            removed = True
        # A visibility sweep covers the complete set of metric names. Each
        # answer is bracketed in both guest clocks; wall time selects rows only.
        if 15 <= elapsed <= 145:
            lower = max(wall, wall + int((elapsed - 10) * 1e9))
            upper = wall + int((elapsed + 1) * 1e9)
            for node in ('edge', SELF_LABEL, 'controlled'):
                node_wall = probe_server['unix_ns'] if node == SELF_LABEL else probe_edge['unix_ns']
                lower, upper = node_wall - 10*10**9, node_wall + 10**9
                bodies = []
                if node != 'controlled':
                    bodies.extend({'kind': k, 'node': node, 'from_ns': lower, 'to_ns': upper, 'limit': 1000}
                                  for k in ('logs', 'spans'))
                for name in (('cross.counter',) if node == 'controlled' else NATIVE_METRICS):
                    bodies.append({'kind': 'metrics', 'node': node, 'name': name,
                                   'from_ns': lower, 'to_ns': upper, 'limit': 1000})
                before = edge.correlation() if node != SELF_LABEL else server.correlation()
                answers = []
                for body in bodies:
                    answer = call('/v1/admin/query', body, 'POST')
                    answers.append((body, answer))
                    time.sleep(.06)
                after = edge.correlation() if node != SELF_LABEL else server.correlation()
                for body, answer in answers:
                    queries.append({'query': body, 'answer': answer, 'before': before, 'after': after})
        if removed:
            producer = edge.script('systemctl show cross-producer.service -p ActiveState -p ExecMainStatus\n', timeout=15).stdout
            views = {'edge': inspect(edge, '/etc/fabrico11y/node.conf'),
                     'self': inspect(server, '/var/lib/fabrico11y/server/self-spindle/node.conf')}
            phases.append({'type': 'drain', 'edge_clock': edge.correlation(), 'producer': producer, 'views': views})
            if 'ActiveState=inactive' in producer and 'ExecMainStatus=0' in producer and all(
                    int(v['acked_through']) == int(v['next_sequence']) - 1 for v in views.values()):
                break
            if elapsed > 315:
                raise RuntimeError('registered native/control drain deadline exceeded')
        time.sleep(.2)
    else:
        raise RuntimeError('finite measurement deadline exceeded')
    if not all((outage, removed, prequery, changed)):
        raise ValueError('registered phase missing')
    (result / 'visibility.json').write_text(json.dumps(queries))
    (result / 'clock-correlations.json').write_text(json.dumps(correlations))
    (result / 'phases.json').write_text(json.dumps(phases))
    offers = fetch(edge, '/root/cross/offers.jsonl', result, 'offers.jsonl')
    controlled = fetch(edge, '/root/cross/controlled-custody.jsonl', result, 'controlled-custody.jsonl')
    snapshots = fixed_queries(call, 0, cut)
    (result / 'queries-after-drain.json').write_text(json.dumps(snapshots))
    outputs, sources, resources = {}, {}, {}
    for guest, service, node_config in ((edge, 'fabrico11y-node', '/etc/fabrico11y/node.conf'),
                                      (server, 'fabrico11y-server', '/var/lib/fabrico11y/server/self-spindle/node.conf')):
        guest.script('systemctl stop cross-monitor ' + service + '\n', timeout=60)
        outputs[guest.role] = guest.script('journalctl --no-pager -o cat -u ' + service + '\n').stdout
        (result / (guest.role + '-timing.txt')).write_text(outputs[guest.role])
        resources[guest.role] = fetch(guest, '/root/cross/resources.jsonl', result, guest.role + '-resources.jsonl')
        text = guest.script('/root/cross/spool_dump ' + node_config + '\n', timeout=60).stdout
        (result / (guest.role + '-spool.jsonl')).write_text(text)
        sources[guest.role] = json_lines(text)
    recovered = json_lines(server.script('/root/cross/server_dump /etc/fabrico11y/server.conf\n', timeout=60).stdout)
    records = json_lines(server.script('/root/cross/server_dump /etc/fabrico11y/server.conf --records\n', timeout=60).stdout)
    prior_records = records
    (result / 'recovered-before-restart.jsonl').write_text(''.join(json.dumps(r) + '\n' for r in recovered))
    (result / 'records-before-restart.jsonl').write_text(''.join(json.dumps(r) + '\n' for r in records))
    server.script('systemctl start fabrico11y-server\n', timeout=60)
    deadline = time.monotonic() + 60
    while True:
        try:
            restart_queries = fixed_queries(call, 0, cut)
            break
        except OSError:
            if time.monotonic() > deadline:
                raise
            time.sleep(.2)
    (result / 'queries-after-restart.json').write_text(json.dumps(restart_queries))
    # Restart creates additional real companion work. Preserve that complete
    # source/attempt prefix too, rather than treating its new snapshot as if it
    # contained only the records recovered before restart.
    server.script('systemctl stop fabrico11y-server\n', timeout=60)
    outputs['server'] = server.script('journalctl --no-pager -o cat -u fabrico11y-server\n').stdout
    (result / 'server-timing.txt').write_text(outputs['server'])
    text = server.script('/root/cross/spool_dump /var/lib/fabrico11y/server/self-spindle/node.conf\n', timeout=60).stdout
    sources['server'] = json_lines(text)
    (result / 'server-spool.jsonl').write_text(text)
    recovered = json_lines(server.script('/root/cross/server_dump /etc/fabrico11y/server.conf\n', timeout=60).stdout)
    records = json_lines(server.script('/root/cross/server_dump /etc/fabrico11y/server.conf --records\n', timeout=60).stdout)
    if records[:len(prior_records)] != prior_records:
        raise ValueError('restart changed the exact retained record prefix')
    (result / 'recovered.jsonl').write_text(''.join(json.dumps(r) + '\n' for r in recovered))
    (result / 'records.jsonl').write_text(''.join(json.dumps(r) + '\n' for r in records))
    journals = json_lines(server.script(r"""python3 - <<'PY'
import base64,json,re
from pathlib import Path
root=Path('/var/lib/fabrico11y/server/journal')
files=sorted(p for p in root.iterdir() if re.fullmatch(r'(batches|sealed-\d{20})\.faj',p.name))
total=0
for p in files:
 if p.is_symlink() or not p.is_file(): raise ValueError('unsafe journal member')
 total+=p.stat().st_size
 if total>128*1024**2: raise ValueError('bounded journal allowance exceeded')
 print(json.dumps({'file':p.name,'bytes':base64.b64encode(p.read_bytes()).decode()}))
PY
""", timeout=60).stdout)
    from snapshot import decode
    groups = sorted((g for file in journals for g in decode(base64.b64decode(file['bytes'], validate=True))), key=lambda g: g['group'])
    if [r for g in groups for r in g['records']] != records:
        raise ValueError('independent journal groups differ from exact stopped replay helper')
    (result / 'journal-groups.json').write_text(json.dumps(groups))
    return {'begin': begin, 'offers': offers, 'controlled': controlled, 'queries': queries,
            'correlations': correlations, 'phases': phases, 'sources': sources, 'outputs': outputs,
            'resources': resources, 'recovered': recovered, 'records': records,
            'groups': groups, 'fixed_queries': [json.loads((result / 'queries-before-outage.json').read_text()), snapshots, restart_queries]}
