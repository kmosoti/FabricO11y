"""Explicit production adapter; the historical campaign decisions stay separate."""
import json
from pathlib import Path
import shutil
import subprocess

from release_runtime import CandidateFixture, WorkloadReader, STORAGE, directory_bytes, digest


def options(parser):
    parser.add_argument('--production-access', action='store_true')
    parser.add_argument('--deb-receipt', type=Path)
    parser.add_argument('--rpm-receipt', type=Path)
    parser.add_argument('--production-out', type=Path)


def validate_options(args):
    paths = (args.deb_receipt, args.rpm_receipt, args.production_out)
    if args.production_access != all(path is not None for path in paths):
        raise ValueError('production access requires all three exact receipt/output paths')
    if not args.production_access and any(path is not None for path in paths):
        raise ValueError('production options require explicit production access mode')


class ProductionAccess(CandidateFixture):
    def __init__(self, args, raw_root, *, soak):
        validate_options(args)
        self.raw_root = Path(raw_root)
        self.soak = soak
        allocated = int(subprocess.check_output(['du', '-s', '-B1', str(STORAGE)]).split()[0])
        if allocated + 5 * 1024**3 > 95_000_000_000:
            raise RuntimeError('5 GiB reservation exceeds 95 GB admission stop')
        super().__init__(args.deb_receipt, args.rpm_receipt, args.production_out)
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
        self.receipt['historical_oracles_unchanged'] = True
        source = Path(__file__).resolve().parent
        self.receipt['harness_sha256'] = {name: digest(source / name) for name in
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
        if directory_bytes(self.raw_root) + directory_bytes(self.work) + directory_bytes(self.out) > 5 * 1024**3:
            raise RuntimeError('combined production campaign live data exceeded 5 GiB')
        allocated = int(subprocess.check_output(['du', '-s', '-B1', str(STORAGE)]).split()[0])
        if allocated >= 95_000_000_000:
            raise RuntimeError('production campaign reached aggregate 95 GB stop')
