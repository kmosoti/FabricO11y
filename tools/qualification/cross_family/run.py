#!/usr/bin/env python3
"""Registered four cross-family cells. Exact candidate inputs are mandatory."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import ssl
import subprocess
import time
import urllib.request
from guest import Guest, Q, image

HERE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run-id', required=True)
    parser.add_argument('--server-family', choices=('debian', 'fedora'), required=True)
    parser.add_argument('--edge-family', choices=('debian', 'fedora'), required=True)
    parser.add_argument('--deb-receipt', type=Path, required=True)
    parser.add_argument('--rpm-receipt', type=Path, required=True)
    args = parser.parse_args()
    if not re.fullmatch(r'[a-z0-9][a-z0-9-]{0,79}', args.run_id):
        raise ValueError('invalid run identity')
    limits = Q.require_containment()
    # Candidate adapters validate receipts, exact source bundles, installed
    # payload equivalence and console identity before constructing a guest.
    from inputs import validate_inputs
    artifacts = validate_inputs(args.deb_receipt, args.rpm_receipt)
    for family in (args.server_family, args.edge_family):
        image(family)
    Q.GUEST_DISK_BYTES = 12 * 1024**3
    admission = Q.require_data_budget()
    root = Q.DATA / 'scratch' / ('cross-family-' + args.run_id)
    result = Q.DATA / 'results/cross-family' / args.run_id
    root.mkdir(exist_ok=False)
    result.mkdir(parents=True, exist_ok=False)
    facts = {'run_id': args.run_id, 'classification': 'finite local two-OS VM transport/custody cell; migration provision fixture; not production access qualification',
             'server_family': args.server_family, 'edge_family': args.edge_family,
             'inputs': artifacts['identity'], 'storage_admission': admission, 'host_cgroup': limits,
             'exit': 2, 'gates': {}}
    for filename in ('run.py', 'guest.py', 'producer.py', 'inputs.py', 'grade.py'):
        shutil.copy2(HERE / filename, result / filename)
    key = root / 'id_ed25519'
    Q.run([Q.executable('ssh-keygen'), '-q', '-t', 'ed25519', '-N', '', '-f', str(key)], timeout=30)
    data_port = Q.free_port()
    server = Guest('server', args.server_family, root, result, key, 4096, data_port)
    edge = Guest('edge', args.edge_family, root, result, key, 1536)
    began = time.monotonic()
    try:
        server.start()
        edge.start()
        server.ready()
        edge.ready()
        if server.boot_id == edge.boot_id:
            raise ValueError('two distinct boot identities are required')
        facts['placement'] = {'server_boot_id': server.boot_id, 'edge_boot_id': edge.boot_id,
                              'server_command': server.command, 'edge_command': edge.command,
                              'edge_destination': f'https://10.0.2.2:{data_port}',
                              'host_query_destination': f'https://127.0.0.1:{data_port}'}
        for guest in (server, edge):
            guest.install(artifacts[guest.family], artifacts['helpers'])
        for command in [
            ['openssl', 'req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:P-256', '-nodes', '-days', '2', '-subj', '/CN=cross-family-owned-ca', '-keyout', str(root / 'ca.key'), '-out', str(root / 'ca.pem')],
            ['openssl', 'req', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:P-256', '-nodes', '-subj', '/CN=cross-family-server', '-keyout', str(root / 'server.key'), '-out', str(root / 'server.csr')],
        ]:
            Q.run(command, timeout=30)
        (root / 'san.ext').write_text('subjectAltName=IP:127.0.0.1,IP:10.0.2.2\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n')
        Q.run(['openssl', 'x509', '-req', '-in', str(root / 'server.csr'), '-CA', str(root / 'ca.pem'), '-CAkey', str(root / 'ca.key'), '-CAcreateserial', '-days', '2', '-extfile', str(root / 'san.ext'), '-out', str(root / 'server.pem')], timeout=30)
        admin = hashlib.sha256(os.urandom(32)).hexdigest()
        (root / 'admin-token').write_text(admin + '\n')
        for filename in ('ca.pem', 'server.pem', 'server.key', 'admin-token'):
            server.copy(root / filename, '/home/accept/' + filename)
        server.script('''set -eu
install -m 0644 /home/accept/ca.pem /home/accept/server.pem /etc/fabrico11y/
install -m 0640 -o root -g fabricolly /home/accept/server.key /home/accept/admin-token /etc/fabrico11y/
cat > /etc/fabrico11y/server.conf <<'EOF'
listen=0.0.0.0:7443
tls_cert=/etc/fabrico11y/server.pem
tls_key=/etc/fabrico11y/server.key
admin_token_file=/etc/fabrico11y/admin-token
state_dir=/var/lib/fabrico11y/server
journal_bytes=1073741824
journal_file_bytes=16777216
retention_s=86400
retention_bytes=100000000000
self_spindle_ca=/etc/fabrico11y/ca.pem
EOF
mkdir -p /etc/systemd/system/fabrico11y-server.service.d
printf '[Service]\nExecStart=\nExecStart=/usr/bin/fabric-server serve-legacy /etc/fabrico11y/server.conf --timing-events\n' > /etc/systemd/system/fabrico11y-server.service.d/cross-fixture.conf
if command -v restorecon >/dev/null; then restorecon -R /etc/fabrico11y; fi
systemctl daemon-reload
systemctl enable --now fabrico11y-server
''', timeout=60)
        context = ssl.create_default_context(cafile=str(root / 'ca.pem'))
        base = f'https://127.0.0.1:{data_port}'
        def call(route, body=None, method=None):
            request = urllib.request.Request(base + route, data=None if body is None else json.dumps(body).encode(),
                        method=method, headers={'authorization': 'Bearer ' + admin, 'content-type': 'application/json'})
            with urllib.request.urlopen(request, context=context, timeout=15) as response:
                return json.loads(response.read())
        deadline = time.monotonic() + 60
        while True:
            try:
                call('/v1/admin/nodes')
                break
            except OSError:
                if time.monotonic() > deadline:
                    raise RuntimeError('installed server did not start verified TLS')
                time.sleep(1)
        edge_token = call('/v1/admin/nodes', {'name': 'edge', 'logs': ['/var/log/fabric-cross/source.log'], 'metric_interval_s': 15})['token']
        metric_token = call('/v1/admin/nodes', {'name': 'controlled', 'metric_interval_s': 15})['token']
        for guest in (edge,):
            guest.copy(root / 'ca.pem', '/home/accept/ca.pem')
        (root / 'edge-token').write_text(edge_token + '\n')
        (root / 'controlled-token').write_text(metric_token + '\n')
        for filename in ('edge-token', 'controlled-token'):
            edge.copy(root / filename, '/home/accept/' + filename)
        config = {'root': '/root/cross', 'log': '/var/log/fabric-cross/source.log',
                  'ca': '/etc/fabrico11y/ca.pem', 'token_file': '/root/cross/controlled-token',
                  'node_id': hashlib.sha256((args.run_id + ':controlled').encode()).hexdigest()[:32],
                  'server_url': f'https://10.0.2.2:{data_port}'}
        (root / 'producer.json').write_text(json.dumps(config))
        edge.copy(root / 'producer.json', '/home/accept/producer.json')
        edge.copy(HERE / 'producer.py', '/home/accept/producer.py')
        edge.script(f'''set -eu
install -m 0644 /home/accept/ca.pem /etc/fabrico11y/ca.pem
install -m 0640 -o root -g fabricolly /home/accept/edge-token /etc/fabrico11y/node-token
install -m 0600 /home/accept/controlled-token /root/cross/controlled-token
install -m 0600 /home/accept/producer.json /root/cross/producer.json
install -m 0700 /home/accept/producer.py /root/cross/producer.py
install -d -m 0755 /var/log/fabric-cross
install -m 0644 /dev/null /var/log/fabric-cross/source.log
cat > /etc/fabrico11y/node.conf <<'EOF'
spool_dir=/var/lib/fabrico11y/node/spool
metric_interval_s=15
spool_bytes=33554432
log=/var/log/fabric-cross/source.log
server_url=https://10.0.2.2:{data_port}
server_ca=/etc/fabrico11y/ca.pem
token_file=/etc/fabrico11y/node-token
traces_listen=127.0.0.1:4318
EOF
mkdir -p /etc/systemd/system/fabrico11y-node.service.d
printf '[Service]\\nExecStart=\\nExecStart=/usr/bin/fabric-node run /etc/fabrico11y/node.conf --timing-events\\n' > /etc/systemd/system/fabrico11y-node.service.d/cross-fixture.conf
if command -v restorecon >/dev/null; then restorecon -R /etc/fabrico11y /var/log/fabric-cross; fi
systemctl daemon-reload
systemctl enable --now fabrico11y-node
systemd-run --unit=cross-producer --property=MemoryMax=128M --property=MemorySwapMax=0 --property=TasksMax=32 /usr/bin/python3 -B /root/cross/producer.py --config /root/cross/producer.json
''', timeout=60)
        # Grading uses all populations; unjoined clocks or missing evidence are
        # incomplete gates. No omitted gate can be promoted to a successful cell.
        from grade import measure_and_grade
        facts['gates'] = measure_and_grade(server, edge, call, config, result, data_port)
        facts['exit'] = 0 if facts['gates'] and all(g['passed'] for g in facts['gates'].values()) else 1
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        facts['error'] = f'{type(error).__name__}: {error}'
    finally:
        for guest in (edge, server):
            if guest.child is not None and guest.child.poll() is None:
                try:
                    diagnostic = guest.script('set +e\nsystemctl stop cross-producer fabrico11y-node fabrico11y-server\njournalctl --no-pager -u fabrico11y-node -u fabrico11y-server -u cross-producer\n', timeout=60, check=False)
                    (result / (guest.role + '-journal.txt')).write_text(diagnostic.stdout + diagnostic.stderr)
                except (OSError, subprocess.SubprocessError) as error:
                    facts[guest.role + '_diagnostic_error'] = str(error)
        cleanup = [guest.stop() for guest in (edge, server)]
        facts['process_cleanup_confirmed'] = all(cleanup)
        if facts['exit'] == 0 and all(cleanup):
            shutil.rmtree(root)
        facts['scratch_removed'] = not root.exists()
        facts['scratch_retained'] = str(root) if root.exists() else None
        facts['elapsed_seconds'] = round(time.monotonic() - began, 3)
        (result / 'receipt.json').write_text(json.dumps(facts, indent=2) + '\n')
        print(json.dumps(facts, indent=2))
    return facts['exit']


if __name__ == '__main__':
    raise SystemExit(main())
