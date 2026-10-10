"""Explicit production adapter; the historical campaign decisions stay separate."""
import json
import os
import stat
import re
from pathlib import Path
import shutil
import subprocess

from release_runtime import CandidateFixture, WorkloadReader, STORAGE, directory_bytes, digest, cpu_pair


def options(parser):
    parser.add_argument('--production-access', action='store_true')
    parser.add_argument('--deb-receipt', type=Path)
    parser.add_argument('--rpm-receipt', type=Path)
    parser.add_argument('--production-out', type=Path)
    parser.add_argument('--production-server-cpus', choices=('0-1', '4-5'), default='0-1')


def server_placement(args, *, soak):
    value = getattr(args, 'production_server_cpus', '0-1')
    selected = cpu_pair(value)
    if selected & {2, 3} or value not in ('0-1', '4-5'):
        raise ValueError('production server placement must be 0-1 or 4-5, disjoint from workers 2-3')
    if value != '0-1' and (soak or not args.production_access):
        raise ValueError('alternative server placement requires a production outage; soak retains 0-1')
    return value


def validate_options(args):
    server_placement(args, soak=False)
    paths = (args.deb_receipt, args.rpm_receipt, args.production_out)
    if args.production_access != all(path is not None for path in paths):
        raise ValueError('production access requires all three exact receipt/output paths')
    if not args.production_access and any(path is not None for path in paths):
        raise ValueError('production options require explicit production access mode')


class ProductionAccess(CandidateFixture):
    def __init__(self, args, raw_root, *, soak):
        validate_options(args)
        placement = server_placement(args, soak=soak)
        self.raw_root = Path(raw_root)
        self.soak = soak
        allocated = int(subprocess.check_output(['du', '-s', '-B1', str(STORAGE)]).split()[0])
        if allocated + 5 * 1024**3 > 95_000_000_000:
            raise RuntimeError('5 GiB reservation exceeds 95 GB admission stop')
        super().__init__(args.deb_receipt, args.rpm_receipt, args.production_out,
                         server_cpus=placement, supervisor_cpus='2-3')
        # Match the original bin/examples layout using immutable verified copies.
        examples = self.bins / 'examples'
        examples.mkdir()
        for name in ('spindle_sim', 'server_dump', 'spool_dump'):
            shutil.copy2(self.helpers / name, examples / name)
            if digest(examples / name) != digest(self.helpers / name):
                raise RuntimeError("immutable qualification helper copy changed")
            self.input_hashes[str((examples / name).relative_to(self.work))] = digest(examples / name)
        self.state_root = self.work / 'state'
        self.query_reader = None
        self.receipt['adapter'] = 'production-access-campaign-protocol'
        self.receipt['adapter_cpu_placement'] = {
            'server': sorted(cpu_pair(placement)), 'workers': [2, 3],
            'soak': soak, 'policy_commit': '414fb29'}
        self.receipt['historical_oracles_unchanged'] = True
        source = Path(__file__).resolve().parent
        self.receipt['adapter_harness_sha256'] = {name: digest(source / name) for name in
            ('production_access.py', 'soak_tier.py', 'outage_drain.py', 'console_bridge.py',
             'delivery_oracle.py', 'soak_companion.py')}


    def spawn(self, command, label, group=None, cpus="2-3"):
        # R2/outage did not enable optional timing-event logging. Preserve that
        # measured command while retaining the normal production companion.
        if label.startswith("server-"):
            command = [part for part in command if str(part) != "--timing-events"]
        return super().spawn(command, label, group, cpus)

    def start_server(self):
        # Called virtually before the first launch by CandidateFixture. Preserve
        # registered campaign storage/default retention instead of R3 settings.
        lines = self.config.read_text().splitlines()
        lines = [line for line in lines if not line.startswith(
            ('journal_bytes=', 'journal_file_bytes=', 'retention_bytes=', 'retention_s='))]
        if self.soak:
            lines += ['journal_bytes=4294967296', 'journal_file_bytes=67108864']
        self.config.write_text('\n'.join(lines) + '\n')
        super().start_server()

    def call(self, method, suffix='', body=None):
        path = '/v1/console/nodes' + suffix
        status, value = self.bridge.request(path, method, body)
        if status != 200:
            raise RuntimeError('production control request returned status %d' % status)
        return value

    def query_call(self, method, body=None):
        if method != 'POST' or self.query_reader is None:
            raise ValueError('scoped query reader must be issued before reads')
        return self.query_reader.query(body)

    def issue_reader(self, enrollments, ttl_s):
        self.query_reader = self.reader(enrollments, ttl_s=ttl_s)
        return self

    def sample_bound(self):
        bridge = getattr(self, 'bridge', None)
        if bridge is not None and getattr(bridge, 'ui_polling', False):
            bridge.poll_health()
        temporary = getattr(getattr(self, "bridge", None), "browser_temporary", None)
        temporary_bytes = directory_bytes(temporary) if temporary is not None else 0
        if directory_bytes(self.raw_root) + directory_bytes(self.work) + directory_bytes(self.out) + temporary_bytes > 5 * 1024**3:
            raise RuntimeError('combined production campaign live data exceeded 5 GiB')
        allocated = int(subprocess.check_output(['du', '-s', '-B1', str(STORAGE)]).split()[0])
        if allocated >= 95_000_000_000:
            raise RuntimeError('production campaign reached aggregate 95 GB stop')

    def close(self):
        if getattr(self, 'closed', False):
            return
        temporary = getattr(getattr(self, 'bridge', None), 'browser_temporary', None)
        observation = None
        observation_error = None
        ui_error = None
        bridge = getattr(self, 'bridge', None)
        if bridge is not None and hasattr(bridge, 'poll_health'):
            try:
                health = bridge.poll_health(require_progress=False)
                checked = health['phases'] > 0
                passed = not checked or (health['attempts'] > 0 and health['completed'] > 0
                    and health['statusFailures'] == 0 and health['transportFailures'] == 0
                    and health['inflight'] == 0)
                self.receipt['ui_polling_prerequisite'] = {'checked': checked, 'passed': passed}
                if not passed:
                    raise RuntimeError('actual UI polling prerequisite failed')
            except (OSError, RuntimeError) as error:
                ui_error = error
                self.receipt['passed'] = False
                self.receipt['ui_polling_prerequisite'] = {'checked': True, 'passed': False,
                    'error_type': type(error).__name__}
        try:
            if temporary is not None:
                socket_lengths = []
                for base, _, names in os.walk(temporary, followlinks=False):
                    for name in names:
                        path = Path(base) / name
                        try:
                            if stat.S_ISSOCK(path.lstat().st_mode):
                                socket_lengths.append(len(os.fsencode(str(path))))
                        except FileNotFoundError:
                            pass
                observation = {'path': str(temporary), 'directory_path_bytes': len(os.fsencode(str(temporary))),
                               'regular_bytes_before_cleanup': directory_bytes(temporary),
                               'socket_path_bytes': socket_lengths}
        except OSError as error:
            observation_error = error
            observation = {"path": str(temporary), "observation_error": type(error).__name__}
        try:
            super().close()
        finally:
            if observation is not None:
                observation['removed'] = not temporary.exists()
            self.receipt['browser_temporary_cleanup'] = observation
            # spindle_sim mirrors its supplied credential list into temporary
            # token files. They are not oracle ledgers or recovery evidence.
            removed = []
            raw_root = getattr(self, 'raw_root', None)
            if raw_root is not None:
                for path in sorted((raw_root / 'sim').glob('token-*')):
                    info = path.lstat()
                    if (not re.fullmatch(r'token-[0-9]{1,3}', path.name)
                            or not stat.S_ISREG(info.st_mode) or info.st_nlink != 1
                            or info.st_uid != os.getuid() or not 64 <= info.st_size <= 65):
                        raise RuntimeError('unexpected simulator credential cleanup member')
                    removed.append({'name': str(path.relative_to(raw_root)), 'bytes': info.st_size,
                                    'sha256': digest(path)})
                    path.unlink()
            self.receipt['simulator_credential_cleanup'] = removed
            (self.out / 'runtime.json').write_text(json.dumps(self.receipt, indent=2) + '\n')
        if observation_error is not None:
            raise RuntimeError('browser temporary observation failed after confirmed cleanup') from observation_error
        if observation is not None and not observation['removed']:
            raise RuntimeError('owned browser temporary directory remained after cleanup')
        if ui_error is not None:
            raise RuntimeError('actual UI polling prerequisite failed after cleanup') from ui_error
