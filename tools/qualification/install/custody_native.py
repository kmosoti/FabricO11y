#!/usr/bin/env python3
"""Qualification-only guest capture; original installed paths remain authority."""
import hashlib
import fcntl
import struct
import json
import math
import os
from pathlib import Path
import pwd
import re
import selectors
import shutil
import signal
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
def expected_profile(phase):
    revision = {'prepare': 1, 'before': 1, 'installed': 1, 'drain': 2, 'reboot': 4}[phase]
    return dict(NATIVE_PROFILE, revision=revision, paused=phase in ('drain', 'reboot'))

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
        self.process_observations = []
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
        started_ns = time.time_ns()
        started_mono_ns = time.monotonic_ns()
        result = subprocess.run(argv, capture_output=True, text=True, timeout=min(30, remaining))
        finished_mono_ns = time.monotonic_ns()
        finished_ns = time.time_ns()
        elapsed_ns = finished_mono_ns - started_mono_ns
        record = dict(argv=argv, exit=result.returncode, stdout=result.stdout, stderr=result.stderr,
                      started_ns=started_ns, finished_ns=finished_ns, elapsed_ns=elapsed_ns,
                      elapsed_seconds=time.monotonic()-started)
        if argv[:2] == ['fabricctl', 'inspect'] and result.returncode == 0:
            # Pin the exact captured text record; subprocess(text=True) may normalize newlines.
            stdout_bytes = result.stdout.encode('utf-8')
            leaf = 'command-' + str(len(self.commands) + 1).zfill(4) + '-stdout.txt'
            artifact = self.output / leaf
            with artifact.open('xb') as stream:
                stream.write(stdout_bytes)
            os.chmod(artifact, 0o600)
            record['stdout_artifact'] = dict(path=leaf, sha256=sha(stdout_bytes),
                                             encoding='utf-8-from-subprocess-text')
        self.commands.append(record)
        self.save('commands.json', self.commands)
        witness.require(not check or result.returncode == 0, 'observer command failed: ' + argv[0])
        return record

    def transition(self, revision, paused, *, require_applied, issue=True):
        if issue:
            action = 'pause' if paused else 'resume'
            self.command(['fabricctl', 'admin', '/root/accept/admin.conf', 'node', action, 'spindle-1'])
        deadline = min(self.deadline, time.monotonic() + 30)
        status = 'paused' if paused else 'active'
        while time.monotonic() < deadline:
            record = self.command(['fabricctl', 'admin', '/root/accept/admin.conf', 'node', 'list'])
            rows = json.loads(record['stdout'])['nodes']
            matches = [row for row in rows if row.get('name') == 'spindle-1']
            witness.require(len(matches) == 1, 'native control inventory identity')
            row = matches[0]
            witness.require(all(type(row[key]) is int for key in
                ('desired_revision', 'applied_revision', 'metric_interval_s')),
                'native control integer fields')
            witness.require(row['desired_revision'] == revision and row['status'] == status
                and row['config_error'] is None and row['logs'] == NATIVE_PROFILE['logs']
                and row['metric_interval_s'] == NATIVE_PROFILE['metric_interval_s'],
                            'native desired profile transition')
            if not require_applied or row['applied_revision'] == revision:
                return row
            time.sleep(min(.25, deadline - time.monotonic()))
        witness.require(False, 'native applied profile deadline')

    def drain_prefix(self, minimum=-1, *, exact=None):
        deadline = min(self.deadline, time.monotonic() + 40)
        previous = None
        while time.monotonic() < deadline:
            record = self.command(['fabricctl', 'inspect', '/etc/fabrico11y/node.conf'], check=False)
            if record['exit']:
                witness.require(any(record['stderr'].endswith(message) for message in (
                    'log rotated during inspection; retry\n', 'log append or rotation in progress; retry inspection\n',
                    'log append completed during inspection; retry\n', 'log failure recorded during inspection; retry\n',
                    'sealed log reclaimed during inspection; retry\n', 'log rotated or reclaimed during inspection; retry\n',
                    'log append in progress or recovery required; retry inspection\n')),
                    'native spool inspection failed')
                time.sleep(min(.25, deadline - time.monotonic()))
                continue
            values = dict(field.split('=', 1) for field in record['stdout'].split())
            witness.require(values['recovery_required'] == 'false'
                and values['interrupted_append'] == 'false', 'native spool requires recovery')
            highwater = int(values['next_sequence']) - 1
            matches = highwater == exact if exact is not None else highwater > minimum
            if matches and highwater == previous and int(values['acked_through']) >= highwater:
                return highwater
            previous = highwater
            time.sleep(min(.25, deadline - time.monotonic()))
        witness.require(False, 'native fixed-prefix ACK deadline')

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
            == expected_profile(name),
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
        observation_started_ns = time.time_ns()
        show = self.command(['systemctl', 'show', UNITS[1], '-p', 'MainPID', '-p', 'ControlGroup', '-p', 'InvocationID'])
        values = dict(line.split('=', 1) for line in show['stdout'].splitlines())
        pid = int(values['MainPID']); proc = Path('/proc') / str(pid)
        info = (proc / 'stat').read_text(); uid = (proc / 'status').read_text()
        cgroup = (proc / 'cgroup').read_text()
        now, uptime, ticks = time.time_ns(), time.clock_gettime_ns(time.CLOCK_BOOTTIME), os.sysconf('SC_CLK_TCK')
        start_ticks = int(info.rsplit(')', 1)[1].split()[19])
        start_ns = max(0, now - uptime + start_ticks * 1000000000 // ticks - 1000000000)
        executable = self.command(['sha256sum', str(proc / 'exe')])['stdout'].split()[0]
        witness.require(executable == self.config['node_sha256'] and pid > 0
            and re.fullmatch('[0-9a-f]{32}', values['InvocationID']) and values['InvocationID'] != '0' * 32
            and values['ControlGroup'] == '/system.slice/system-fabrico11y.slice/' + UNITS[1]
            and cgroup.strip() == '0::' + values['ControlGroup']
            and all(int(value) == pwd.getpwnam('fabricolly').pw_uid for value in re.search(r'^Uid:\s+(.+)$', uid, re.M)[1].split())
            and all(int(value) == pwd.getpwnam('fabricolly').pw_gid for value in re.search(r'^Gid:\s+(.+)$', uid, re.M)[1].split()),
            'candidate executable/service identity')
        calibration = dict(realtime_ns=now, boottime_ns=uptime, clock_ticks=ticks,
                           start_ticks=start_ticks)
        process = dict(**self.config['binding'], unit=UNITS[1], pid=pid,
                       boot_id=Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
                       start_ns=start_ns, end_ns=0, proc_stat=info, status=uid,
                       exe_sha256=executable, show=show, invocation_id=values['InvocationID'],
                       calibration=calibration)
        observation_finished_ns = time.time_ns()
        self.process_observations.append(dict(process, started_ns=observation_started_ns,
            finished_ns=observation_finished_ns, cgroup=cgroup))
        self.save('process-observations.json', self.process_observations)
        return process

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


def wait_readiness(guest, deadline):
    original = guest.deadline
    guest.deadline = min(original, deadline)
    expected = ['fabricctl', 'admin', '/root/accept/admin.conf', 'query', json.dumps(
        dict(kind='logs', node='spindle-1', from_ns=0, to_ns=9223372036854775807, limit=1000))]
    try:
        while True:
            witness.require(time.monotonic() < guest.deadline, 'native drain deadline')
            count = len(guest.commands)
            try:
                rows = guest.query('readiness')
            except ValueError:
                attempts = guest.commands[count:]
                if not (len(attempts) == 1 and attempts[0]['argv'] == expected and attempts[0]['exit'] == 2
                    and attempts[0]['stdout'] == '' and attempts[0]['stderr'] == 'fabricctl: transport: io: Connection refused\n'):
                    raise
                rows = []
            witness.require(time.monotonic() < guest.deadline, 'native drain deadline')
            if any(row['body'] == UNREAD and row['attributes'].get('log.file.path') == SOURCE for row in rows):
                return
            time.sleep(min(1, guest.deadline - time.monotonic()))
    finally:
        guest.deadline = original


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
            # Readiness precedes the explicit pause and fixed-prefix drain.
            deadline = min(guest.deadline - 40, time.monotonic() + 40)
            wait_readiness(guest, deadline)
            current = guest.process()
            witness.require(current['pid'] == process['pid'] and current['invocation_id'] == process['invocation_id'] and current['proc_stat'].rsplit(')', 1)[1].split()[19]
                == process['proc_stat'].rsplit(')', 1)[1].split()[19], 'candidate process changed before quiesce')
            if phase == 'reboot':
                prior = unpack(json.loads((guest.base / 'drain' / 'drain-snapshot.json').read_text()))
                prior_ack = prior['ack']
                guest.transition(3, False, require_applied=True, issue=False)
                # This active-revision prefix is only a lower bound; a commit may race pause application.
                guest.drain_prefix(prior_ack)
            current = guest.process()
            witness.require(current['pid'] == process['pid'] and current['invocation_id'] == process['invocation_id']
                and current['proc_stat'].rsplit(')', 1)[1].split()[19]
                == process['proc_stat'].rsplit(')', 1)[1].split()[19], 'candidate process changed before quiesce')
            revision = 2 if phase == 'drain' else 4
            guest.transition(revision, True, require_applied=True)
            # The applied_revision response confirms pause application; the journal marker is checked later.
            guest.drain_prefix()
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
            # Inspect the stopped cutoff before restarting the server or changing the node profile.
            guest.command(['fabricctl', 'inspect', '/etc/fabrico11y/node.conf'])
            guest.command(['systemctl', 'start', UNITS[0]])
            # Resume exactly once while the node remains stopped.
            guest.transition(3 if phase == 'drain' else 5, False, require_applied=False)
            if phase == 'reboot':
                read = lambda phase, name: unpack(json.loads((guest.base / phase / name).read_text()))
                before, installed, drain = (read(phase, name + '-snapshot.json') for phase, name in
                    (('prepare', 'before'), ('installed', 'installed'), ('drain', 'drain')))
                for value in (before, installed, drain, snapshot): value['fixture_paths'] = tuple(value['fixture_paths'])
                witness.conserve(before, installed, drain, dict(before=read('prepare', 'baseline.json'),
                    after_drain=read('drain', 'rows.json'), after_reboot=rows, pending_body=PENDING, unread_body=UNREAD,
                    boot_before=before['boot_id'], boot_after=snapshot['boot_id']), read('drain', 'ack-events.json'),
                    after_reboot=snapshot, reboot_ack_events=events, native_configuration_transition=True)
                guest.save('conservation.json', dict(status='accepted', required_checkpoints=4))
            if phase == 'reboot':
                guest.command(['systemctl', 'start', UNITS[1]])
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
        if event.get('MESSAGE', '').startswith(('delivery sequence=', 'batch=', 'config revision=')):
            witness.require(event.get('_SYSTEMD_UNIT') == process['unit'] and event.get('_EXE') == '/usr/bin/fabric-node'
                and event.get('_SYSTEMD_INVOCATION_ID') == process['invocation_id']
                and event.get('_SYSTEMD_CGROUP') == '/system.slice/system-fabrico11y.slice/' + process['unit']
                and event.get('_UID') == re.search(r'^Uid:\s+(\d+)', process['status'], re.M)[1]
                and event.get('_GID') == re.search(r'^Gid:\s+(\d+)', process['status'], re.M)[1], 'raw journal sender mismatch')
        events.append(dict(unit=event.get('_SYSTEMD_UNIT'), pid=int(event['_PID']),
            boot_id=str(__import__('uuid').UUID(event['_BOOT_ID'])), at_ns=int(event['__REALTIME_TIMESTAMP']) * 1000,
            message=event.get('MESSAGE', '')))
    return events


def pinned_cutoff(pin, data):
    """Read the explicitly admitted executable; no search or runtime compilation."""
    witness.require(isinstance(pin, dict) and set(pin) == {'path', 'sha256'}
        and isinstance(pin['path'], str) and isinstance(pin['sha256'], str)
        and re.fullmatch('[0-9a-f]{64}', pin['sha256']), 'cutoff helper pin required')
    path = Path(pin['path'])
    witness.require(path.is_absolute() and path.resolve(strict=True) == path
        and path.is_relative_to(data / 'results'), 'cutoff helper outside DATA results')
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC), 'rb') as stream:
        before = os.fstat(stream.fileno())
        witness.require(stat.S_ISREG(before.st_mode) and before.st_uid == os.getuid()
            and before.st_nlink == 1 and before.st_mode & 0o111 and before.st_size <= 64 * 1024**2,
            'unsafe cutoff executable')
        raw = stream.read(before.st_size + 1)
        witness.require(len(raw) == before.st_size and sha(raw) == pin['sha256']
            and witness.identity(before) == witness.identity(os.fstat(stream.fileno()))
            == witness.identity(path.lstat()), 'cutoff executable changed or digest mismatch')
    return raw


def captured_cutoffs(archive, files, snapshots, admission, pin, result_dir, data, deadline, bundle_sha256):
    """Transport original admitted members; all projection/cutoff decisions are Rust."""
    data, result_dir = Path(data), Path(result_dir)
    witness.require(str(data) == os.environ['FABRIC_STORAGE_ROOT']
        and result_dir.resolve(strict=True) == result_dir and result_dir.is_relative_to(data / 'results')
        and result_dir.stat().st_uid == os.getuid() and stat.S_IMODE(result_dir.stat().st_mode) == 0o700,
        'cutoff result/root mismatch')
    base = result_dir / 'custody-cutoff-inputs'
    base.mkdir(mode=0o700)  # Exclusive: never clean a directory inherited from another attempt.
    identity = (base.stat().st_dev, base.stat().st_ino)
    def create(path, mode=0o600):
        return os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, mode), 'wb')
    def write(path, raw, mode=0o600):
        with create(path, mode) as stream:
            stream.write(raw)
    raw = pinned_cutoff(pin, data)
    executable = base / 'helper'
    write(executable, raw, 0o500)
    witness.require(sha(executable.read_bytes()) == pin['sha256'], 'helper snapshot mismatch')
    outcomes, total = [], 0
    for phase, prior in (('drain', 'installed'), ('reboot', 'drain')):
        directory = base / phase; directory.mkdir(mode=0o700)
        def member(leaf, limit):
            nonlocal total
            witness.require(isinstance(leaf, str) and re.fullmatch('[A-Za-z0-9][A-Za-z0-9._-]{0,127}', leaf), 'unsafe capture leaf')
            source = files[phase + '/' + leaf]
            witness.require(source['item'].size <= limit and total + source['item'].size <= 64 * 1024**2, 'capture input bound')
            body = archive.extractfile(source['item']).read(limit + 1)
            witness.require(len(body) == source['item'].size and sha(body) == source['sha256'], 'capture member changed')
            total += len(body); write(directory / leaf, body)
            return dict(path=str((directory / leaf).relative_to(data)), sha256=sha(body))
        manifest = dict(version=1, candidate=admission['binding']['candidate_frozen'], phase=phase,
            prior_ack=snapshots[prior]['ack'], relative_phase_root=str(directory.relative_to(data)))
        for key, leaf in (('commands', 'commands.json'), ('observed_events', 'ack-events.json'),
                ('process_observations', 'process-observations.json'), ('stopped_snapshot', phase + '-snapshot.json')):
            manifest[key] = member(leaf, 4 * 1024**2)
        for command in json.loads((directory / 'commands.json').read_bytes()):
            if 'stdout_artifact' in command:
                artifact = command['stdout_artifact']; copied = member(artifact['path'], 4096)
                witness.require(copied['sha256'] == artifact['sha256'], 'inspection archive/stdout mismatch')
        plan = (json.dumps(manifest, separators=(',', ':')) + '\n').encode()
        write(directory / 'manifest.json', plan)
        report = dict(phase=phase, candidate=manifest['candidate'], helper=pin, manifest_sha256=sha(plan),
            bundle_sha256=bundle_sha256, input_root=str(directory), state='started')
        prefix = result_dir / ('custody-cutoff-' + phase)
        try:
            remaining = min(60, deadline - time.monotonic())
            witness.require(remaining > 0, 'cutoff invocation deadline')
            command = [str(executable), 'custody-cutoff', '--root', str(data),
                '--capture-manifest', str((directory / 'manifest.json').relative_to(data)),
                '--manifest-sha256', sha(plan), '--candidate', manifest['candidate']]
            cutoff = time.monotonic() + remaining
            output_limit = 65536
            with create(prefix.with_suffix('.stdout')) as out, create(prefix.with_suffix('.stderr')) as err:
                proc = subprocess.Popen(command, stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
                selector = None
                sinks = {'stdout': out, 'stderr': err}
                sizes = {'stdout': 0, 'stderr': 0}
                overflow = False
                timed_out = False
                stop_at = None
                def stop_group():
                    try:
                        os.killpg(proc.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                try:
                    selector = selectors.DefaultSelector()
                    for name, pipe in (('stdout', proc.stdout), ('stderr', proc.stderr)):
                        os.set_blocking(pipe.fileno(), False)
                        selector.register(pipe, selectors.EVENT_READ, name)
                    while selector.get_map():
                        now = time.monotonic()
                        if stop_at is None and now >= cutoff:
                            timed_out = True
                            stop_at = now + 2
                            stop_group()
                        elif stop_at is not None and now >= stop_at:
                            break
                        wait = (cutoff - now) if stop_at is None else (stop_at - now)
                        for key, _ in selector.select(max(0.0, min(wait, 0.25))):
                            try:
                                chunk = os.read(key.fd, 8192)
                            except BlockingIOError:
                                continue
                            if not chunk:
                                selector.unregister(key.fileobj)
                                key.fileobj.close()
                                continue
                            name = key.data
                            room = output_limit - sizes[name]
                            if room > 0:
                                kept = chunk[:room]
                                sinks[name].write(kept)
                                sizes[name] += len(kept)
                            if len(chunk) > max(room, 0):
                                overflow = True
                                if stop_at is None:
                                    stop_at = time.monotonic() + 2
                                    stop_group()
                    if selector.get_map():
                        for key in list(selector.get_map().values()):
                            selector.unregister(key.fileobj)
                            key.fileobj.close()
                    while not timed_out and not overflow:
                        now = time.monotonic()
                        if now >= cutoff:
                            timed_out = True
                            break
                        observed = os.waitid(os.P_PID, proc.pid,
                            os.WEXITED | os.WNOHANG | os.WNOWAIT)
                        if observed is not None and observed.si_pid:
                            break
                        time.sleep(min(0.01, cutoff - now))
                finally:
                    if selector is not None:
                        selector.close()
                    for pipe in (proc.stdout, proc.stderr):
                        if not pipe.closed:
                            pipe.close()
                    # The leader remains unreaped through this signal, so its PID/PGID
                    # cannot be reused before descendants in the original group are stopped.
                    stop_group()
                    proc.wait()
                    report['exit'] = proc.returncode
                    report['stdout_bytes'] = sizes['stdout']
                    report['stderr_bytes'] = sizes['stderr']
                    report['output_overflow'] = overflow
            if timed_out:
                raise subprocess.TimeoutExpired(command, remaining)
            witness.require(not overflow and proc.returncode == 0, 'Rust capture verifier refused')
            def unique(items):
                result = {}
                for key, value in items:
                    witness.require(key not in result, 'duplicate Rust report key')
                    result[key] = value
                return result
            def nonfinite(value):
                raise ValueError('nonfinite Rust report number: ' + value)
            def finite_float(value):
                number = float(value)
                witness.require(math.isfinite(number), 'nonfinite Rust report number: ' + value)
                return number
            result = json.loads(prefix.with_suffix('.stdout').read_bytes(),
                object_pairs_hook=unique, parse_constant=nonfinite, parse_float=finite_float)
            witness.require(isinstance(result, dict) and result.get('status') == 'structural-cutoff'
                and result.get('candidate') == manifest['candidate'], 'Rust capture report mismatch')
            report.update(state='completed', result=result); outcomes.append(report)
        except BaseException as error:
            report.update(state='failed', error=type(error).__name__, message=str(error))
            raise
        finally:
            write(prefix.with_suffix('.json'), (json.dumps(report, sort_keys=True) + '\n').encode())
    witness.require(base.resolve(strict=True) == base and (base.stat().st_dev, base.stat().st_ino) == identity,
        'cutoff scratch ownership changed')
    shutil.rmtree(base)
    return outcomes


# Host admission freezes owner inputs before boot and rechecks them at transfer.
def host_admission(owner_path, source_root, storage, package, source_commit, run_id, family):
    owner = json.loads(owner_path.read_text())
    cutoff_pin = owner.get('cutoff_helper')
    pinned_cutoff(cutoff_pin, storage)
    git = lambda *args: subprocess.check_output(['git', '-C', str(source_root), *args], text=True).strip()
    witness.require(not git('status', '--porcelain') and git('rev-parse', 'HEAD') == owner['harness_commit'], 'clean harness source mismatch')
    subprocess.run(['git', '-C', str(source_root), 'merge-base', '--is-ancestor', owner['harness_commit'], 'origin/main'], check=True)
    names = ('custody_native.py', 'custody_witness.py', 'lifecycle.sh', 'run-qemu.py', 'acceptance.sh', 'removal_observer.py')
    paths = [source_root / 'tools/qualification/install' / name for name in names] + [source_root / 'tools/qualification/query_oracle.py']
    source_hashes = {str(path.relative_to(source_root)): sha(path.read_bytes()) for path in paths}
    witness.require(source_hashes == owner['source_hashes'], 'producer/helper source mismatch')
    debdir = storage / 'results/installation-package-build/certificate-distrust-01-deb-a'
    rpmdir = storage / 'results/packages/certificate-distrust-01-rpm-a'
    fixed = {debdir / 'receipt.json': '0b304acefd1dab014ae547f7c9767aba3dec05ff363a540e8b941b381b79627c',
             rpmdir / 'receipt.json': '0d11fde82ce12a3de06da9f516a6f651ebaf1597aeaa990ec18fdd0c6bf28721',
             storage / 'results/candidate-build-certificate-distrust-01/artifact-admission.json':
             'ed4219351dbaceef6c143b93a1bea662d17f10053cc8da549fcd63465f00d4ec',
             storage / 'results/candidate-build-certificate-distrust-01/package-inspection.json':
             '27b6ae8820d5a4c0f5f3c90a15e203faee19de8828d7882db85d8bcf7c261031'}
    fixed[Path(cutoff_pin['path'])] = cutoff_pin['sha256']
    for path, expected in fixed.items(): witness.require(sha(path.read_bytes()) == expected, 'owner package/admission receipt mismatch')
    deb, rpm = (json.loads((directory / 'receipt.json').read_text()) for directory in (debdir, rpmdir))
    frozen, canonical = '1eaf9ad17dd5507809cb5cca0964e6e84758648a', '531e67cbf678af483debab7ea0c0f7ecf6b79686'
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
        (storage / 'results/console-certificate-distrust-01-a/build.json', deb['console_receipt_sha256'])):
        witness.require(sha(path.read_bytes()) == expected_hash, 'canonical source/console mismatch')
        fixed[path] = expected_hash
    return dict(binding=dict(run_id=run_id, candidate_frozen=frozen, candidate_package_sha256=expected['package_sha256'],
        package_source_commit=canonical, harness_commit=owner['harness_commit']), source_hashes=source_hashes,
        cutoff_helper=cutoff_pin, owner_sha256=sha(owner_path.read_bytes()), inputs={str(path): value for path, value in fixed.items()},
        node_sha256=rpm['payload_identity']['usr/bin/fabric-node']['sha256'], fixture_paths=[SOURCE], spool_root=str(ROOT))


def admit_transfer(bundle, admission, guest_sha256, *, control=None, cutoff_helper=None, result_dir=None, data_root=None, deadline=None):
    """Inspect the labelled archive; never substitute an extracted root for guest input."""
    if not control:
        witness.require(isinstance(cutoff_helper, dict) and cutoff_helper == admission.get('cutoff_helper')
            and result_dir is not None and data_root is not None and deadline is not None, 'frozen cutoff invocation required')
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
                                and json.loads(content) == expected_profile(phase), 'native applied configuration profile mismatch')
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
        cutoff_outcomes = []
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
                read('drain/ack-events.json'), after_reboot=snapshots['reboot'], reboot_ack_events=read('reboot/ack-events.json'), native_configuration_transition=True)
            credentials = [read(phase + '/' + ('before' if phase == 'prepare' else phase) + '-credential.json') for phase in ('prepare', 'installed', 'drain', 'reboot')]
            witness.require(credentials[0] == credentials[1] == credentials[2], 'credential metadata/hash drift')
            for path, observed in credentials[3]['files'].items():
                prior = credentials[0]['files'][path]
                if observed['btrfs_identity'] is not None and observed['btrfs_identity'] == prior['btrfs_identity']:
                    observed['stat'][0] = prior['stat'][0]  # Comparison copy only; original record remains retained.
            witness.require(credentials[3] == credentials[0], 'credential metadata/hash drift')
            try:
                cutoff_outcomes = captured_cutoffs(archive, files, snapshots, admission, cutoff_helper,
                    result_dir, data_root, deadline, guest_sha256)
            except BaseException as error:
                report = dict(state='failed', bundle_sha256=guest_sha256, helper=cutoff_helper,
                    error=type(error).__name__, message=str(error))
                path = Path(result_dir) / 'custody-cutoff-failure.json'
                with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600), 'w') as stream:
                    json.dump(report, stream)
                raise
    return dict(bundle_sha256=sha(bundle.read_bytes()), bytes=bundle.stat().st_size,
        phases=sorted(expected_phases), cutoff_outcomes=cutoff_outcomes)


if __name__ == '__main__':
    guest_main(json.loads(Path(sys.argv[1]).read_text()), sys.argv[2])
