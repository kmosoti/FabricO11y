#!/usr/bin/env python3
"""Qualification-only guest capture; original installed paths remain authority."""
import hashlib
import fcntl
import struct
import json
import os
from pathlib import Path
import pwd
import re
import stat
import subprocess
import sys
import tarfile
import time
import zlib

import custody_witness as witness

ROOT = Path('/var/lib/fabrico11y/node/spool')
SOURCE = '/var/log/fabric-accept/allowed.log'
DIAG = str(ROOT / 'diagnostics/spindle.log')
FIXTURE = Path('/var/lib/fabric-custody-fixture')
UNITS = witness.UNITS
PENDING = 'accept-upgrade-pending-literal'
UNREAD = 'accept-upgrade-unread-literal'
NATIVE_PROFILE = dict(revision=1, paused=False, logs=[SOURCE, '/var/log/fabric-accept/denied.log'],
                      metric_interval_s=15)
sha = lambda raw: hashlib.sha256(raw).hexdigest()


def packed(value):
    if isinstance(value, bytes):
        return {'hex_bytes': value.hex()}
    if isinstance(value, dict):
        return {key: packed(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [packed(item) for item in value]
    return value


class Guest:
    def __init__(self, config, phase):
        witness.require(os.geteuid() == 0 and Path('/run/cloud-init').is_dir(), 'disposable guest root required')
        self.config, self.phase, self.commands = config, phase, []
        self.started = time.monotonic()
        self.deadline = float(os.environ['FABRIC_NATIVE_PHASE_DEADLINE'])
        witness.require(config['fixture_paths'] == [SOURCE] and config['spool_root'] == str(ROOT)
            and Path('/var/lib/cloud/data/instance-id').read_text().strip() == 'fabric-install-' + config['binding']['run_id'],
            'guest/run/configured-root admission')
        boot = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
        witness.require((phase == 'reboot') == (boot != config['boot_before']), 'guest boot/phase admission')
        self.base = FIXTURE / config['binding']['run_id']
        if phase == 'prepare':
            FIXTURE.mkdir(mode=0o700, exist_ok=True)
            self.base.mkdir(mode=0o700)
        witness.require(self.base.resolve() == self.base and self.base.stat().st_uid == 0
                        and stat.S_IMODE(self.base.stat().st_mode) == 0o700, 'unsafe guest output')
        self.output = self.base / phase
        self.output.mkdir(mode=0o700)
        os.umask(0o077)
        os.environ['FABRIC_SCRATCH_ROOT'] = '/var/lib'

    def save(self, name, value):
        (self.output / name).write_text(json.dumps(packed(value), sort_keys=True) + '\n')

    def command(self, argv, check=True):
        remaining = self.deadline - time.monotonic()
        witness.require(remaining > 0, 'native phase deadline')
        started = time.monotonic()
        result = subprocess.run(argv, capture_output=True, text=True, timeout=min(30, remaining))
        record = dict(argv=argv, exit=result.returncode, stdout=result.stdout, stderr=result.stderr, elapsed_seconds=time.monotonic()-started)
        self.commands.append(record)
        self.save('commands.json', self.commands)
        witness.require(not check or result.returncode == 0, 'observer command failed: ' + argv[0])
        return record

    def observe_stop(self):
        account = self.command(['getent', 'passwd', 'fabricolly'])['stdout']
        group = self.command(['getent', 'group', 'fabricolly'])['stdout']
        user = pwd.getpwnam('fabricolly')
        inventory = self.command(['ps', '-e', '-o', 'pid=,ppid=,uid=,gid=,comm=,args='])
        processes = []
        for directory in Path('/proc').glob('[0-9]*'):
            try:
                executable = os.readlink(directory / 'exe')
            except FileNotFoundError:
                continue
            except PermissionError as error:
                raise ValueError('unreadable process inventory') from error
            try:
                descriptors = {}
                for descriptor in (directory / 'fd').iterdir():
                    try:
                        target = os.readlink(descriptor)
                    except FileNotFoundError:
                        continue
                    if target.startswith('/var/lib/fabrico11y/'):
                        descriptors[descriptor.name] = target
                if descriptors or Path(executable.removesuffix(' (deleted)')).name in ('fabric-node', 'fabric-server'):
                    processes.append(dict(pid=int(directory.name), exe=executable, descriptors=descriptors,
                        stat=(directory / 'stat').read_text(), cgroup=(directory / 'cgroup').read_text()))
            except FileNotFoundError:
                continue
        self.save('processes.json', dict(ps=inventory, proc=processes))
        units = {}
        for name in UNITS:
            active = self.command(['systemctl', 'is-active', name], check=False)
            show = self.command(['systemctl', 'show', name, '-p', 'MainPID', '-p', 'ControlGroup', '-p', 'ActiveState'])
            values = dict(line.split('=', 1) for line in show['stdout'].splitlines())
            expected = '/system.slice/system-fabrico11y.slice/' + name
            witness.require(values['ControlGroup'] in ('', expected), 'unexpected service cgroup')
            group_path = Path('/sys/fs/cgroup' + expected)
            if group_path.exists():
                self.command(['find', str(group_path), '-name', 'cgroup.procs', '-print', '-exec', 'cat', '{}', '+'])
            members = {str(path): path.read_text() for path in group_path.rglob('cgroup.procs')}
            self.save(name + '.json', dict(active=active, show=show, cgroup=expected,
                directory_exists=Path('/sys/fs/cgroup' + expected).exists(), members=members))
            witness.require(active['exit'] == 3 and active['stdout'].strip() in ('inactive', 'failed')
                and values['ActiveState'] == active['stdout'].strip() and values['MainPID'] == '0'
                and all(not raw.strip() for raw in members.values()), 'stop observation: live or unreadable writer')
            units[name] = dict(active_state=values['ActiveState'], query_exit=active['exit'], main_pid=0,
                               cgroup_empty=True, companions=[])
        listed = self.command(['systemctl', 'list-units', '--all', '--plain', '--no-legend', 'fabrico11y*'])
        extras = [line for line in listed['stdout'].splitlines() if line.split()[0].endswith('.service')
                  and line.split()[0] not in UNITS]
        self.save('processes.json', dict(ps=inventory, proc=processes, extra_units=extras))
        witness.require(not any(line.split(None, 5)[4] in ('fabric-node', 'fabric-server')
            for line in inventory['stdout'].splitlines() if len(line.split(None, 5)) >= 5) and not processes and not extras, 'stop observation: live or unexpected project writer')
        return dict(spool_root=str(ROOT), spool_bytes=268435456, stop_confirmed=True,
            boot_id=Path('/proc/sys/kernel/random/boot_id').read_text().strip(), binding=self.config['binding'],
            extra_project_units=[], units=units, service_identity=dict(uid=user.pw_uid, gid=user.pw_gid,
                                                                     passwd=account, group=group))

    def read_input(self, path, limit):
        original = Path(path)
        before = witness.identity(original.lstat())
        witness.require(original.resolve(strict=True) == original and stat.S_ISREG(before[2])
            and before[5] == 1 and before[6] <= limit, 'unsafe original input')
        with os.fdopen(os.open(original, os.O_RDONLY | os.O_NOFOLLOW), 'rb') as stream:
            filesystem = self.command(['stat', '-f', '-c', '%t', path])['stdout'].strip()
            stable = None
            # Linux UAPI linux/btrfs.h: FS_INFO1024, INO_LOOKUP4096, root objectid256.
            if filesystem == '9123683e':
                fs, lookup = bytearray(1024), bytearray(struct.pack('<QQ', 0, 256) + bytes(4080))
                fcntl.ioctl(stream.fileno(), 0x8400941f, fs, True)
                fcntl.ioctl(stream.fileno(), 0xd0009412, lookup, True)
                stable = dict(uuid=bytes(fs[16:32]).hex(), subvolume_id=struct.unpack_from('<Q', lookup)[0])
                witness.require(any(fs[16:32]) and stable['subvolume_id'] > 0, 'invalid original Btrfs namespace')
            raw = stream.read(limit + 1)
            witness.require(before == witness.identity(os.fstat(stream.fileno()))
                == witness.identity(original.lstat()) and len(raw) == before[6], 'original input drift')
        return raw, before, stable

    def capture(self, name, process=None):
        receipt = self.observe_stop()  # Refuse before reading any source or raw root.
        settings = dict(line.split('=', 1) for line in Path('/etc/fabrico11y/node.conf').read_text().splitlines()
                        if line and not line.startswith('#') and '=' in line)
        witness.require(settings.get('spool_dir') == str(ROOT) and settings.get('spool_bytes') == '268435456'
            and settings.get('token_file') == '/etc/fabrico11y/node-token'
            and [line[4:] for line in Path('/etc/fabrico11y/node.conf').read_text().splitlines() if line.startswith('log=')] == [SOURCE, '/var/log/fabric-accept/denied.log'],
                        'configured root/cap mismatch')
        sources = {}
        for index, path in enumerate((SOURCE, DIAG)):
            raw, before, stable = self.read_input(path, witness.MAX_BATCH)
            archive = self.output / (name + '-source-' + str(index))
            archive.write_bytes(raw)
            sources[path] = dict(archive_path=str(archive), device=before[0], inode=before[1],
                                 original_stat=before, sha256=sha(raw), btrfs_identity=stable)
        receipt['candidate_process'] = process
        receipt['root_stat'] = witness.identity(ROOT.lstat())
        with tarfile.open(self.output / (name + '-raw.tar'), 'x') as archive:
            archive.add(ROOT, arcname='spool', recursive=True)
        snapshot = witness.stopped_snapshot(ROOT, receipt, sources, fixture_paths=(SOURCE,))
        witness.require(snapshot['applied_configuration'] is not None and json.loads(snapshot['applied_configuration'])
            == NATIVE_PROFILE,
            'registered native applied configuration missing or changed')
        self.save(name + '-snapshot.json', snapshot)
        self.save(name + '-receipt.json', receipt)
        self.save(name + '-sources.json', sources)
        # Compare every leaf after the complete tree export, retaining original stats.
        witness.require(snapshot == witness.stopped_snapshot(ROOT, receipt, sources, fixture_paths=(SOURCE,)),
                        'raw root drift during archive')
        credentials = {}
        for path in ('/etc/fabrico11y/node-token', '/root/accept/node-token'):
            raw, info, stable = self.read_input(path, 4096)
            expected = (0o640, 0, pwd.getpwnam('fabricolly').pw_gid) if path.startswith('/etc/') else (0o600, 0, 0)
            witness.require((stat.S_IMODE(info[2]), info[3], info[4]) == expected, 'credential ownership/mode')
            credentials[path] = dict(stat=info, sha256=sha(raw), trimmed_sha256=sha(raw.strip()), btrfs_identity=stable)
        control = json.loads(Path('/var/lib/fabrico11y/server/control.json').read_text())
        enrolled = [node for node in control['nodes'] if node['name'] == 'spindle-1']
        witness.require(len(enrolled) == 1 and enrolled[0]['token_sha256'] == credentials['/etc/fabrico11y/node-token']['trimmed_sha256']
            == credentials['/root/accept/node-token']['trimmed_sha256'], 'credential/enrollment join')
        self.save(name + '-credential.json', dict(files=credentials, enrollment=enrolled[0], node_id=snapshot['node_id']))
        return snapshot

    def query(self, name):
        pages, rows, tokens = [], [], set()
        query = dict(kind='logs', node='spindle-1', from_ns=0, to_ns=9223372036854775807, limit=1000)
        for index in range(100):
            result = self.command(['fabricctl', 'admin', '/root/accept/admin.conf', 'query', json.dumps(query)])
            (self.output / (name + '-query-' + str(len(self.commands)) + '-page-' + str(index) + '.json')).write_text(result['stdout'])
            page = json.loads(result['stdout']); pages.append(page); rows.extend(page['rows'])
            token = page['next_page']
            if token is None:
                return rows
            witness.require(isinstance(token, str) and token and token not in tokens, 'query pagination cycle')
            tokens.add(token); query['page'] = token
        raise ValueError('query page ceiling')

    def process(self):
        show = self.command(['systemctl', 'show', UNITS[1], '-p', 'MainPID', '-p', 'ControlGroup', '-p', 'InvocationID'])
        values = dict(line.split('=', 1) for line in show['stdout'].splitlines())
        pid = int(values['MainPID']); proc = Path('/proc') / str(pid)
        info = (proc / 'stat').read_text(); uid = (proc / 'status').read_text()
        now, uptime, ticks = time.time_ns(), time.clock_gettime_ns(time.CLOCK_BOOTTIME), os.sysconf('SC_CLK_TCK')
        start_ticks = int(info.rsplit(')', 1)[1].split()[19])
        start_ns = max(0, now - uptime + start_ticks * 1000000000 // ticks - 1000000000)
        executable = self.command(['sha256sum', str(proc / 'exe')])['stdout'].split()[0]
        witness.require(executable == self.config['node_sha256'] and pid > 0
            and re.fullmatch('[0-9a-f]{32}', values['InvocationID']) and values['InvocationID'] != '0' * 32
            and values['ControlGroup'] == '/system.slice/system-fabrico11y.slice/' + UNITS[1]
            and (proc / 'cgroup').read_text().strip() == '0::' + values['ControlGroup']
            and all(int(value) == pwd.getpwnam('fabricolly').pw_uid for value in re.search(r'^Uid:\s+(.+)$', uid, re.M)[1].split())
            and all(int(value) == pwd.getpwnam('fabricolly').pw_gid for value in re.search(r'^Gid:\s+(.+)$', uid, re.M)[1].split()),
            'candidate executable/service identity')
        return dict(**self.config['binding'], unit=UNITS[1], pid=pid, boot_id=Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
                    start_ns=start_ns, end_ns=0, proc_stat=info, status=uid, exe_sha256=executable, show=show, invocation_id=values['InvocationID'],
                    calibration=dict(realtime_ns=now, boottime_ns=uptime, clock_ticks=ticks, start_ticks=start_ticks))

    def finish(self):
        files = {path.name: dict(sha256=sha(path.read_bytes()), stat=witness.identity(path.lstat()))
                 for path in self.output.iterdir() if path.is_file()}
        self.save('manifest.json', dict(binding=self.config['binding'], phase=self.phase, files=files, elapsed_seconds=time.monotonic()-self.started))
        witness.require(sum(path.stat().st_size for path in self.base.rglob('*') if path.is_file()) <= 512 * 1024**2,
                        'native checkpoint overhead exceeds half-GiB transfer budget')


# Caller: fixed lifecycle entry/exit, with no copied-root reader fallback.
def unpack(value):
    if isinstance(value, dict):
        return bytes.fromhex(value['hex_bytes']) if set(value) == {'hex_bytes'} else {key: unpack(item) for key, item in value.items()}
    return [unpack(item) for item in value] if isinstance(value, list) else value


def guest_main(config, phase):
    guest = Guest(config, phase)
    try:
        if phase == 'prepare':
            guest.command(['systemctl', 'stop', UNITS[1]])
            baseline = guest.query('before')
            guest.save('baseline.json', baseline)
            guest.command(['systemctl', 'stop', UNITS[0]])
            with open(SOURCE, 'a') as stream:
                stream.write(PENDING + '\n')
            if config.get('control') != 'pending-not-produced':
                guest.command(['systemctl', 'start', UNITS[1]])
                time.sleep(min(20, max(0, guest.deadline - time.monotonic())))
            if config.get('control') != 'live-writer':
                guest.command(['systemctl', 'stop', UNITS[1]])
            with open(SOURCE, 'a') as stream:
                stream.write(UNREAD + '\n')
            before = guest.capture('before')
            witness.require(any(row['body'] == PENDING and row['attributes'].get('log.file.path') == SOURCE
                for source in before['sources'] if source['sequence'] > before['ack'] for row in source['rows']),
                'missing-committed-pending')
        elif phase == 'installed':
            guest.capture('installed')
        elif phase in ('drain', 'reboot'):
            if phase == 'drain':
                guest.command(['systemctl', 'start', *UNITS])
            process = guest.process()
            # Stop collection first, then drain the fixed committed prefix during normal stop.
            deadline = min(guest.deadline - 40, time.monotonic() + 40)
            while True:
                rows = guest.query('readiness')
                if any(row['body'] == UNREAD and row['attributes'].get('log.file.path') == SOURCE for row in rows):
                    break
                witness.require(time.monotonic() < deadline, 'native drain deadline')
                time.sleep(1)
            current = guest.process()
            witness.require(current['pid'] == process['pid'] and current['invocation_id'] == process['invocation_id'] and current['proc_stat'].rsplit(')', 1)[1].split()[19]
                == process['proc_stat'].rsplit(')', 1)[1].split()[19], 'candidate process changed before stop')
            guest.command(['systemctl', 'stop', UNITS[1]])
            rows = guest.query('after_' + phase)
            guest.save('rows.json', rows)
            process['end_ns'] = time.time_ns()
            journal = guest.command(['journalctl', '--no-pager', '-q', '-o', 'json', '-b', '-u', UNITS[1]])
            (guest.output / 'journal.jsonl').write_text(journal['stdout'])
            events = observed_events(journal['stdout'], process)
            guest.save('ack-events.json', events)
            guest.command(['systemctl', 'stop', UNITS[0]])
            snapshot = guest.capture(phase, process)
            if phase == 'reboot':
                read = lambda phase, name: unpack(json.loads((guest.base / phase / name).read_text()))
                before, installed, drain = (read(phase, name + '-snapshot.json') for phase, name in
                    (('prepare', 'before'), ('installed', 'installed'), ('drain', 'drain')))
                for value in (before, installed, drain, snapshot): value['fixture_paths'] = tuple(value['fixture_paths'])
                witness.conserve(before, installed, drain, dict(before=read('prepare', 'baseline.json'),
                    after_drain=read('drain', 'rows.json'), after_reboot=rows, pending_body=PENDING, unread_body=UNREAD,
                    boot_before=before['boot_id'], boot_after=snapshot['boot_id']), read('drain', 'ack-events.json'),
                    after_reboot=snapshot, reboot_ack_events=events)
                guest.save('conservation.json', dict(status='accepted', required_checkpoints=4))
            guest.command(['systemctl', 'start', *(UNITS if phase == 'reboot' else (UNITS[0],))])
        else:
            raise ValueError('unknown native phase')
    except Exception as error:
        guest.save('failure.json', dict(error=type(error).__name__, message=str(error)))
        raise
    finally:
        guest.finish()


def observed_events(raw, process):
    events = []
    for event in map(json.loads, raw.splitlines()):
        if event.get('_PID') != str(process['pid']): continue
        if event.get('MESSAGE', '').startswith('delivery sequence='):
            witness.require(event.get('_SYSTEMD_UNIT') == process['unit'] and event.get('_EXE') == '/usr/bin/fabric-node'
                and event.get('_SYSTEMD_INVOCATION_ID') == process['invocation_id']
                and event.get('_SYSTEMD_CGROUP') == '/system.slice/system-fabrico11y.slice/' + process['unit']
                and event.get('_UID') == re.search(r'^Uid:\s+(\d+)', process['status'], re.M)[1]
                and event.get('_GID') == re.search(r'^Gid:\s+(\d+)', process['status'], re.M)[1], 'raw journal sender mismatch')
        events.append(dict(unit=event.get('_SYSTEMD_UNIT'), pid=int(event['_PID']),
            boot_id=str(__import__('uuid').UUID(event['_BOOT_ID'])), at_ns=int(event['__REALTIME_TIMESTAMP']) * 1000,
            message=event.get('MESSAGE', '')))
    return events


# Host admission freezes owner inputs before boot and rechecks them at transfer.
def host_admission(owner_path, source_root, storage, package, source_commit, run_id, family):
    owner = json.loads(owner_path.read_text())
    git = lambda *args: subprocess.check_output(['git', '-C', str(source_root), *args], text=True).strip()
    witness.require(not git('status', '--porcelain') and git('rev-parse', 'HEAD') == owner['harness_commit'], 'clean harness source mismatch')
    subprocess.run(['git', '-C', str(source_root), 'merge-base', '--is-ancestor', owner['harness_commit'], 'origin/main'], check=True)
    names = ('custody_native.py', 'custody_witness.py', 'lifecycle.sh', 'run-qemu.py', 'acceptance.sh')
    paths = [source_root / 'tools/qualification/install' / name for name in names] + [source_root / 'tools/qualification/query_oracle.py']
    source_hashes = {str(path.relative_to(source_root)): sha(path.read_bytes()) for path in paths}
    witness.require(source_hashes == owner['source_hashes'], 'producer/helper source mismatch')
    debdir = storage / 'results/installation-package-build/replacement-tls-01-deb-a'
    rpmdir = storage / 'results/packages/replacement-tls-01-rpm-a'
    fixed = {debdir / 'receipt.json': 'f2a06e16fd8f82e27517eee12a1d8176d052765ac2e75959a85f70cdaeb6fd85',
             rpmdir / 'receipt.json': '4a2a800e8351ec8480359d0912f05d36682d27834c11c60bae87752961a5326a',
             storage / 'results/candidate-build-replacement-tls-01/candidate-admission.json':
             'bf27a2576bba8c32c7e27f9e434f27822c37aba60bbda387587723f2a1f36eb2'}
    for path, expected in fixed.items(): witness.require(sha(path.read_bytes()) == expected, 'owner package/admission receipt mismatch')
    deb, rpm = (json.loads((directory / 'receipt.json').read_text()) for directory in (debdir, rpmdir))
    frozen, canonical = 'e078ba23b543126011a2136dd1f7e98ca65370eb', '28bcdf7ac833ab634d3b130b85cb2a0aa47eb7a8'
    witness.require(frozen == deb['source']['original_head'] and source_commit == deb['source_commit'] == canonical
        == rpm['source']['source_commit'] and rpm['source'] == deb['source'], 'candidate frozen/source mismatch')
    for receipt in (deb, rpm):
        path = Path(receipt['package'])
        witness.require(path.resolve() == path and path.is_relative_to(storage / 'results')
            and sha(path.read_bytes()) == receipt['package_sha256'], 'canonical package mismatch')
        fixed[path] = receipt['package_sha256']
    expected = deb if family == 'debian' else rpm
    witness.require(package == Path(expected['package']), 'noncanonical candidate path')
    helper = debdir / 'qualification-helpers/manifest.json'
    manifest = json.loads(helper.read_text())
    witness.require(manifest == deb['qualification_helpers'] and manifest['source'] == deb['source'], 'canonical helper manifest mismatch')
    fixed[helper] = sha(helper.read_bytes())
    for name, expected_hash in manifest['files'].items():
        path = helper.parent / name
        witness.require(name in ('spool_dump', 'server_dump', 'spindle_sim') and sha(path.read_bytes()) == expected_hash, 'canonical helper binary mismatch')
        fixed[path] = expected_hash
    for path, expected_hash in ((debdir / 'provenance/source-manifest.json', deb['source']['manifest_sha256']),
        (debdir / 'provenance/frozen-source.bundle', deb['source']['source_bundle_sha256']),
        (storage / 'results/console-replacement-tls-01-a/build.json', deb['console_receipt_sha256'])):
        witness.require(sha(path.read_bytes()) == expected_hash, 'canonical source/console mismatch')
        fixed[path] = expected_hash
    return dict(binding=dict(run_id=run_id, candidate_frozen=frozen, candidate_package_sha256=expected['package_sha256'],
        package_source_commit=canonical, harness_commit=owner['harness_commit']), source_hashes=source_hashes,
        owner_sha256=sha(owner_path.read_bytes()), inputs={str(path): value for path, value in fixed.items()},
        node_sha256=rpm['payload_identity']['usr/bin/fabric-node']['sha256'], fixture_paths=[SOURCE], spool_root=str(ROOT))


def admit_transfer(bundle, admission, guest_sha256, *, control=None):
    """Inspect the labelled archive; never substitute an extracted root for guest input."""
    witness.require(bundle.stat().st_size <= 1024**3, 'native transfer overhead exceeded')
    witness.require(sha(bundle.read_bytes()) == guest_sha256, 'guest/host transfer digest mismatch')
    expected_phases = {'prepare'} if control else {'prepare', 'installed', 'drain', 'reboot'}
    with tarfile.open(bundle, 'r:') as archive:
        entries = archive.getmembers()
        witness.require(len(entries) <= 10000 and len({item.name for item in entries}) == len(entries), 'transfer path inventory')
        files = {}
        for item in entries:
            path = Path(item.name)
            witness.require(not path.is_absolute() and '..' not in path.parts and item.uid == item.gid == 0
                and ((item.isdir() and item.mode == 0o700 and len(path.parts) == 1 and path.name in expected_phases) or (item.isfile() and item.mode == 0o600)), 'transfer path/ownership')
            if item.isfile():
                witness.require(len(path.parts) == 2 and path.parts[0] in expected_phases, 'transfer phase/path')
                stream = archive.extractfile(item)
                files[item.name] = dict(sha256=sha(stream.read()), item=item)
        read = lambda name: json.load(archive.extractfile(files[name]['item']))
        witness.require({Path(name).parts[0] for name in files} == expected_phases, 'incomplete stopped phases')
        snapshots = {}
        for phase in expected_phases:
            manifest = read(phase + '/manifest.json')
            witness.require(manifest['phase'] == phase and manifest['binding'] == admission['binding'], 'producer binding mismatch')
            actual = {Path(name).name: value['sha256'] for name, value in files.items()
                      if name.startswith(phase + '/') and not name.endswith('/manifest.json')}
            witness.require(actual == {name: value['sha256'] for name, value in manifest['files'].items()}, 'raw transfer digest mismatch')
            witness.require(all(value['stat'][3:5] == [0, 0] and stat.S_IMODE(value['stat'][2]) == 0o600
                for value in manifest['files'].values()), 'producer file ownership mismatch')
            if phase == 'prepare' and control:
                failure = read('prepare/failure.json')
                expected_error = 'missing-committed-pending' if control == 'pending-not-produced' else 'stop observation: live or unreadable writer'
                witness.require(failure['error'] == 'ValueError' and failure['message'] == expected_error, 'wrong producer refusal')
                witness.require(('prepare/before-snapshot.json' in files) == (control == 'pending-not-produced'), 'producer refusal boundary')
                continue
            name = 'before' if phase == 'prepare' else phase
            witness.require(all(phase + '/' + leaf in files for leaf in ('processes.json', *(unit + '.json' for unit in UNITS))), 'incomplete stopped observations')
            receipt = read(phase + '/' + name + '-receipt.json')
            snapshot = unpack(read(phase + '/' + name + '-snapshot.json'))
            snapshot['fixture_paths'] = tuple(snapshot['fixture_paths'])
            snapshots[phase] = snapshot
            witness.require('applied-config.json' in snapshot['file_observations'], 'native applied configuration absent')
            witness.require((phase == 'reboot' or receipt['boot_id'] == admission['boot_before'])
                and receipt['spool_root'] == str(ROOT) and snapshot['fixture_paths'] == (SOURCE,)
                and snapshot['binding'] == admission['binding'] and snapshot['boot_id'] == receipt['boot_id'], 'native root/binding mismatch')
            with tarfile.open(fileobj=archive.extractfile(files[phase + '/' + name + '-raw.tar']['item'])) as raw_archive:
                raw_entries = raw_archive.getmembers()
                witness.require(len({item.name for item in raw_entries}) == len(raw_entries)
                    and {item.name for item in raw_entries} == {'spool'} | {'spool/' + path for path in snapshot['file_observations']},
                    'incomplete original raw tree')
                raw_files = {}
                for item in raw_entries:
                    if item.name == 'spool':
                        witness.require(item.isdir() and (item.uid, item.gid, item.mode)
                            == (receipt['root_stat'][3], receipt['root_stat'][4], stat.S_IMODE(receipt['root_stat'][2])), 'original root metadata mismatch')
                        continue
                    relative = item.name.removeprefix('spool/')
                    info = snapshot['file_observations'][relative]
                    witness.require((item.uid, item.gid, item.mode) == (info[3], info[4], stat.S_IMODE(info[2]))
                        and (item.isdir() if stat.S_ISDIR(info[2]) else item.isfile()), 'original raw ownership/type mismatch')
                    if item.isfile():
                        content = raw_archive.extractfile(item).read()
                        witness.require(sha(content) == snapshot['files'][relative]['sha256'], 'original raw bytes mismatch')
                        if relative == 'applied-config.json':
                            witness.applied_configuration(content, info, receipt['service_identity'])
                            witness.require(content == snapshot.get('applied_configuration')
                                and json.loads(content) == NATIVE_PROFILE, 'native applied configuration profile mismatch')
                        if relative.endswith('.faj'): raw_files[relative] = content
            payloads = []
            for filename in sorted(name for name in raw_files if name.startswith('sealed-')) + ['batches.faj']:
                raw, offset = raw_files[filename], 0
                while offset < len(raw):
                    witness.require(len(raw) - offset >= 32 and raw[offset:offset + 4] == b'FAB1', 'raw archive frame header')
                    size, head_crc, payload_crc = struct.unpack_from('<III', raw, offset + 4)
                    end = offset + 16 + size
                    witness.require(0 < size <= witness.MAX_BATCH and end + 16 <= len(raw)
                        and zlib.crc32(raw[offset:offset + 8]) == head_crc, 'raw archive frame size/CRC')
                    payload, marker = raw[offset + 16:end], raw[end:end + 16]
                    witness.require(zlib.crc32(payload) == payload_crc and marker[:4] == b'FAC1'
                        and struct.unpack_from('<Q', marker, 4)[0] == end and zlib.crc32(marker[:12]) == struct.unpack_from('<I', marker, 12)[0], 'raw archive commit CRC')
                    payloads.append(payload); offset = end + 16
            witness.require(payloads == [source['payload'] for source in snapshot['sources']], 'raw archive/source payload mismatch')
            sources = read(phase + '/' + name + '-sources.json')
            witness.require(set(sources) == {SOURCE, DIAG}, 'original source archive inventory')
            for index, path in enumerate((SOURCE, DIAG)):
                entry, log = sources[path], snapshot['logs'][path]
                leaf = name + '-source-' + str(index)
                witness.require(entry['archive_path'] == str(FIXTURE / admission['binding']['run_id'] / phase / leaf)
                    and entry['sha256'] == files[phase + '/' + leaf]['sha256'] == log['sha256']
                    and all(entry[key] == log[key] for key in ('device', 'inode', 'btrfs_identity'))
                    and entry['original_stat'][:2] == [entry['device'], entry['inode']], 'original source archive join')
            if phase in ('drain', 'reboot'):
                process = snapshot['candidate_process']
                shown = dict(line.split('=', 1) for line in process['show']['stdout'].splitlines())
                witness.require(process['exe_sha256'] == admission['node_sha256']
                    and process['pid'] == int(shown['MainPID']) and process['invocation_id'] == shown['InvocationID'], 'candidate process executable/incarnation mismatch')
                raw_journal = archive.extractfile(files[phase + '/journal.jsonl']['item']).read().decode()
                witness.require(read(phase + '/ack-events.json') == observed_events(raw_journal, process), 'raw journal/ACK observation mismatch')
            for source in snapshot['sources']:
                batch, _, rows = witness.envelope(source['payload'])
                witness.require(source['sha256'] == sha(source['payload']) and source['rows'] == rows
                    and source['sequence'] == batch['sequence'] and batch['node_id'].hex() == snapshot['node_id'], 'raw payload identity mismatch')
            inventory = read(phase + '/processes.json')
            witness.require(not inventory['proc'] and not inventory['extra_units']
                and not any(line.split(None, 5)[4] in ('fabric-node', 'fabric-server')
                    for line in inventory['ps']['stdout'].splitlines() if len(line.split(None, 5)) >= 5) and receipt['binding'] == admission['binding'], 'incomplete stopped observations')
            for unit in UNITS:
                observed = read(phase + '/' + unit + '.json')
                values = dict(line.split('=', 1) for line in observed['show']['stdout'].splitlines())
                witness.require(observed['active']['exit'] == 3 and observed['show']['exit'] == 0
                    and values['MainPID'] == '0' and values['ActiveState'] in ('inactive', 'failed')
                    and not any(value.strip() for value in observed['members'].values()), 'unconfirmed native stop')
        if not control:
            witness.require(read('reboot/conservation.json') == dict(status='accepted', required_checkpoints=4), 'native conservation absent')
            def query_rows(phase, prefix):
                names = sorted((name for name in files if name.startswith(phase + '/' + prefix + '-query-')),
                               key=lambda name: int(name.rsplit('-page-', 1)[1].split('.')[0]))
                witness.require(names and read(names[-1])['next_page'] is None, 'incomplete raw query pages')
                return [row for name in names for row in read(name)['rows']]
            witness.conserve(snapshots['prepare'], snapshots['installed'], snapshots['drain'],
                dict(before=query_rows('prepare', 'before'), after_drain=query_rows('drain', 'after_drain'),
                    after_reboot=query_rows('reboot', 'after_reboot'), pending_body=PENDING, unread_body=UNREAD,
                    boot_before=snapshots['prepare']['boot_id'], boot_after=snapshots['reboot']['boot_id']),
                read('drain/ack-events.json'), after_reboot=snapshots['reboot'], reboot_ack_events=read('reboot/ack-events.json'))
            credentials = [read(phase + '/' + ('before' if phase == 'prepare' else phase) + '-credential.json') for phase in ('prepare', 'installed', 'drain', 'reboot')]
            witness.require(credentials[0] == credentials[1] == credentials[2], 'credential metadata/hash drift')
            for path, observed in credentials[3]['files'].items():
                prior = credentials[0]['files'][path]
                if observed['btrfs_identity'] is not None and observed['btrfs_identity'] == prior['btrfs_identity']:
                    observed['stat'][0] = prior['stat'][0]  # Comparison copy only; original record remains retained.
            witness.require(credentials[3] == credentials[0], 'credential metadata/hash drift')
    return dict(bundle_sha256=sha(bundle.read_bytes()), bytes=bundle.stat().st_size, phases=sorted(expected_phases))


if __name__ == '__main__':
    guest_main(json.loads(Path(sys.argv[1]).read_text()), sys.argv[2])
