#!/usr/bin/env python3
"""Execute the registered fresh-owner guide in one fresh native OS pair.

Placement/observers are explicit harness actions. Guide fences execute unchanged
with source spans and call/return records; no legacy provisioning is imported.
The exact input admission is frozen externally only after actual merge/freeze.
"""
import argparse
import base64
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path[:0] = [str(HERE), str(ROOT / 'tools'), str(ROOT / 'tools/ui')]
from owner_commands import Commands, PHASES, private_write
from owner_guide import Guide
from owner_evidence import require, grade_queries, completed
from cross_family.guest import Guest, Q, image
from cross_family.inputs import validate_inputs

ORIGIN = 'https://telemetry.example.com:7443'
RESERVATION = 13_958_643_712


def sha(path):
    return Q.digest(Path(path), 'sha256')


def artifact(path, expected):
    path = Path(path)
    path.resolve(strict=True).relative_to(Q.DATA.resolve(strict=True))
    require(not path.is_symlink() and sha(path) == expected, 'exact admitted DATA input changed')
    return path


def admit(path, expected):
    plan = json.loads(artifact(path, expected).read_text())
    require(re.fullmatch('[0-9a-f]{40}', plan['harness_commit'])
            and Q.run(['git', 'rev-parse', 'HEAD']).stdout.strip() == plan['harness_commit']
            and not Q.run(['git', 'status', '--porcelain']).stdout, 'clean admitted harness required')
    Q.run(['git', 'merge-base', '--is-ancestor', plan['harness_commit'], 'origin/main'])
    Q.run(['git', 'merge-base', '--is-ancestor', plan['guide_commit'], 'origin/main'])
    for relative, expected_hash in plan['harness_files'].items():
        require(not Path(relative).is_absolute() and '..' not in Path(relative).parts
                and sha(ROOT / relative) == expected_hash, 'admitted harness source changed')
    artifacts = validate_inputs(Path(plan['deb_receipt']), Path(plan['rpm_receipt']))
    source = json.loads(Path(plan['deb_receipt']).read_text())['source']
    require(source['original_head'] == plan['original_head']
            and source['source_date_epoch'] == plan['epoch']
            and artifacts['identity']['source_commit'] == plan['source_commit'], 'registered candidate source differs')
    for family in ('debian', 'fedora'):
        require(sha(artifacts[family]) == plan['package_sha256'][family], 'registered package differs')
    # Fresh exact native installation results are not historical pass substitutes.
    require(set(plan['installation']) == {'debian', 'fedora'}, 'both fresh installation prerequisites required')
    for family, item in plan['installation'].items():
        receipt = json.loads(artifact(item['path'], item['sha256']).read_text())
        require(receipt['package_family'] == family and receipt['package_sha256'] == plan['package_sha256'][family]
                and receipt['source_snapshot_commit'] == plan['source_commit'] and receipt['mutation'] is None
                and receipt['acceptance_exit'] == 0 and receipt['cleanup_confirmed'] is True
                and receipt['qemu_process_cleanup_confirmed'] is True, 'fresh exact native installation not completed')
        outer = json.loads(artifact(item['outer'], item['outer_sha256']).read_text())
        require(outer['exit'] == 0 and outer['stop_confirmed'] is True, 'prerequisite outer stop unconfirmed')
    fixture = json.loads(artifact(plan['fixture_receipt'], plan['fixture_sha256']).read_text())
    require(fixture['state'] == 'built' and fixture['exit'] == 0 and fixture['source'] == source
            and fixture['native_older'] == {'debian': True, 'fedora': True}, 'native same-code constructor not completed')
    outer = json.loads(artifact(plan['fixture_outer'], plan['fixture_outer_sha256']).read_text())
    require(outer['exit'] == 0 and outer['stop_confirmed'] is True, 'constructor outer stop unconfirmed')
    for family in ('debian', 'fedora'):
        artifact(fixture['packages'][family]['package'], fixture['packages'][family]['sha256'])
    for item in plan['browser_controls']:
        control = json.loads(artifact(item['path'], item['sha256']).read_text())
        require(control['exit'] == 0 and control['guardian_own_children_empty'] is True,
                'native browser mechanism control incomplete')
    require({item['case'] for item in plan['browser_controls']} == {'trusted', 'wrong-san', 'untrusted-ca'},
            'three native browser mechanism controls required')
    guides = {}
    for document in ('docs/operations.md', 'docs/access-operations.md'):
        raw = subprocess.check_output(['git', 'show', plan['guide_commit'] + ':' + document], cwd=ROOT)
        guides[document] = Guide(document, raw, plan['guide_sha256'][document])
    return plan, artifacts, fixture, guides


@dataclass(frozen=True)
class Prose:
    document: str
    heading: str
    start: int
    end: int
    text: str
    command: bytes
    purpose: str

    def record(self):
        return dict(kind='prose', document=self.document, heading=self.heading,
                    opening_line=self.start, closing_line=self.end, text=self.text,
                    text_sha256=hashlib.sha256(self.text.encode()).hexdigest(),
                    command=self.command.decode(), purpose=self.purpose)


class Walkthrough:
    def __init__(self, plan, artifacts, fixture, guides, root, out, server_family):
        self.plan, self.artifacts, self.fixture, self.guides = plan, artifacts, fixture, guides
        self.root, self.out = root, out
        self.commands = Commands(out / 'protected-transcript')
        self.record = {'classification': 'incomplete native owner walkthrough', 'exit': 2,
            'binding': {key: plan[key] for key in ('original_head', 'epoch', 'source_commit', 'guide_commit', 'harness_commit')},
            'signals': {}, 'owner_enrollments': [], 'native_older': fixture['native_older'], 'events': []}
        self.record['binding'].update(deb_sha256=plan['package_sha256']['debian'], rpm_sha256=plan['package_sha256']['fedora'])
        self.key = root / 'id_ed25519'
        self.bridge = self.budget = None
        self.server = Guest('server', server_family, root, out, self.key, 4096, 7443, network='192.0.2.0/24')
        self.edge = Guest('edge', 'fedora' if server_family == 'debian' else 'debian', root, out, self.key, 1536,
                          network='192.0.2.0/24')
        self.queries = []
        self.query_stages = {}
        self.source_log = b'owner-walkthrough-original-log\n'

    def event(self, kind, **value):
        self.record['events'].append(dict(kind=kind, phase=PHASES[self.commands.index][0],
                                          at_ns=time.monotonic_ns(), **value))

    def prose(self, guest, anchor, needle, command, *, document='docs/operations.md', stdin=None, purpose=None):
        guide = self.guides[document]
        lines = guide.raw.decode().splitlines(keepends=True)
        matches = [number for number, line in enumerate(lines, 1) if needle in line]
        require(len(matches) == 1, 'prose action requires one exact guide source span')
        at = matches[0]
        action = Prose(document, anchor, at, at, lines[at - 1], command.encode(), purpose or needle)
        return self.commands.shell(guest, action, stdin=stdin)

    def observed(self, guest, command, label):
        # Observers are distinctly recorded; they never masquerade as guide fences.
        before = time.monotonic_ns()
        result = guest.script(command, timeout=min(30, self.commands.remaining()))
        path = self.out / 'protected-transcript' / (label + '.json')
        private_write(path, json.dumps({'role': guest.role, 'boot_id': guest.boot_id,
            'command': command, 'start_ns': before, 'end_ns': time.monotonic_ns(),
            'exit': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}).encode())
        self.commands.remaining()
        return result.stdout

    def fence(self, guest, anchor, index=0, *, document='docs/operations.md', stdin=None, principal=None):
        block = self.guides[document].section(anchor)[index]
        if principal is not None:
            require(re.fullmatch('[A-Za-z0-9_-]{1,128}', principal), 'invalid principal substitution')
            from dataclasses import replace
            original = block
            block = replace(block, command=block.command.replace(b'OWNER_PRINCIPAL_ID', principal.encode()))
            self.event('declared-substitution', source=original.record(), name='OWNER_PRINCIPAL_ID', public_value=principal)
        return self.commands.shell(guest, block, stdin=stdin)

    def install(self, guest, candidate):
        package = self.artifacts[guest.family] if candidate else Path(self.fixture['packages'][guest.family]['package'])
        for suffix in ('deb', 'rpm'):
            self.prose(guest, 'install', 'Download the `.deb` or `.rpm` and `SHA256SUMS`',
                       'rm -f ~/fabrico11y_*.' + suffix + ' ~/fabrico11y-*.' + suffix + '\n',
                       purpose='Remove only earlier staged package filenames so the guide wildcard selects exactly one artifact')
        guest.copy(package, '/home/accept/' + package.name)
        sums = self.root / (guest.role + '-SHA256SUMS')
        sums.write_text(sha(package) + '  ' + package.name + '\n')
        guest.copy(sums, '/home/accept/SHA256SUMS')
        self.event('package-stage', role=guest.role, family=guest.family, candidate=candidate,
                   package=package.name, sha256=sha(package))
        self.fence(guest, 'install', 0 if guest.family == 'debian' else 1)

    def pages(self, query):
        pages, request = [], dict(query)
        for _ in range(1000):
            self.commands.remaining()
            time.sleep(.25)
            status, answer = self.bridge.request('/v1/console/query', 'POST', request)
            require(status == 200, 'authorized query refused')
            pages.append(answer)
            require(sum(len(json.dumps(page)) for page in pages) <= 16 * 1024**2, 'query evidence exceeds bound')
            if not answer.get('next_page'):
                self.queries.append((query, pages))
                return pages
            request['page'] = answer['next_page']
        raise ValueError('query page count exceeded')

    def received(self, stage):
        before = len(self.queries)
        upper = time.time_ns()
        for kind in ('logs', 'metrics', 'spans'):
            query = dict(kind=kind, node='edge', from_ns=upper - 900 * 10**9, to_ns=upper, limit=100)
            if kind == 'metrics': query['name'] = 'system.cpu.time'
            self.pages(query)
        self.query_stages[stage] = (before, len(self.queries))

    def snapshot(self, stage):
        require('inactive' in self.observed(self.server,
            'systemctl is-active fabrico11y-server.service || test "$?" = 3\n', stage + '-stopped'),
            'stopped server required before raw journal capture')
        text = self.observed(self.server, r'''python3 - <<'OBS'
import base64,json,re
from pathlib import Path
root=Path('/var/lib/fabrico11y/server/journal')
files=sorted(p for p in root.iterdir() if re.fullmatch(r'(batches|sealed-\d{20})\.faj',p.name))
total=0
for p in files:
 if p.is_symlink() or not p.is_file():raise ValueError('unsafe journal member')
 total+=p.stat().st_size
 if total>128*1024**2:raise ValueError('journal evidence bound')
 print(json.dumps({'file':p.name,'bytes':base64.b64encode(p.read_bytes()).decode()}))
OBS
''', stage + '-original-journals')
        journals = [json.loads(line) for line in text.splitlines()]
        replay = [json.loads(line) for line in self.observed(self.server,
            '/root/owner-observers/server_dump /etc/fabrico11y/server.conf --records\n', stage + '-replay').splitlines()]
        for name, (start, end) in self.query_stages.items():
            if name not in self.record['signals']:
                self.record['signals'][name] = grade_queries(journals, replay, self.queries[start:end], self.source_log)
        private_write(self.out / 'protected-transcript' / (stage + '-queries.json'), json.dumps(self.queries).encode())
