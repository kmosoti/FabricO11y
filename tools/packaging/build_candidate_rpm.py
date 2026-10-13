#!/usr/bin/env python3
"""Record exact candidate RPM construction or lower metadata owner fixtures."""
import argparse
import io
import json
import os
import re
import shutil
import signal
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import xml.etree.ElementTree as ET

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE.parent))
from resource_group import require_limits, STORAGE
from stage_candidate import digest, stage_candidate
from payload import assert_equivalent, bounded_command, cpio, identity, inventory, stop_group

FIXTURE_VERSIONS = {'debian': ('0.1.0~alpha.1', '0.1.0~alpha.0'),
                    'fedora': ('0.1.0-0.alpha.1', '0.1.0-0.alpha.0')}


def validate_fixture_source(source, frozen, epoch):
    if (not isinstance(source, dict) or not isinstance(frozen, str)
            or not re.fullmatch('[0-9a-f]{40}', frozen) or type(epoch) is not int or epoch <= 0
            or source.get('original_head') != frozen or type(source.get('source_date_epoch')) is not int
            or source.get('source_date_epoch') != epoch):
        raise ValueError('fixture source pins differ')


def validate_fixture_transform(family, candidate, fixture, before, after, scripts, changed_scripts):
    if (candidate, fixture) != FIXTURE_VERSIONS.get(family):
        raise ValueError('fixture native metadata differs')
    if not before or before != after:
        raise ValueError('fixture payload differs')
    if not scripts or scripts != changed_scripts:
        raise ValueError('fixture scripts differ')


def run_build(command, env, out, label='build'):
    process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               start_new_session=True)
    interruption = None
    stdout = stderr = b''
    try:
        try:
            stdout, stderr = process.communicate(timeout=300)
        except (subprocess.TimeoutExpired, KeyboardInterrupt) as error:
            interruption = error
            stdout = getattr(error, 'output', None) or stdout
            stderr = getattr(error, 'stderr', None) or stderr
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                stdout, stderr = process.communicate(timeout=10)
            except subprocess.TimeoutExpired as error:
                stdout = error.output or stdout
                stderr = error.stderr or stderr
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                stdout, stderr = process.communicate(timeout=10)
    except BaseException as error:
        interruption = error
        stdout = getattr(error, 'output', None) or stdout
        stderr = getattr(error, 'stderr', None) or stderr
    finally:
        try:
            stop_group(process)
        except BaseException as error:
            interruption = error
        try:
            for stream, body in (('stdout', stdout), ('stderr', stderr)):
                (out / (label + '.' + stream)).write_bytes(body.encode() if isinstance(body, str) else body)
        except BaseException as error:
            error.cleanup_failed = getattr(interruption, 'cleanup_failed', False)
            raise
        finally:
            process.stdout.close()
            process.stderr.close()
    if interruption is not None:
        if isinstance(interruption, subprocess.TimeoutExpired):
            interruption.output, interruption.stderr = stdout, stderr
        raise interruption
    return process.returncode


def deb_controls(package):
    result = {}
    size = 0
    raw = bounded_command(['ar', 'p', str(package), 'control.tar.xz'])
    with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
        for member in archive:
            if member.isdir():
                continue
            name = member.name.removeprefix('./')
            size += member.size
            if (not member.isfile() or not name or '/' in name or name in ('.', '..')
                    or name in result or size > 1024**2):
                raise ValueError('unsafe fixture Debian control entry')
            body = archive.extractfile(member).read()
            if name == 'control':
                if sum(line.startswith(b'Version:') for line in body.splitlines()) != 1:
                    raise ValueError('fixture native metadata differs')
                body = b''.join(line for line in body.splitlines(keepends=True) if not line.startswith(b'Version:'))
            result[name] = {'mode': member.mode, **identity(body)}
    return result


def deb_headers(package):
    fields = ('name', 'mode', 'uid', 'gid', 'uname', 'gname', 'mtime', 'size', 'linkname')
    result = {}
    for name in ('control.tar.xz', 'data.tar.xz'):
        raw = bounded_command(['ar', 'p', str(package), name])
        with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
            result[name] = [{**{field: getattr(member, field) for field in fields},
                             'type': member.type.decode('ascii')} for member in archive]
    return result


def rpm_records(package):
    raw = bounded_command(['rpm', '-qp', '--xml', str(package)])
    header = ET.fromstring(raw)
    prefixes = ('PREIN', 'POSTIN', 'PREUN', 'POSTUN', 'PRETRANS', 'POSTTRANS',
                'VERIFYSCRIPT', 'TRIGGER', 'FILETRIGGER', 'TRANSFILETRIGGER')
    blocks = {match[1].decode().upper(): match[0]
              for match in re.finditer(rb'<rpmTag\b[^>]*\bname="([^"]+)"[^>]*>.*?</rpmTag>', raw, re.S)}
    stable = {'NAME', 'VERSION', 'ARCH', 'OS', 'LICENSE', 'SUMMARY', 'DESCRIPTION',
              'FILEMODES', 'FILEFLAGS', 'FILEVERIFYFLAGS', 'FILEUSERNAME', 'FILEGROUPNAME',
              'FILELINKTOS', 'FILECAPS', 'FILEMTIMES', 'BASENAMES', 'DIRNAMES', 'DIRINDEXES',
              'REQUIRENAME', 'REQUIREFLAGS', 'REQUIREVERSION', 'CONFLICTNAME', 'CONFLICTFLAGS',
              'CONFLICTVERSION', 'OBSOLETENAME', 'OBSOLETEFLAGS', 'OBSOLETEVERSION',
              'PROVIDENAME', 'PROVIDEFLAGS', 'PROVIDEVERSION'}
    stable.update(stem + field for stem in ('RECOMMEND', 'SUGGEST', 'SUPPLEMENT', 'ENHANCE')
                  for field in ('NAME', 'FLAGS', 'VERSION'))
    if not {'NAME', 'VERSION', 'ARCH', 'FILEMODES', 'FILEUSERNAME', 'FILEGROUPNAME',
            'PROVIDENAME', 'PROVIDEFLAGS', 'PROVIDEVERSION'} <= blocks.keys():
        raise ValueError('fixture RPM native metadata unavailable')
    nodes = {node.attrib['name'].upper(): node for node in header.findall('rpmTag')}
    names = [node.text for node in nodes['PROVIDENAME'].iter('string')]
    flags = [node.text for node in nodes['PROVIDEFLAGS'].iter('integer')]
    versions = [node.text for node in nodes['PROVIDEVERSION'].iter('string')]
    if not len(names) == len(flags) == len(versions):
        raise ValueError('fixture RPM provide arrays differ')
    provides = iter(zip(names, flags, versions))
    def normalize_self(match):
        name, flag, version = next(provides)
        if (name in ('fabrico11y', 'fabrico11y(x86-64)') and flag == '8'
                and version == '0.1.0-0.alpha.1' and match[1] == b'0.1.0-0.alpha.1'):
            return match[0].replace(b'0.1.0-0.alpha.1', b'0.1.0-0.alpha.0')
        return match[0]
    blocks['PROVIDEVERSION'] = re.sub(rb'<string>(.*?)</string>', normalize_self,
                                     blocks['PROVIDEVERSION'], flags=re.S)
    tags = {name: identity(block) for name, block in blocks.items()}
    return {'scripts': {name: value for name, value in tags.items() if name.startswith(prefixes)},
            'metadata': {name: value for name, value in tags.items() if name in stable}}


def validate_harness(commit):
    if (not re.fullmatch('[0-9a-f]{40}', commit)
            or bounded_command(['git', '-C', str(ROOT), 'rev-parse', 'HEAD']).decode().strip() != commit
            or bounded_command(['git', '-C', str(ROOT), 'status', '--porcelain'])):
        raise ValueError('fixture requires the exact clean pinned harness')


def fixture_main(args):
    if any(value is None for value in (args.rpm_receipt, args.frozen, args.source_date_epoch, args.harness_commit)):
        raise ValueError('fixture requires RPM receipt, frozen head, epoch and full harness commit')
    validate_harness(args.harness_commit)
    source = json.loads(args.deb_receipt.read_text())['source']
    validate_fixture_source(source, args.frozen, args.source_date_epoch)
    sys.path.insert(0, str(HERE.parent / 'qualification/cross_family'))
    from inputs import validate_inputs
    admitted = validate_inputs(args.deb_receipt, args.rpm_receipt)
    packages = {family: admitted[family] for family in ('debian', 'fedora')}
    inputs = {str(path.resolve()): digest(path) for path in (args.deb_receipt, args.rpm_receipt, *packages.values())}
    before = {family: inventory(package) for family, package in packages.items()}
    rpm_before = rpm_records(packages['fedora'])
    scripts = {'debian': deb_controls(packages['debian']), 'fedora': rpm_before['scripts']}
    versions = FIXTURE_VERSIONS
    version_commands = {'debian': ['dpkg-deb', '-f', str(packages['debian']), 'Version'],
                        'fedora': ['rpm', '-qp', '--qf', '%{VERSION}-%{RELEASE}', str(packages['fedora'])]}
    for family, command in version_commands.items():
        if bounded_command(command).decode().strip() != versions[family][0]:
            raise ValueError('fixture native metadata differs')
        validate_fixture_transform(family, *versions[family], before[family], before[family],
                                   scripts[family], scripts[family])
    if bounded_command(['rpm', '-qp', '--qf', '%{EPOCHNUM}', str(packages['fedora'])]).strip() != b'0':
        raise ValueError('fixture RPM epoch differs')
    deb_compare = ['dpkg', '--compare-versions', versions['debian'][1], 'lt', versions['debian'][0]]
    deb_order = bounded_command(deb_compare)
    if deb_order:
        raise ValueError('unexpected Debian version comparison output')
    rpm_compare = ['rpm', '--eval', "%{lua: print(rpm.vercmp('0:0.1.0-0.alpha.0','0:0.1.0-0.alpha.1'))}"]
    rpm_order = bounded_command(rpm_compare)
    if rpm_order.decode().strip() != '-1':
        raise ValueError('fixture RPM version is not natively older')
    out = args.out.resolve()
    out.relative_to(STORAGE / 'results')
    out.mkdir(parents=True, exist_ok=False)
    record = {'state': 'failed', 'exit': 2, 'role': 'same implementation, lower native metadata only; not historical migration',
              'source': source, 'harness_commit': args.harness_commit, 'inputs': inputs,
              'constructor_sha256': digest(Path(__file__)), 'payload_parser_sha256': digest(HERE / 'payload.py'),
              'input_validator_sha256': digest(HERE.parent / 'qualification/cross_family/inputs.py'),
              'rpm_builder_sha256': digest(ROOT / 'packaging/build-rpm.sh'), 'commands': [], 'packages': {},
              'native_comparisons': [{'command': deb_compare, 'exit': 0, 'stdout': deb_order.decode()},
                                     {'command': rpm_compare, 'exit': 0, 'stdout': rpm_order.decode()}]}
    work = None
    env = {**os.environ, 'SOURCE_DATE_EPOCH': str(args.source_date_epoch)}
    try:
        work = Path(tempfile.mkdtemp(dir=os.environ['FABRIC_SCRATCH_ROOT'], prefix='owner-fixture-'))
        deb = out / 'fabrico11y_0.1.0~alpha.0_amd64.deb'
        def execute(command, label):
            step = {'command': command, 'exit': None}
            record['commands'].append(step)
            status = run_build(command, env, out, label)
            step['exit'] = status
            if status:
                if status < 0:
                    record['state'] = 'interrupted'
                raise ValueError('fixture build failed: ' + label)
        execute(['dpkg-deb', '-R', str(packages['debian']), str(work / 'deb')], 'deb-unpack')
        record['debian_tar_headers_before'] = deb_headers(packages['debian'])
        control = work / 'deb/DEBIAN/control'
        body = control.read_bytes()
        if body.count(b'Version: 0.1.0~alpha.1\n') != 1:
            raise ValueError('fixture native metadata differs')
        control.write_bytes(body.replace(b'Version: 0.1.0~alpha.1\n', b'Version: 0.1.0~alpha.0\n'))
        execute(['dpkg-deb', '--root-owner-group', '-Zxz', '--build', str(work / 'deb'), str(deb)], 'deb-build')
        deb_after_headers = deb_headers(deb)
        if record['debian_tar_headers_before'] != deb_after_headers:
            raise ValueError('fixture Debian tar headers differ')
        stage = work / 'rpm-stage'
        stage.mkdir()
        for name, (body, mode) in cpio(bounded_command(['rpm2cpio', str(packages['fedora'])]), contents=True).items():
            target = stage / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(body)
            target.chmod(mode)
        shutil.copyfile(work / 'deb/DEBIAN/preinst', stage / 'candidate-preinst')
        execute(['bash', str(ROOT / 'packaging/build-rpm.sh'), str(out), str(stage), '--same-code-fixture-stage'], 'rpm-build')
        fixtures = {'debian': deb, 'fedora': out / 'fabrico11y-0.1.0-0.alpha.0.x86_64.rpm'}
        rpm_after = rpm_records(fixtures['fedora'])
        if rpm_before['metadata'] != rpm_after['metadata']:
            raise ValueError('fixture RPM stable native metadata differs')
        changed_scripts = {'debian': deb_controls(deb), 'fedora': rpm_after['scripts']}
        for family, package in fixtures.items():
            command = [str(package) if part == str(packages[family]) else part for part in version_commands[family]]
            version = bounded_command(command).decode().strip()
            actual = inventory(package)
            validate_fixture_transform(family, versions[family][0], version, before[family], actual,
                                       scripts[family], changed_scripts[family])
            record['packages'][family] = {'package': str(package), 'sha256': digest(package), 'version': version,
                                          'payload_identity': actual, 'script_identity': changed_scripts[family]}
        record['packages']['fedora']['native_metadata_identity'] = rpm_after['metadata']
        record['packages']['debian']['tar_header_identity'] = deb_after_headers
        if any(digest(Path(path)) != expected for path, expected in inputs.items()):
            raise ValueError('fixture canonical inputs changed during construction')
        validate_inputs(args.deb_receipt, args.rpm_receipt)
        validate_harness(args.harness_commit)
        record.update(state='built', exit=0, native_older={'debian': True, 'fedora': True})
    except (Exception, KeyboardInterrupt) as error:
        record['error'] = str(error) or type(error).__name__
        if (isinstance(error, (KeyboardInterrupt, subprocess.TimeoutExpired))
                or isinstance(error, subprocess.CalledProcessError) and error.returncode < 0):
            record['state'] = 'interrupted'
        if getattr(error, 'stderr', None) is not None:
            record['stderr'] = error.stderr.decode(errors='replace') if isinstance(error.stderr, bytes) else error.stderr
        if work is not None:
            record['failure_evidence'] = str(work)
            if getattr(error, 'cleanup_failed', False):
                record['archive_error'] = 'process cleanup unconfirmed; staging retained in place'
            else:
                try:
                    shutil.move(str(work), str(out / 'failed-fixture-work'))
                    record['failure_evidence'] = str(out / 'failed-fixture-work')
                except Exception as archive_error:
                    record['archive_error'] = str(archive_error)
    finally:
        try:
            interruption = sys.exc_info()[1]
            if isinstance(interruption, KeyboardInterrupt):
                raise interruption
            if record['state'] == 'built' and work is not None and work.exists():
                try:
                    shutil.rmtree(work)
                except OSError as cleanup_error:
                    record.update(state='failed', exit=2, error=str(cleanup_error), failure_evidence=str(work))
            (out / 'fixture-receipt.json').write_text(json.dumps(record, indent=2) + '\n')
            print(json.dumps(record, indent=2))
        except KeyboardInterrupt as error:
            retained = [path for path in (out / 'failed-fixture-work', work)
                        if path is not None and path.exists()]
            record.update(state='interrupted', exit=2, error=str(error) or type(error).__name__,
                          failure_evidence=str(retained[0]) if retained else None,
                          retained_staging=[str(path) for path in retained],
                          cleanup_confirmed=False, cleanup_outcome='retained staging may be partial' if retained
                          else 'staging path absent at interrupted closeout')
            (out / 'fixture-receipt.json').write_text(json.dumps(record, indent=2) + '\n')
            print(json.dumps(record, indent=2))
    return record['exit']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--deb-receipt', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--same-code-fixture', action='store_true')
    parser.add_argument('--rpm-receipt', type=Path)
    parser.add_argument('--frozen')
    parser.add_argument('--source-date-epoch', type=int)
    parser.add_argument('--harness-commit')
    args = parser.parse_args()
    require_limits()
    if args.same_code_fixture:
        def interrupt(signum, frame):
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            raise KeyboardInterrupt('constructor received SIGTERM')
        previous = signal.signal(signal.SIGTERM, interrupt)
        try:
            return fixture_main(args)
        except KeyboardInterrupt as error:
            print(str(error) or 'constructor interrupted', file=sys.stderr)
            return 2
        finally:
            signal.signal(signal.SIGTERM, previous)
    if any(value is not None for value in (args.rpm_receipt, args.frozen, args.source_date_epoch, args.harness_commit)):
        parser.error('fixture pins require --same-code-fixture')
    out = args.out.resolve()
    out.relative_to(STORAGE / 'results')
    out.mkdir(exist_ok=False, parents=True)
    source = json.loads(args.deb_receipt.read_text())
    package = Path(source['package'])
    with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT'], prefix='candidate-rpm-input-') as scratch:
        staged = Path(scratch) / 'validated'
        stage_candidate(package, args.deb_receipt, staged)
        provenance_hash = digest(staged / 'usr/share/doc/fabrico11y/CANDIDATE-PROVENANCE.json')
    epoch = source['source']['source_date_epoch']
    env = {**os.environ, 'SOURCE_DATE_EPOCH': str(epoch)}
    command = ['bash', str(ROOT / 'packaging/build-rpm.sh'), str(out), str(package), '--candidate-deb', str(args.deb_receipt)]
    record = {'state': 'failed', 'command': command, 'exit': 2,
              'deb_receipt': str(args.deb_receipt.resolve()), 'deb_receipt_sha256': digest(args.deb_receipt),
              'deb_sha256': source['package_sha256'], 'source': source['source'],
              'rpm_builder_sha256': digest(ROOT / 'packaging/build-rpm.sh'),
              'adapter_sha256': digest(Path(__file__)), 'package_family': 'fedora'}
    try:
        record['build_exit'] = run_build(command, env, out)
        if record['build_exit']:
            raise ValueError('RPM build failed')
        packages = list(out.glob('*.rpm'))
        if len(packages) != 1:
            raise ValueError('expected exactly one candidate RPM')
        payload = assert_equivalent(package, packages[0])
        if inventory(packages[0])['usr/share/doc/fabrico11y/CANDIDATE-PROVENANCE.json']['sha256'] != provenance_hash:
            raise ValueError('RPM candidate provenance differs from exact validated build inputs')
        record.update(state='built', package=str(packages[0]), package_sha256=digest(packages[0]),
                      payload_identity=payload, candidate_provenance_sha256=provenance_hash, exit=0)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        record.update(error=str(error), exit=2)
    (out / 'receipt.json').write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps(record, indent=2))
    return record['exit']


if __name__ == '__main__':
    raise SystemExit(main())
