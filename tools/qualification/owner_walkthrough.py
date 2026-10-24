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
from owner_evidence import require, grade_queries, grade_custody, completed
import owner_observation
from cross_family.guest import Guest, Q, image
from cross_family.remote import placement_options, validate_placement, edge_guest
from cross_family.inputs import validate_inputs

ORIGIN = 'https://telemetry.example.com:7443'
RESERVATION = 13_958_643_712


def sha(path):
    return Q.digest(Path(path), 'sha256')


def require_admitted_browser_entry_helper(plan, helper, helper_sha256):
    record = plan.get('inputs', {}).get('browser_entry_helper')
    require(type(record) is dict
            and type(record.get('path')) is str
            and Path(record['path']).is_absolute()
            and type(record.get('sha256')) is str
            and re.fullmatch('[0-9a-f]{64}', record['sha256']) is not None
            and record['path'] == str(helper)
            and record['sha256'] == helper_sha256,
            'native browser entry helper differs from admitted owner plan')


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
        require(control['exit'] == 0 and control.get('case') == item['case'] and control['guardian_own_children_empty'] is True,
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
    def __init__(self, plan, artifacts, fixture, guides, root, out, server_family,
                 browser_entry_helper, browser_entry_helper_sha256, placement=None):
        self.plan, self.artifacts, self.fixture, self.guides = plan, artifacts, fixture, guides
        self.root, self.out = root, out
        self.browser_entry_helper = browser_entry_helper
        self.browser_entry_helper_sha256 = browser_entry_helper_sha256
        self.commands = Commands(out / 'protected-transcript')
        self.record = {'classification': 'incomplete native owner walkthrough', 'exit': 2,
            'binding': {key: plan[key] for key in ('original_head', 'epoch', 'source_commit', 'guide_commit', 'harness_commit')},
            'signals': {}, 'owner_enrollments': [], 'native_older': fixture['native_older'], 'events': []}
        self.record['binding'].update(deb_sha256=plan['package_sha256']['debian'], rpm_sha256=plan['package_sha256']['fedora'])
        self.record['binding'].update(browser_entry_helper=str(browser_entry_helper),
                                      browser_entry_helper_sha256=browser_entry_helper_sha256)
        self.expected_binding = dict(self.record['binding'])
        self.key = root / 'id_ed25519'
        self.bridge = self.budget = None
        self.server = Guest('server', server_family, root, out, self.key, 4096, 7443, network='192.0.2.0/24')
        family = 'fedora' if server_family == 'debian' else 'debian'
        self.edge = (edge_guest(placement, family, root, out, self.key, 7443, network='192.0.2.0/24')
                     if placement is not None else
                     Guest('edge', family, root, out, self.key, 1536, network='192.0.2.0/24'))
        self.queries = []
        self.query_stages = {}
        self.source_log = b'owner-walkthrough-original-log\n'
        self.previous_boot_ids = set()

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

    def observed(self, guest, command, label, *, timeout=30):
        # Observers are distinctly recorded; they never masquerade as guide fences.
        before = time.monotonic_ns()
        result = guest.script(command, timeout=min(timeout, self.commands.remaining()), check=False)
        path = self.out / 'protected-transcript' / (label + '.json')
        private_write(path, json.dumps({'role': guest.role, 'boot_id': guest.boot_id,
            'command': command, 'start_ns': before, 'end_ns': time.monotonic_ns(),
            'exit': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}).encode())
        require(len(result.stdout.encode()) + len(result.stderr.encode()) <= 8 * 1024**2, 'observer output exceeded bound')
        require(result.returncode == 0, 'read-only guest observer failed; raw output retained')
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
        self.fence(guest, 'install', 0 if guest.family == 'debian' else 1, stdin=b'y\n')
        self.installed_payload(guest, candidate)

    def pages(self, query):
        pages, request = [], dict(query)
        proof = {'before': self.edge_position()}
        for _ in range(1000):
            self.commands.remaining()
            time.sleep(.25)
            attempted = time.monotonic_ns()
            status, answer = self.bridge.request('/v1/console/query', 'POST', request)
            finished = time.monotonic_ns()
            require(status == 200, 'authorized query refused')
            if not pages:
                proof.update(attempted_ns=attempted, completed_ns=finished, after=self.edge_position())
            pages.append(answer)
            require(sum(len(json.dumps(page)) for page in pages) <= 16 * 1024**2, 'query evidence exceeds bound')
            if not answer.get('next_page'):
                self.queries.append((query, pages, proof))
                return pages
            request['page'] = answer['next_page']
        raise ValueError('query page count exceeded')

    def edge_position(self):
        began = time.monotonic_ns()
        text = self.observed(self.edge, 'fabricctl inspect /etc/fabrico11y/node.conf\n',
                             'edge-query-position-' + str(began))
        fields = dict(word.split('=', 1) for word in text.split() if '=' in word)
        require(fields['coverage_unknown'] == fields['recovery_required'] == fields['interrupted_append'] == 'false',
                'edge inspection incomplete or concurrently interrupted')
        return dict(boot_id=self.edge.boot_id, node_id=fields['node_id'], generation=int(fields['generation']),
                    acked_through=int(fields['acked_through']), next_sequence=int(fields['next_sequence']),
                    started_ns=began, finished_ns=time.monotonic_ns())

    def processes(self, guest, stage, *, active):
        value = json.loads(self.observed(guest, "python3 - <<'OBS'\n" + owner_observation.NATIVE + '\nOBS\n',
                                        stage + '-' + guest.role + ('-active' if active else '-stopped') + '-processes'))
        owner_observation.verify(value, guest.role, guest.boot_id, active=active)
        self.record.setdefault('process_observations', []).append(dict(stage=stage, role=guest.role, active=active, value=value))
        return value

    def received(self, stage):
        for guest in (self.server, self.edge):
            self.processes(guest, stage, active=True)
        before = len(self.queries)
        upper = time.time_ns()
        for kind in ('logs', 'metrics', 'spans'):
            query = dict(kind=kind, node='edge', from_ns=upper - 900 * 10**9, to_ns=upper, limit=100)
            if kind == 'metrics': query['name'] = 'system.cpu.time'
            self.pages(query)
        self.query_stages[stage] = (before, len(self.queries))

    def snapshot(self, stage):
        for guest in (self.server, self.edge):
            self.processes(guest, stage, active=False)
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
        spools = []
        for guest, config in ((self.server, '/var/lib/fabrico11y/server/self-spindle/node.conf'),
                              (self.edge, '/etc/fabrico11y/node.conf')):
            rows = [json.loads(line) for line in self.observed(guest,
                '/root/owner-observers/spool_dump ' + config + '\n', stage + '-' + guest.role + '-original-spool').splitlines()]
            spools.append(rows)
            self.capture_sources(guest, stage)
        self.record.setdefault('custody', {})[stage] = grade_custody(spools, replay)
        for name, (start, end) in self.query_stages.items():
            if name not in self.record['signals']:
                self.record['signals'][name] = grade_queries(journals, replay, self.queries[start:end], self.source_log)
        private_write(self.out / 'protected-transcript' / (stage + '-queries.json'), json.dumps(self.queries).encode())

    def capture_sources(self, guest, stage):
        # Read only the selected log and diagnostic directories. Token, CA and
        # access-store private material is never included in this observation.
        roots = (['/var/lib/fabrico11y/server/diagnostics',
                  '/var/lib/fabrico11y/server/self-spindle/spool/diagnostics'] if guest.role == 'server'
                 else ['/var/log/app.log', '/var/lib/fabrico11y/node/spool/diagnostics'])
        command = "python3 - <<'OBS'\nimport base64,json,os,stat\nfrom pathlib import Path\nroots=" + repr(roots) + r'''
rows=[];total=0
for name in roots:
 root=Path(name)
 if root.is_symlink() or not root.exists():raise ValueError('source input missing or symlinked')
 paths=sorted(root.iterdir()) if root.is_dir() else [root]
 for p in paths:
  before=p.lstat()
  if not stat.S_ISREG(before.st_mode) or before.st_nlink!=1:raise ValueError('source is not an owned regular file')
  total+=before.st_size
  if total>4*1024**2:raise ValueError('source archive exceeds bounded observer output')
  raw=p.read_bytes();after=p.stat()
  if (before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns,before.st_ctime_ns)!=(after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns,after.st_ctime_ns):raise ValueError('stopped source changed')
  rows.append({'path':str(p),'device':before.st_dev,'inode':before.st_ino,'bytes':base64.b64encode(raw).decode()})
print(json.dumps(rows))
OBS
'''
        rows = json.loads(self.observed(guest, command, stage + '-' + guest.role + '-original-source-files'))
        require(rows, 'original selected/diagnostic files missing')
        if guest.role == 'edge':
            original = next(row for row in rows if row['path'] == '/var/log/app.log')
            require(base64.b64decode(original['bytes'], validate=True) == self.source_log, 'original selected log changed')
        self.record.setdefault('original_source_files', {})[stage + '-' + guest.role] = [
            dict(path=row['path'], device=row['device'], inode=row['inode'],
                 sha256=hashlib.sha256(base64.b64decode(row['bytes'], validate=True)).hexdigest()) for row in rows]

    def installed_payload(self, guest, candidate):
        expected = self.artifacts['identity']['payload_identity']
        command = "python3 - <<'OBS'\nimport hashlib,json\nfrom pathlib import Path\nexpected=" + repr(expected) + "\nactual={}\nfor name in expected:\n p=Path('/')/name\n if p.is_symlink() or not p.is_file():raise ValueError('installed payload missing or symlinked')\n actual[name]={'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()}\nprint(json.dumps(actual))\nOBS\n"
        observed = json.loads(self.observed(guest, command,
            'payload-' + guest.role + '-' + str(len(self.record['events']))))
        require(observed == expected, 'installed binaries/complete console differ from canonical payload')
        version_command = "dpkg-query -W -f='${Version}' fabrico11y" if guest.family == 'debian' else "rpm -q --qf '%{VERSION}-%{RELEASE}' fabrico11y"
        version = self.observed(guest, version_command + '\n',
            'version-' + guest.role + '-' + str(len(self.record['events']))).strip()
        expected_version = ('0.1.0~alpha.' if guest.family == 'debian' else '0.1.0-0.alpha.') + ('1' if candidate else '0')
        require(version == expected_version, 'actual installed native package version differs')
        self.event('installed-exact-payload', role=guest.role, candidate=candidate, version=version,
                   payload_sha256=hashlib.sha256(json.dumps(observed, sort_keys=True).encode()).hexdigest())

    def ui_wait(self, predicate):
        deadline = time.monotonic() + min(20, self.commands.remaining())
        while time.monotonic() < deadline:
            if self.bridge._js('return (' + predicate + ')'): return
            self.commands.remaining()
            time.sleep(.1)
        raise TimeoutError('documented browser action did not complete')

    def click(self, text):
        self.bridge._js('''const wanted=arguments[0];const e=Array.from(document.querySelectorAll('button,summary,.nav-item')).find(e=>e.matches('.nav-item')?e.textContent.includes(wanted):e.textContent.trim()===wanted);if(!e||e.disabled)throw Error('documented control unavailable');e.click();''', text)
        time.sleep(.55)
        self.ui_wait("!document.body.textContent.includes('Working…')")

    def field(self, label, value):
        self.bridge._js('''const [label,value]=arguments;const l=Array.from(document.querySelectorAll('label')).find(e=>e.textContent.trim().startsWith(label));const e=l&&l.querySelector('input,textarea,select');if(!e)throw Error('documented field unavailable');e.value=value;e.dispatchEvent(new Event(e.tagName==='SELECT'?'change':'input',{bubbles:true}));''', label, value)

    def browser_observers(self):
        self.bridge._js('''window.ownerPublicCeremonies=[];window.ownerPublicRequests=[];
const originalFetch=window.ownerOriginalFetch||(window.ownerOriginalFetch=window.fetch.bind(window));window.fetch=async(...args)=>{const response=await originalFetch(...args);const route=new URL(typeof args[0]==='string'?args[0]:args[0].url,location.href).pathname;if(route.startsWith('/v1/console/'))window.ownerPublicRequests.push({route,status:response.status});if(window.ownerPublicRequests.length>4096)throw Error('observer bound');return response;};
const create=window.ownerOriginalCreate||(window.ownerOriginalCreate=navigator.credentials.create.bind(navigator.credentials));navigator.credentials.create=async options=>{const credential=await create(options);const data=new Uint8Array(credential.response.getAuthenticatorData());if(data.length<37)throw Error('authenticator data truncated');window.ownerPublicCeremonies.push({rp_hash:Array.from(data.slice(0,32),x=>x.toString(16).padStart(2,'0')).join(''),up:!!(data[32]&1),uv:!!(data[32]&4)});return credential;};''')

    def enroll_owner(self, stage):
        self.record['stale_session_http'] = self.bridge._request('/v1/console/session')[0]
        require(self.record['stale_session_http'] == 401, 'restored stale session was authorized')
        self.browser_observers()
        self.bridge._js("const e=document.querySelector('[data-testid=connect-server]');if(e)e.click();")
        self.ui_wait("document.body.textContent.includes('First owner setup')")
        secret = self.fence(self.server, 'configure-and-enroll-the-first-owner', document='docs/access-operations.md')
        raw = Path(secret['stdout']['path']).read_bytes().strip()
        require(re.fullmatch(b'[a-f0-9]{64}', raw), 'protected bootstrap output malformed')
        self.click('First owner setup')
        self.field('Display name', 'Fresh walkthrough owner')
        self.field('One-time setup secret', raw.decode())
        self.click('Create owner passkey')
        self.ui_wait("document.querySelector('[data-testid=live-console] h1').textContent==='Connected'")
        view = self.bridge.session_view()
        self.bridge.principal_id = view['principal_id']
        require(view['fresh_until_unix_s'] > int(time.time()), 'server did not validate fresh owner authority')
        first = self.bridge.authenticators[0]
        devices = [self.bridge.resident_credentials(first)]
        # Removing the first virtual device forces independent second-device
        # registration, without exporting or importing any private key.
        self.bridge.remove_authenticator(first)
        second = self.bridge.add_authenticator()
        self.click('Settings')
        self.click('Add another passkey')
        self.ui_wait('window.ownerPublicCeremonies.length===2')
        devices.append(self.bridge.resident_credentials(second))
        flags = self.bridge._js('return window.ownerPublicCeremonies')
        expected = hashlib.sha256(b'telemetry.example.com').hexdigest()
        require(len(flags) == 2 and all(row == dict(rp_hash=expected, up=True, uv=True) for row in flags),
                'actual browser authenticator data lacks UP/UV/RP binding')
        require(all(row['count'] == 1 and row['credentials'] == [dict(resident=True, rp='telemetry.example.com')]
                    for row in devices), 'independent resident device evidence missing')
        self.record['owner_enrollments'].append(dict(stage=stage, principal_id=view['principal_id'],
                                                     flags=flags, devices=devices))
        self.event('browser-owner-enrollment', stage=stage, principal_id=view['principal_id'],
                   public_flags=flags, resident_devices=devices)
        self.prose(self.server, 'server', 'On the first successful access-store initialization',
                   'sudo test ! -e /var/lib/fabrico11y/server/access/access-bootstrap.secret\n',
                   document='docs/access-operations.md', purpose='Observe consumed bootstrap secret')
        return view['principal_id']

    def node_enroll(self):
        self.click('Settings')
        self.field('Absolute log paths', '/var/log/app.log')
        self.field('Metric interval', '15')
        self.click('Enroll a new Spindle')
        self.field('New Spindle name', 'edge')
        self.click('Enroll with configuration above')
        token = self.bridge._js("const p=Array.from(document.querySelectorAll('p')).find(e=>e.textContent.startsWith('One-time credential:'));return p&&p.nextElementSibling.textContent")
        require(isinstance(token, str) and re.fullmatch('[a-f0-9]{64}', token), 'one-time edge credential unavailable')
        self.click('Clear displayed credential')
        self.fence(self.edge, 'node', 0, stdin=token.encode() + b'\n')
        self.event('edge-token-installed', token_sha256=hashlib.sha256(token.encode()).hexdigest())
        return token

    def reader(self):
        self.click('Settings')
        self.field('Reader name', 'walkthrough')
        self.click('Issue reader token')
        token = self.bridge._js("const p=Array.from(document.querySelectorAll('p')).find(e=>e.textContent.startsWith('One-time credential:'));return p&&p.nextElementSibling.textContent")
        require(isinstance(token, str) and re.fullmatch('[a-f0-9]{64}', token), 'one-time reader credential unavailable')
        self.click('Clear displayed credential')
        self.prose(self.edge, 'reader-tokens-and-read-only-clients', 'Copy the [access client example]',
                   '''sudo install -d -m 0700 -o accept -g accept /home/operator/.config/fabrico11y
install -m 0600 /dev/stdin /home/operator/.config/fabrico11y/reader-token
cp /usr/share/doc/fabrico11y/examples/access.conf.example /home/operator/.config/fabrico11y/access.conf
sudo install -m 0644 /etc/fabrico11y/ca.pem /home/operator/.config/fabrico11y/ca.pem
sed -i 's|^server_ca=.*|server_ca=/home/operator/.config/fabrico11y/ca.pem|' /home/operator/.config/fabrico11y/access.conf
''', document='docs/access-operations.md', stdin=token.encode() + b'\n', purpose='Create documented access.conf, readable public CA and protected owner-only reader token')
        self.fence(self.edge, 'reader-tokens-and-read-only-clients', document='docs/access-operations.md')
        return token

    def reader_status(self, token):
        # A real query permission is exercised, not an owner-only route that
        # would also refuse a perfectly valid read-only token.
        result = self.prose(self.edge, 'reader-tokens-and-read-only-clients', 'Copy the [access client example]',
            "python3 -c 'import json,ssl,sys,time,urllib.request,urllib.error; token=sys.stdin.read().strip(); now=time.time_ns(); "
            "body=json.dumps({\"kind\":\"logs\",\"node\":\"edge\",\"from_ns\":now-900*10**9,\"to_ns\":now,\"limit\":1}).encode(); "
            "request=urllib.request.Request(\"https://telemetry.example.com:7443/v1/console/query\",data=body,"
            "headers={\"Authorization\":\"Bearer \"+token,\"Content-Type\":\"application/json\",\"X-Fabric-Client-Version\":\"1\"});\n"
            "try:\n response=urllib.request.urlopen(request,context=ssl.create_default_context(cafile=\"/home/operator/.config/fabrico11y/ca.pem\"),timeout=5); print(response.status); response.close()\n"
            "except urllib.error.HTTPError as error: print(error.code)'\n",
            document='docs/access-operations.md', stdin=token.encode(),
            purpose='Observe actual read-only query authority before and after offline recovery')
        return int(Path(result['stdout']['path']).read_text().strip())

    def configure_server(self, reuse=False):
        self.prose(self.server, 'server', '1. Create `/etc/fabrico11y/server.conf`',
            'sudo install -m 0644 /usr/share/doc/fabrico11y/examples/server.conf.example /etc/fabrico11y/server.conf\n'
            "printf 'self_spindle_ca=/etc/fabrico11y/ca.pem\\n' | sudo tee -a /etc/fabrico11y/server.conf >/dev/null\n",
            purpose='Create documented server configuration from packaged example')
        self.event('condition', condition='valid TLS material already available', observed=reuse,
                   selected='reuse protected PKI' if reuse else 'certificate recipe callpoint')
        if reuse:
            self.prose(self.server, 'server', '1. Create `/etc/fabrico11y/server.conf`',
                'sudo install -m 0644 ~/fabric-pki/ca.pem ~/fabric-pki/server.pem /etc/fabrico11y/\n'
                'sudo install -m 0640 -o root -g fabricolly ~/fabric-pki/server.key /etc/fabrico11y/server.key\n',
                purpose='Reuse retained valid public certificate and protected key without regenerating CA')
        else:
            self.event('call', document='docs/operations.md', section='network-and-certificate-setup', return_to='server step1')
            self.fence(self.server, 'network-and-certificate-setup', 0)
            self.fence(self.server, 'network-and-certificate-setup', 1)
            self.event('return', destination='server step1')
        self.prose(self.server, 'network-and-certificate-setup', 'hostname, not an IP address.',
            "printf '127.0.0.1 telemetry.example.com\\n' | sudo tee -a /etc/hosts >/dev/null\n",
            purpose='Declared HOST/IP routing substitution retains hostname/origin/RP/SAN')
        self.prose(self.edge, 'network-and-certificate-setup', 'hostname, not an IP address.',
            "printf '192.0.2.2 telemetry.example.com\\n' | sudo tee -a /etc/hosts >/dev/null\n",
            purpose='Declared 192.0.2.0/24 edge-to-host-forward routing')
        ca = self.observed(self.server, 'cat /etc/fabrico11y/ca.pem\n', 'public-ca-' + str(len(self.record['events'])))
        public_ca = self.root / ('ca-' + str(len(self.record['events'])) + '.pem')
        private_write(public_ca, ca.encode())
        self.edge.copy(public_ca, '/home/accept/ca.pem')
        self.prose(self.edge, 'network-and-certificate-setup', 'edge as `/etc/fabrico11y/ca.pem`',
            'sudo install -m 0644 /home/accept/ca.pem /etc/fabrico11y/ca.pem\n')
        self.prose(self.server, 'server', '2. `sudo systemctl enable',
            'sudo systemctl enable --now fabrico11y-server.service\n')
        leaf = self.root / ('server-' + str(len(self.record['events'])) + '.pem')
        private_write(leaf, self.observed(self.server, 'cat /etc/fabrico11y/server.pem\n', 'public-leaf-' + str(len(self.record['events']))).encode())
        return public_ca, leaf

    def configure_edge(self):
        self.prose(self.edge, 'node', '1. Create `/etc/fabrico11y/node.conf`',
            '''sudo cp /usr/share/doc/fabrico11y/examples/node.conf.example /etc/fabrico11y/node.conf
printf 'server_url=https://telemetry.example.com:7443\nserver_ca=/etc/fabrico11y/ca.pem\ntoken_file=/etc/fabrico11y/node-token\nlog=/var/log/app.log\ntraces_listen=127.0.0.1:4318\n' | sudo tee -a /etc/fabrico11y/node.conf >/dev/null
''')
        if self.edge.family == 'debian':
            self.prose(self.edge, 'node', '2. Give the service read access',
                'command -v setfacl >/dev/null || sudo apt install -y acl\nsudo apt install -y python3-venv\n',
                purpose='Observe/install documented missing Debian ACL and Python venv prerequisites')
        self.prose(self.edge, 'node', '2. Give the service read access',
            "printf '%s\\n' 'owner-walkthrough-original-log' | sudo tee /var/log/app.log >/dev/null\nsudo setfacl -m u:fabricolly:r /var/log/app.log\n",
            purpose='Create one exact original selected source line and grant documented narrow ACL')
        self.prose(self.edge, 'node', '4. `sudo systemctl enable',
            'sudo systemctl enable --now fabrico11y-node.service\n')
        self.fence(self.edge, 'node', 1)
        deadline = time.monotonic() + min(60, self.commands.remaining())
        while time.monotonic() < deadline:
            text = self.observed(self.edge, 'fabricctl inspect /etc/fabrico11y/node.conf\n',
                                 'edge-inspect-' + str(len(self.record['events'])))
            self.event('edge-backlog', output_sha256=hashlib.sha256(text.encode()).hexdigest())
            fields = dict(word.split('=', 1) for word in text.split() if '=' in word)
            if (int(fields['next_sequence']) > 1 and int(fields['acked_through']) == int(fields['next_sequence']) - 1
                    and fields['log_backlog_bytes'] == '0'): return
            time.sleep(.5)
        raise TimeoutError('native edge did not drain within registered readiness window')

    def browser(self, label, ca, leaf, *, wrong_san=False, untrusted=False):
        from console_bridge import ConsoleBridge
        origin = 'https://wrong.telemetry.example:7443' if wrong_san else ORIGIN
        work = self.root / ('browser-' + label)
        out = self.out / ('browser-' + label)
        instance = ConsoleBridge.__new__(ConsoleBridge)
        error = None
        try:
            instance.__init__(origin, leaf, work, out, browser_group=self.budget['path'],
                browser_parent=self.budget, browser_deadline=self.overall_end,
                backend='firefox', trusted_ca=ca, private_dns=origin.split('/')[2].split(':')[0],
                browser_entry_helper=self.browser_entry_helper,
                browser_entry_helper_sha256=self.browser_entry_helper_sha256)
        except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as failure:
            error = failure
            path = self.out / ('browser-' + label + '-constructor-error.json')
            private_write(path, json.dumps({'label': label, 'origin': origin,
                'type': type(failure).__name__, 'message': str(failure)}).encode())
            self.record.setdefault('browser_constructor_errors', []).append(
                dict(path=str(path), sha256=hashlib.sha256(path.read_bytes()).hexdigest()))
        if wrong_san or untrusted:
            try:
                receipt_path = out / 'receipt.json'
                if not receipt_path.is_file():
                    raise RuntimeError('candidate browser startup produced no TLS evidence: '
                                       + str(error or 'no constructor refusal')) from error
                receipt = json.loads(receipt_path.read_text())
                diagnostic = receipt.get('tls_navigation_failure', {})
                expected = 'SSL_ERROR_BAD_CERT_DOMAIN' if wrong_san else 'SEC_ERROR_UNKNOWN_ISSUER'
                require(error is not None and diagnostic.get('uri', '').startswith('about:certerror')
                        and diagnostic.get('codes') == [expected], 'specific actual candidate certificate refusal missing')
                self.event('candidate-browser-certificate-control', expected=expected, diagnostic=diagnostic)
            finally:
                if getattr(instance, 'receipt', None) is not None:
                    instance.shutdown_deadline = min(self.overall_end, time.monotonic() + 20)
                    instance.close()
            require('populated 0' in (self.budget['path'] / 'cgroup.events').read_text(), 'browser control domain remains populated')
            return
        require(error is None, 'actual trusted candidate browser startup failed')
        actual = instance._js('return {origin:location.origin,secure:isSecureContext}')
        require(actual == dict(origin=ORIGIN, secure=True), 'trusted actual candidate origin missing')
        self.event('candidate-browser-trusted-positive', observation=actual)
        return instance

    def state(self, stage):
        answer = {}
        for guest in (self.server, self.edge):
            text = self.observed(guest, r'''python3 - <<'OBS'
import hashlib,json,os,pwd,stat
from pathlib import Path
rows={}
for root in (Path('/etc/fabrico11y'),Path('/var/lib/fabrico11y')):
 for base,dirs,files in os.walk(root,followlinks=False):
  for name in [*dirs,*files]:
   p=Path(base)/name;s=p.lstat()
   if stat.S_ISLNK(s.st_mode):raise ValueError('state symlink not admitted')
   row={'mode':s.st_mode,'uid':s.st_uid,'gid':s.st_gid,'size':s.st_size if p.is_file() else None}
   if p.is_file():
    h=hashlib.sha256()
    with p.open('rb') as stream:
     for part in iter(lambda:stream.read(1024*1024),b''):h.update(part)
    row['sha256']=h.hexdigest()
   rows[str(p)]=row
account=pwd.getpwnam('fabricolly')
print(json.dumps({'files':rows,'account':{'uid':account.pw_uid,'gid':account.pw_gid,'shell':account.pw_shell}}))
OBS
''', stage + '-' + guest.role + '-state')
            answer[guest.role] = json.loads(text)
        return answer

    def services(self, start):
        for guest in (self.server, self.edge):
            service = 'fabrico11y-server.service' if guest.role == 'server' else 'fabrico11y-node.service'
            if start:
                self.prose(guest, 'upgrade-backup-and-removal', 'Explicitly start the roles configured on that host.',
                           'sudo systemctl start ' + service + '\n', purpose='Documented role-specific restart')
            else:
                self.fence(guest, 'upgrade-backup-and-removal', 0)

    def restart_login(self):
        status, _ = self.bridge._request('/v1/console/session')
        require(status == 401, 'pre-restart browser session remained authorized')
        self.record['stale_session_http'] = status
        self.bridge.login()
        self.event('actual-fresh-login', principal_id=self.bridge.principal_id)

    def execute(self):
        self.commands.phase('boot')
        self.overall_end = time.monotonic() + 1800
        Q.run([Q.executable('ssh-keygen'), '-q', '-t', 'ed25519', '-N', '', '-f', str(self.key)],
              timeout=min(30, self.commands.remaining()))
        self.server.start()
        self.edge.start()
        deadline = time.monotonic() + self.commands.remaining()
        self.server.ready(deadline=deadline)
        self.edge.ready(deadline=deadline)
        require(self.server.boot_id != self.edge.boot_id, 'two distinct fresh guest identities required')
        require(not self.previous_boot_ids.intersection((self.server.boot_id, self.edge.boot_id)),
                'reversed pair reused a previous guest boot identity')
        self.record['guests'] = {guest.role: dict(family=guest.family, boot_id=guest.boot_id, command=guest.command)
                                 for guest in (self.server, self.edge)}
        for guest in (self.server, self.edge):
            # These prepare the documented firewall/ACL tools, not Fabric or its state.
            command = ('dnf install -y firewalld acl; systemctl enable --now firewalld' if guest.family == 'fedora'
                       else 'apt-get update; apt-get install -y acl python3-venv')
            self.observed(guest, 'set -eu\n' + command + '\n', 'documented-prerequisites-' + guest.role, timeout=self.commands.remaining())
        self.commands.phase('install')
        for guest in (self.server, self.edge):
            self.install(guest, candidate=False)
            for name in ('server_dump', 'spool_dump'):
                guest.copy(self.artifacts['helpers'] / name, '/home/accept/' + name)
            self.observed(guest, 'install -d -m0700 /root/owner-observers\ninstall -m0700 /home/accept/server_dump /home/accept/spool_dump /root/owner-observers/\n'
                'systemctl is-active fabrico11y-server.service fabrico11y-node.service; test "$?" = 3\n',
                'initial-inactive-' + guest.role)
            self.processes(guest, 'initial-install', active=False)
        self.commands.phase('tls')
        ca, leaf = self.configure_server()
        if self.server.family == 'fedora':
            self.prose(self.server, 'network-and-certificate-setup', "192.0.2.0/24, Fedora's firewalld:",
                "sudo firewall-cmd --permanent --add-rich-rule='rule family=\"ipv4\" source address=\"192.0.2.0/24\" port port=\"7443\" protocol=\"tcp\" accept' && sudo firewall-cmd --reload\n")
        self.event('condition', condition='Debian ufw present', observed=False,
                   selected='documented Debian default has no firewall' if self.server.family == 'debian' else 'Fedora firewalld recipe')
        decoy = self.root / 'untrusted-ca.pem'
        Q.run(['openssl', 'req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:P-256', '-nodes',
               '-keyout', str(self.root / 'untrusted-ca.key'), '-out', str(decoy), '-days', '1', '-subj', '/CN=Owned unrelated trust control'],
              timeout=min(20, self.commands.remaining()))
        self.browser('wrong-san', ca, leaf, wrong_san=True)
        self.browser('untrusted-ca', decoy, leaf, untrusted=True)
        self.bridge = self.browser('trusted', ca, leaf)
        self.commands.phase('first-owner')
        self.principal = self.enroll_owner('fixture')
        self.event('condition', condition='first setup expired or interrupted before enrollment', observed=False,
                   selected='renew-bootstrap conditional fence skipped')
        self.commands.phase('edge')
        self.edge_token = self.node_enroll()
        self.configure_edge()
        self.reader_token = self.reader()
        require(self.reader_status(self.reader_token) == 200, 'fresh read-only token query failed')
        self.received('fixture')
        self.commands.phase('queries-revoke-restart')
        self.event('received-guide-queries', role='edge')
        self.commands.phase('backup-restore-recovery')
        for guest in (self.server, self.edge):
            self.fence(guest, 'upgrade-backup-and-removal', 0)
        self.snapshot('fixture')
        for guest in (self.server, self.edge):
            self.prose(guest, 'upgrade-backup-and-removal', 'Keep the archive in controlled storage;',
                'chmod 0700 /home/accept\nsudo install -m0600 /dev/null /home/accept/fabrico11y-backup.tar\n',
                purpose='Protect complete backup before the unchanged archive fence')
            self.fence(guest, 'upgrade-backup-and-removal', 1)
        saved = self.state('before-restore')
        for guest in (self.server, self.edge): self.fence(guest, 'upgrade-backup-and-removal', 2)
        require(self.state('after-restore') == saved, 'restored complete state differs before explicit recovery')
        # Exactly the guide's inline recovery fence, never the reference-only linked recipe.
        self.fence(self.server, 'upgrade-backup-and-removal', 3, principal=self.principal)
        self.prose(self.edge, 'upgrade-backup-and-removal', 'then enroll the owner again; on an edge, start',
                   'sudo systemctl start fabrico11y-node.service\n')
        self.record['stale_session_http'] = self.bridge._request('/v1/console/session')[0]
        require(self.record['stale_session_http'] == 401, 'restored stale session was authorized')
        self.browser_observers()
        self.bridge._js("const e=Array.from(document.querySelectorAll('button')).find(e=>e.textContent.trim()==='Check session');e.click();")
        self.ui_wait("document.body.textContent.includes('Sign in with a passkey')")
        self.field('Principal ID', self.principal)
        self.click('Use passkey')
        requests = self.bridge._js('return window.ownerPublicRequests')
        require(any(row == dict(route='/v1/console/auth/login/finish', status=401) for row in requests),
                'old resident owner key did not receive actual server refusal')
        self.record['old_owner_denied'] = True
        self.record['old_reader_http'] = self.reader_status(self.reader_token)
        require(self.record['old_reader_http'] == 401, 'restored old reader retained query authority')
        # Only after the old key's actual refusal do we replace its virtual device.
        self.bridge.replace_authenticator()
        require(self.enroll_owner('restored') == self.principal, 'recovery changed immutable principal')
        self.reader_token = self.reader()
        self.received('restored')
        self.commands.phase('upgrade')
        self.services(False)
        self.snapshot('restored')
        before = self.state('before-upgrade')
        self.event('call', document='docs/operations.md', section='install', return_to='upgrade')
        for guest in (self.server, self.edge): self.install(guest, candidate=True)
        self.event('return', destination='upgrade')
        require(self.state('after-upgrade') == before, 'native package upgrade changed account/configuration/retained state')
        self.record['upgrade_preserved'] = True
        self.services(True)
        self.restart_login()
        self.received('upgraded')
        self.commands.phase('queries-revoke-restart')
        self.click('Settings')
        self.field('Existing Spindle name', 'edge')
        self.bridge._js('window.confirm=()=>true')
        self.click('revoke')
        # Read-only authority probe from the actual edge, using protected stdin.
        probe = self.prose(self.edge, 'reader-tokens-and-spindle-controls', 'After revoking an edge, stop its',
            "python3 -c 'import json,ssl,sys,urllib.request,urllib.error; t=sys.stdin.read().strip(); r=urllib.request.Request(\"https://telemetry.example.com:7443/v1/config\",headers={\"Authorization\":\"Bearer \"+t});\ntry: urllib.request.urlopen(r,context=ssl.create_default_context(cafile=\"/home/operator/.config/fabrico11y/ca.pem\"),timeout=5)\nexcept urllib.error.HTTPError as e: print(e.code)'\n",
            stdin=self.edge_token.encode(), purpose='Observe specifically unauthorized revoked primary edge over verified TLS')
        require(Path(probe['stdout']['path']).read_text().strip() == '401', 'revoked primary edge did not receive HTTP401')
        self.record['revoked_edge_http'] = 401
        self.prose(self.edge, 'reader-tokens-and-spindle-controls', 'After revoking an edge, stop its',
                   'sudo systemctl stop fabrico11y-node.service\nsudo rm /etc/fabrico11y/node-token\n')
        self.commands.phase('remove-reset-reenroll')
        self.services(False)
        self.snapshot('upgraded')
        retained = self.state('before-remove')
        for guest in (self.server, self.edge):
            remove = 'sudo apt remove fabrico11y' if guest.family == 'debian' else 'sudo dnf remove fabrico11y'
            self.prose(guest, 'upgrade-backup-and-removal', 'To remove executables while retaining state,', remove + '\n', stdin=b'y\n')
        require(self.state('after-remove') == retained, 'native removal changed retained configuration/state/account')
        for guest in (self.server, self.edge):
            reset = 'sudo apt purge fabrico11y' if guest.family == 'debian' else 'sudo dnf remove fabrico11y && sudo rm -rf /etc/fabrico11y /var/lib/fabrico11y'
            self.prose(guest, 'upgrade-backup-and-removal', 'A deliberate reset destroys retained telemetry,', reset + '\n', stdin=b'y\n')
            self.observed(guest, 'test ! -e /etc/fabrico11y; test ! -e /var/lib/fabrico11y\n', 'empty-reset-' + guest.role)
            self.install(guest, candidate=True)
        self.configure_server(reuse=True)
        self.bridge.replace_authenticator()
        self.principal = self.enroll_owner('fresh-candidate')
        self.edge_token = self.node_enroll()
        self.configure_edge()
        self.reader_token = self.reader()
        self.received('fresh-candidate')
        self.commands.phase('archive-stop')
        self.services(False)
        self.snapshot('fresh-candidate')


def stop_owned(walk, budget):
    """Try every owned cleanup even when a preceding operation raises."""
    errors = []
    if walk is not None:
        if walk.bridge is not None:
            try:
                walk.bridge.shutdown_deadline = min(walk.overall_end, time.monotonic() + 20)
                walk.bridge.close()
                require(walk.bridge.supervisor.cleanup_confirmed, 'browser supervisor stop unconfirmed')
            except BaseException as error:
                errors.append('browser: ' + type(error).__name__ + ': ' + str(error))
        for guest in (walk.server, walk.edge):
            try:
                require(guest.stop(), 'guest stop unconfirmed')
            except BaseException as error:
                errors.append(guest.role + ': ' + type(error).__name__ + ': ' + str(error))
    if budget is not None:
        try:
            require('populated 0' in (budget['path'] / 'cgroup.events').read_text().splitlines(),
                    'browser domain remains populated')
        except BaseException as error:
            errors.append('browser budget: ' + type(error).__name__ + ': ' + str(error))
        try:
            os.close(budget['fd'])
        except BaseException as error:
            errors.append('budget descriptor: ' + type(error).__name__ + ': ' + str(error))
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--admission', type=Path, required=True)
    parser.add_argument('--admission-sha256', required=True)
    parser.add_argument('--outer-admission', type=Path, required=True)
    parser.add_argument('--run-id', required=True)
    parser.add_argument('--server-family', choices=('debian', 'fedora'), required=True)
    parser.add_argument('--browser-entry-helper', type=Path, required=True)
    parser.add_argument('--browser-entry-helper-sha256', required=True)
    placement_options(parser)
    args = parser.parse_args()
    validate_placement(args)
    require(args.browser_entry_helper.is_absolute()
            and not args.browser_entry_helper.is_symlink()
            and args.browser_entry_helper.is_file()
            and os.access(args.browser_entry_helper, os.X_OK)
            and re.fullmatch('[0-9a-f]{64}', args.browser_entry_helper_sha256)
            and sha(args.browser_entry_helper) == args.browser_entry_helper_sha256,
            'exact pinned native browser entry helper required')
    require(re.fullmatch('[a-z0-9][a-z0-9-]{0,79}', args.run_id), 'unique safe run identity required')
    require(re.fullmatch('[0-9a-f]{64}', args.admission_sha256), 'exact preregistered admission digest required')
    def interrupted(signum, frame):
        raise InterruptedError('owner walkthrough interrupted by signal ' + str(signum))
    signal.signal(signal.SIGTERM, interrupted)
    Q.require_storage()
    limits = Q.require_containment(split=bool(args.edge_host))
    service = Path('/sys/fs/cgroup') / limits['cgroup'].lstrip('/')
    parent = json.loads(args.outer_admission.read_text())
    require(args.outer_admission.resolve().parent == Q.DATA / 'results/service-opening-candidate'
            and parent['state'] == 'running' and parent['reserved_bytes'] >= RESERVATION
            and parent['admission_observation']['uid'] == parent['admission_observation']['gid'] == 0
            and parent['admission_bytes'] <= 81_041_356_288
            and shutil.disk_usage(Q.DATA).free >= RESERVATION,
            'strict complete namespace census and full pair reservation required')
    plan, artifacts, fixture, guides = admit(args.admission, args.admission_sha256)
    require_admitted_browser_entry_helper(
        plan, args.browser_entry_helper, args.browser_entry_helper_sha256)
    task_max = 1536 if args.edge_host else 4096
    Q.run(['systemctl', '--user', 'set-property', '--runtime', service.name,
           'CPUQuota=400%', 'TasksMax=' + str(task_max)], timeout=15)
    require((service / 'cpu.max').read_text().strip() == '400000 100000'
            and (service / 'pids.max').read_text().strip() == str(task_max),
            'actual host CPU/task enforcement missing')
    for family in ('debian', 'fedora'): image(family)
    previous_boot_ids = set()
    if plan.get('previous_pair'):
        previous = json.loads(artifact(plan['previous_pair']['path'], plan['previous_pair']['sha256']).read_text())
        require(previous['exit'] == 0 and previous['cleanup_confirmed'] is True
                and previous['guests']['server']['family'] != args.server_family,
                'first pair must finish and stop before fresh reversed pair')
        previous_boot_ids = {guest['boot_id'] for guest in previous['guests'].values()}
        require(len(previous_boot_ids) == 2 and all(previous_boot_ids), 'previous pair boot identities incomplete')
    root = Q.DATA / 'scratch' / ('owner-walkthrough-' + args.run_id)
    out = Q.DATA / 'results/owner-walkthrough' / args.run_id
    require(not root.exists() and not out.exists(), 'fresh guest/output paths required')
    out.mkdir(mode=0o700, parents=True)
    walk = None
    rc = 2
    budget = None
    setup_failure = None
    try:
        root.mkdir(mode=0o700)
        for guide in guides.values(): guide.archive(out)
        # Delegate before any VM/browser is spawned. The outer aggregate8GiB cap
        # remains authoritative; all children inherit an explicit CPU/task cap.
        import live_browser_check as browser
        require(browser.cgroups.delegate() == service, 'owned launcher delegation differs')
        browser.cgroups.set_limits(service / 'supervisor', (6 if args.edge_host else 8) * 1024**3,
            (5 if args.edge_host else 7) * 1024**3, task_max, 4)
        budget_path, budget_limits = browser.cgroups.subgroup(service, 'owner-browser-budget',
            1024**3, 850 * 1024**2, 512, 2)
        budget = browser.browser_hold_directory(budget_path)
        budget['exclusive'] = True
        walk = Walkthrough(plan, artifacts, fixture, guides, root, out, args.server_family,
                           args.browser_entry_helper, args.browser_entry_helper_sha256, placement=args)
        walk.budget = budget
        walk.previous_boot_ids = previous_boot_ids
        walk.record['admission_sha256'] = args.admission_sha256
        walk.record['host_cgroup'] = {**limits, 'cpu_max': (service / 'cpu.max').read_text().strip(),
                                      'pids_max': (service / 'pids.max').read_text().strip(), 'browser': budget_limits}
        walk.execute()
        rc = 0
    except BaseException as error:
        if walk is not None:
            walk.record['failure'] = {'type': type(error).__name__, 'reason': str(error)}
            walk.commands.failed = True
        else:
            setup_failure = {'exit': 2, 'type': type(error).__name__, 'reason': str(error)}
    finally:
        errors = stop_owned(walk, budget)
        clean = not errors
        if walk is not None:
            if args.edge_host:
                walk.record['remote_completion_confirmed'] = walk.edge.completion_confirmed
                if not walk.edge.completion_confirmed:
                    rc = 2
            walk.record['cleanup_confirmed'] = clean
            walk.record['cleanup_errors'] = errors
            walk.record['query_evidence'] = 'protected-transcript'
            if rc == 0 and clean:
                try:
                    walk.record['phases'] = walk.commands.finish()
                    verdict = completed(walk.record, walk.expected_binding, dict(PHASES))
                    walk.record.update(verdict, verdict_requires_outer_stop=True)
                except BaseException as error:
                    rc = 2
                    walk.record['completion_error'] = type(error).__name__ + ': ' + str(error)
            if rc != 0 or not clean:
                rc = 2
                walk.record.update(exit=2, scratch_retained=str(root))
            else:
                try:
                    shutil.rmtree(root)
                    walk.record['scratch_removed'] = not root.exists()
                except BaseException as error:
                    rc = 2
                    walk.record.update(exit=2, scratch_retained=str(root),
                                       scratch_cleanup_error=type(error).__name__ + ': ' + str(error))
            private_write(out / 'receipt.json', (json.dumps(walk.record, indent=2) + '\n').encode())
        else:
            private_write(out / 'receipt.json', json.dumps({'exit': 2, 'failure': setup_failure,
                'cleanup_confirmed': clean, 'cleanup_errors': errors, 'scratch_retained': str(root),
                'admission_sha256': args.admission_sha256}).encode())
    return rc


if __name__ == '__main__':
    raise SystemExit(main())
