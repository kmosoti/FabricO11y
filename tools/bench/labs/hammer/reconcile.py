#!/usr/bin/env python3
"""Write a bounded diagnostic reconciliation without claiming strict closeout."""
import argparse
import copy
import hashlib
import json
import math
import os
from pathlib import Path
import shlex
import subprocess
import sys
import time

import job
import service


ROOT = service.ROOT
BASE = job.base.BASE
MISSING_UNIT = 'fabric-work-81d808a9c9e24167ab52f5599fd6c7e3'
MISSING_ID = 'identity-scan-02'
MISSING_PARENT = Path('/run/media/kmosoti/data/FabricO11y/scratch/work-l8v8_2fa')
WRAPPERS = {
    'tools/bench/labs/hammer/job.py',
    'tools/bench/labs/rca/job.py',
    'tools/bench/labs/rca/native_job.py',
    'tools/bench/labs/rca/matrix_job.py',
    'tools/bench/labs/hammer/resume_job.py',
}
REQUIRED = (
    'references-02', 'pressure-scan-02', 'identity-walk-01',
    'identity-scan-01', 'identity-scan-02', 'identity-walk-02',
    'edge-real-regrade-01', 'edge-sim-eight-01', 'network-01',
    'remote-reduce-two-01', 'remote-reduce-eight-01', 'identity-build-01',
    'identity-library-01', 'tail-identity-fixed-01', 'final-preflight-01',
    'final-fast-01', 'final-docs-01',
)
SERVICE_CASES = ('identity-walk-01', 'identity-scan-01', 'identity-scan-02', 'identity-walk-02')
MIB = 2**20
GIB = 2**30
MAINTENANCE_UNITS = ('fabric-work-5e426fbc62cb4da9aa1fd9fa051b2bea',
                     'fabric-work-055f6b2357be41c09fe13822d5ef20bd')


def finite_nonnegative(value):
    return (isinstance(value, (int, float)) and not isinstance(value, bool)
            and math.isfinite(value) and value >= 0)


def digest_bounded(path, deadline, max_bytes=24 * GIB):
    path = Path(path)
    size = path.stat().st_size
    if size > max_bytes:
        raise RuntimeError(f'bounded digest byte limit exceeded: {path} ({size})')
    h = hashlib.sha256()
    with path.open('rb') as stream:
        while True:
            if time.monotonic() >= deadline:
                raise TimeoutError('reconciliation deadline during file digest')
            block = stream.read(1024 * 1024)
            if not block:
                break
            h.update(block)
    return h.hexdigest()


def launcher_wrapper(command):
    return next((name for name in WRAPPERS if name in command), None)


def launcher_identity(command):
    """Read --id only from wrapper arguments, before the child's `--`."""
    wrapper = launcher_wrapper(command)
    if wrapper is None:
        return None, None
    stop = command.index('--') if '--' in command else len(command)
    prefix = command[:stop]
    try:
        index = prefix.index('--id')
    except ValueError:
        return wrapper, None
    return wrapper, prefix[index + 1] if index + 1 < len(prefix) else None


def launcher_index(run_dir):
    result = {}
    ignored = []
    for path in sorted(Path(run_dir).glob('*.json')):
        try:
            record = json.loads(path.read_text())
        except (OSError, json.JSONDecodeError) as exc:
            ignored.append(dict(path=str(path), error=repr(exc)))
            continue
        command = record.get('command', [])
        if not isinstance(command, list):
            ignored.append(dict(path=str(path), error='launcher command is not a list'))
            continue
        wrapper, name = launcher_identity(command)
        if wrapper is None:
            continue
        if name is None:
            ignored.append(dict(path=str(path), wrapper=wrapper, error='wrapper command has no --id'))
            continue
        result.setdefault(name, []).append((path, record, wrapper))
    return result, ignored


def check_launcher(name, receipt_path, receipt, candidates, deadline):
    issues = []
    if not isinstance(receipt, dict) or receipt.get('id') != name:
        issues.append('coordinator identity differs from directory')
        return dict(status='mismatch', issues=issues), issues
    if receipt.get('state') not in ('passed', 'failed'):
        issues.append('coordinator is not completed')
    code = receipt.get('exit')
    if not isinstance(code, int) or isinstance(code, bool):
        issues.append('coordinator exit is missing or invalid')
    elif (receipt.get('state') == 'passed') != (code == 0):
        issues.append('coordinator state and exit disagree')
    if not finite_nonnegative(receipt.get('elapsed_s')):
        issues.append('coordinator duration is invalid')
    limits = receipt.get('limits', {})
    if (limits.get('memory.max') != str(20 * GIB)
            or limits.get('memory.high') != str(16 * GIB)
            or limits.get('memory.swap.max') != '0'):
        issues.append('coordinator cgroup limits differ from registered limits')

    if not candidates:
        return dict(status='launcher_missing', issues=issues), issues

    matches = []
    malformed = []
    for path, launcher, wrapper in candidates:
        command = launcher.get('command', [])
        try:
            child = command[command.index('--') + 1:]
        except (ValueError, TypeError):
            malformed.append(str(path))
            continue
        if child == receipt.get('argv'):
            matches.append((path, launcher, wrapper))
    if len(matches) != 1:
        issues.append('launcher command does not uniquely match coordinator argv')
        if malformed:
            issues.append('launcher command lacks a child separator: ' + ', '.join(malformed))
        return dict(status='mismatch', issues=issues), issues

    path, launcher, wrapper = matches[0]
    command = launcher['command']
    if path.stem != launcher.get('unit'):
        issues.append('launcher receipt filename and unit identity differ')
    outer_stop = command.index('--')
    outer = command[:outer_stop]
    for flag, expected in (('--id', name), ('--lab', receipt.get('lab')),
                           ('--seconds', str(receipt.get('timeout_s')))):
        try:
            index = outer.index(flag)
            observed = outer[index + 1]
        except (ValueError, IndexError):
            observed = None
        if observed != expected:
            issues.append(f'launcher {flag} differs from coordinator receipt')
    if launcher.get('exit') != code:
        issues.append('launcher and coordinator exits differ')
    if not isinstance(launcher.get('exit'), int) or isinstance(launcher.get('exit'), bool):
        issues.append('launcher exit is missing or invalid')
    if not finite_nonnegative(launcher.get('elapsed_s')):
        issues.append('launcher duration is invalid')
    if launcher.get('memory_max_bytes') != 20 * GIB or launcher.get('swap_max_bytes') != 0:
        issues.append('launcher memory/swap limits differ from registered limits')
    if launcher.get('temporary_removed') is not True:
        issues.append('launcher temporary tree was not recorded removed')
    scratch = Path(str(receipt.get('scratch', '')))
    temporary = Path(str(launcher.get('temporary', '')))
    try:
        scratch.relative_to(service.resource_group.STORAGE / 'scratch')
    except ValueError:
        issues.append('coordinator scratch is outside mounted scratch root')
    if scratch != temporary / name:
        issues.append('coordinator scratch and launcher temporary/name differ')
    if scratch.exists():
        issues.append('coordinator scratch still exists')
    if launcher.get('retained_failure_evidence'):
        if receipt.get('state') != 'failed':
            issues.append('launcher retained failure tree for non-failed coordinator')

    active = None
    if time.monotonic() < deadline:
        try:
            check = subprocess.run(
                ['systemctl', '--user', 'is-active', str(launcher.get('unit', ''))],
                capture_output=True, text=True, timeout=min(2, max(.2, deadline-time.monotonic())))
            active = check.stdout.strip()
            if active != 'inactive':
                issues.append('launcher unit is not confirmed inactive: ' + (active or check.stderr.strip()))
        except (OSError, subprocess.TimeoutExpired) as exc:
            active = 'unknown'
            issues.append('launcher unit state unavailable: ' + repr(exc))
    else:
        active = 'unknown'
        issues.append('deadline reached before launcher unit check')

    return dict(status='authenticated' if not issues else 'mismatch',
                wrapper=wrapper, launcher_path=str(path), launcher_sha256=service.digest(path),
                launcher_unit=launcher.get('unit'), launcher_exit=launcher.get('exit'),
                launcher_elapsed_s=launcher.get('elapsed_s'), launcher_memory_max_bytes=launcher.get('memory_max_bytes'),
                launcher_swap_max_bytes=launcher.get('swap_max_bytes'), launcher_temporary=str(temporary),
                temporary_removed=launcher.get('temporary_removed'), unit_active_state=active,
                coordinator_scratch=str(scratch), coordinator_scratch_absent=not scratch.exists(),
                issues=issues), issues


def capture_missing_journal(out, receipt, deadline):
    unit = MISSING_UNIT
    raw_path = out / 'identity-scan-02-journal.jsonl'
    result = dict(unit=unit, raw_path=str(raw_path), unit_inactive=False,
                  invocation_match=False, outer_exit=None, resolved=False, issues=[])
    try:
        state = subprocess.run(['systemctl', '--user', 'is-active', unit],
            capture_output=True, text=True, timeout=min(3, max(.2, deadline-time.monotonic())))
        result['systemctl_is_active_exit'] = state.returncode
        result['systemctl_is_active_stdout'] = state.stdout.strip()
        result['unit_inactive'] = state.stdout.strip() == 'inactive'
        if not result['unit_inactive']:
            result['issues'].append('missing-launcher unit not confirmed inactive')
    except (OSError, subprocess.TimeoutExpired) as exc:
        result['issues'].append('missing-launcher unit state unavailable: ' + repr(exc))

    try:
        if time.monotonic() >= deadline - 12:
            raise TimeoutError('remote inspection reserve reached before journal capture')
        with raw_path.open('xb') as stream:
            process = subprocess.Popen(['journalctl', '--user', '--unit', unit,
                '--output=json', '--no-pager'], stdout=stream, stderr=subprocess.PIPE)
            start = time.monotonic()
            while process.poll() is None:
                if stream.tell() > 4 * 1024 * 1024:
                    process.terminate()
                    process.wait(timeout=1)
                    raise RuntimeError('journal exceeds 4 MiB bounded capture')
                if time.monotonic() >= min(deadline-12, start+8):
                    process.terminate()
                    process.wait(timeout=1)
                    raise TimeoutError('journalctl exceeded 8-second bound')
                time.sleep(.05)
            stderr = process.stderr.read(65536).decode(errors='replace') if process.stderr else ''
            stream.flush()
            result['journal_exit'] = process.returncode
            result['journal_bytes'] = raw_path.stat().st_size
            result['journal_sha256'] = service.digest(raw_path)
            if process.returncode != 0:
                result['issues'].append('journalctl failed: ' + stderr[:2000])
            raw_lines = raw_path.read_text(errors='strict').splitlines()
            events = [json.loads(line) for line in raw_lines if line]
            unit_entries = [row for row in events if row.get('USER_UNIT') == unit + '.service']
            result['journal_records'] = len(events)
            result['unit_records'] = len(unit_entries)
            markers = ['python3', '-B', 'tools/bench/labs/hammer/job.py',
                       '--id', MISSING_ID, '--lab', receipt.get('lab'),
                       '--seconds', str(receipt.get('timeout_s'))]
            expected_child = receipt.get('argv', [])
            invocation = []
            for row in unit_entries:
                message = row.get('MESSAGE', '')
                if 'Started ' not in message or ' --inside ' not in message:
                    continue
                try:
                    command_text = message.split(' --inside ', 1)[1]
                    # systemd appends a sentence period after the displayed argv.
                    # Preserve the original journal bytes; normalize only this parsed copy.
                    if command_text.endswith('.'):
                        command_text = command_text[:-1]
                    tokens = shlex.split(command_text)
                except ValueError:
                    continue
                try:
                    start_index = tokens.index('tools/bench/labs/hammer/job.py')
                except ValueError:
                    continue
                sub = tokens[start_index:]
                if tokens[max(0, start_index-2):start_index+1] != [
                        'python3', '-B', 'tools/bench/labs/hammer/job.py']:
                    continue
                if not all(value is not None for value in markers):
                    continue
                try:
                    sep = sub.index('--')
                    id_index = sub.index('--id')
                    seconds_index = sub.index('--seconds')
                    lab_index = sub.index('--lab')
                except ValueError:
                    continue
                matched = (sub[id_index:id_index+2] == ['--id', MISSING_ID]
                    and sub[lab_index:lab_index+2] == ['--lab', receipt.get('lab')]
                    and sub[seconds_index:seconds_index+2] == ['--seconds', str(receipt.get('timeout_s'))]
                    and sub[sep+1:] == expected_child)
                invocation.append(dict(unit=row.get('USER_UNIT'), invocation_id=row.get('USER_INVOCATION_ID'),
                    job_id=row.get('JOB_ID'), exact_argv_match=matched,
                    message_sha256=hashlib.sha256(message.encode()).hexdigest()))
            result['invocation_records'] = invocation
            result['invocation_match'] = any(r['exact_argv_match'] for r in invocation)
            result['journal_raw_complete'] = process.returncode == 0
            if result['unit_records'] == 0:
                result['issues'].append('journal contains no records for exact user unit')
            if not result['invocation_match']:
                result['issues'].append('journal does not authenticate expected invocation argv')
    except (OSError, subprocess.TimeoutExpired, TimeoutError, RuntimeError,
            json.JSONDecodeError, UnicodeDecodeError) as exc:
        result['issues'].append('journal capture/parse failed: ' + repr(exc))
        if raw_path.exists():
            result['journal_bytes'] = raw_path.stat().st_size
            result['journal_sha256'] = service.digest(raw_path)
            result['journal_raw_complete'] = False

    parent_record = dict(path=str(MISSING_PARENT), existed=MISSING_PARENT.exists(),
                         empty=False, removed=False, checked_only_when_unit_inactive=result['unit_inactive'])
    if MISSING_PARENT.exists():
        try:
            if MISSING_PARENT.is_symlink() or not MISSING_PARENT.is_dir():
                raise RuntimeError('exact scratch parent is not a real directory')
            contents = sorted(p.name for p in MISSING_PARENT.iterdir())
            parent_record['contents'] = contents
            parent_record['empty'] = not contents
            if result['unit_inactive'] and not contents:
                current = subprocess.run(['systemctl', '--user', 'is-active', unit],
                    capture_output=True, text=True, timeout=min(3, max(.2, deadline-time.monotonic())))
                parent_record['unit_state_before_removal'] = current.stdout.strip()
                if current.stdout.strip() != 'inactive':
                    raise RuntimeError('exact unit no longer confirmed inactive before removal')
                MISSING_PARENT.rmdir()
                parent_record['removed'] = not MISSING_PARENT.exists()
                if not parent_record['removed']:
                    raise RuntimeError('exact scratch parent still exists after rmdir')
            else:
                parent_record['issues'] = ['scratch parent retained because unit is active/unknown or directory is nonempty']
        except (OSError, subprocess.TimeoutExpired) as exc:
            parent_record['issues'] = ['scratch parent inspection/removal failed: ' + repr(exc)]
        except RuntimeError as exc:
            parent_record['issues'] = [str(exc)]
    else:
        parent_record.update(empty=True, removed=False, already_absent=True)
    result['scratch_parent'] = parent_record
    if not parent_record['empty'] or parent_record.get('issues'):
        result['issues'].extend(parent_record.get('issues', ['scratch parent not empty']))
    result['current_cleanup'] = dict(unit_inactive=result['unit_inactive'],
        parent_empty_before_removal=parent_record['empty'], parent_removed_now=parent_record['removed'])
    return result


def remote_inspection(deadline):
    code = r'''import glob,json,socket,subprocess
units=subprocess.run(['systemctl','list-units','--all','--plain','--no-legend','fabric-edge-*'],capture_output=True,text=True,timeout=3)
print(json.dumps({'hostname':socket.gethostname(),'identity':subprocess.run(['id','-un'],capture_output=True,text=True,timeout=2).stdout.strip(),'scratch':sorted(glob.glob('/var/tmp/fabric-hammer-*')),'units':units.stdout,'units_exit':units.returncode,'units_error':units.stderr[:1000]}))'''
    try:
        result = subprocess.run(['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=5',
            'digitalocean-02', shlex.join(['python3', '-c', code])], capture_output=True, text=True,
            timeout=min(10, max(.2, deadline-time.monotonic())))
        if result.returncode != 0:
            return dict(state='error', exit=result.returncode, stdout=result.stdout[:2000], stderr=result.stderr[:2000])
        if len(result.stdout.encode()) > 1024 * 1024:
            return dict(state='error', error='remote inspection output exceeds 1 MiB')
        remote = json.loads(result.stdout)
        remote['state'] = 'observed'
        remote['expected_host'] = remote.get('hostname') == 'digitalocean-02'
        remote['expected_identity'] = remote.get('identity') == 'dev'
        remote['cleanup_clear'] = not remote.get('scratch') and not remote.get('units', '').strip()
        if remote['units_exit'] != 0:
            remote['state'] = 'error'
        return remote
    except (OSError, subprocess.TimeoutExpired, json.JSONDecodeError) as exc:
        return dict(state='unknown', error=repr(exc))


def preservation_inventory(deadline, issues, launcher_sha_by_unit):
    entries = []
    summary_paths = sorted((BASE / 'memory').glob('failure-preservation-*/summary.json'))
    for summary_path in summary_paths:
        item = dict(summary_path=str(summary_path))
        try:
            item['summary_sha256'] = service.digest(summary_path)
            records = json.loads(summary_path.read_text())
            item['records'] = []
            for record in records:
                row = {key: record.get(key) for key in (
                    'unit', 'source', 'archive', 'archive_sha256', 'members', 'original_bytes',
                    'archive_bytes', 'launcher_receipt_sha256', 'removed')}
                archive = Path(str(record.get('archive', '')))
                source = Path(str(record.get('source', '')))
                if not archive.exists():
                    pointer = archive.with_name('failure.location.json')
                    location = json.loads(pointer.read_text())
                    target = Path(location['archive'])
                    expected = service.resource_group.STORAGE / 'evidence/hammer-reference-01/failure-preservation-01' / (record['unit']+'.tar.gz')
                    if (pointer.is_symlink() or target.is_symlink() or target != expected
                            or Path(location['original_archive']).absolute() != archive.absolute()
                            or location['archive_sha256'] != record['archive_sha256']
                            or location['archive_bytes'] != record['archive_bytes']
                            or target.stat().st_size != record['archive_bytes']):
                        raise RuntimeError('archive location does not bind original identity')
                    row['location_record'] = str(pointer)
                    row['location_sha256'] = service.digest(pointer)
                    row['relocated_archive'] = str(target)
                    archive = target
                if not record.get('removed'):
                    row['verification'] = 'failed: original tree not recorded removed'
                    issues.append(f'failure archive source remains or removal unconfirmed: {record.get("unit")}')
                elif source.exists():
                    row['verification'] = 'failed: original source tree still exists'
                    issues.append(f'failure archive original source still exists: {record.get("unit")}')
                elif not archive.is_file():
                    row['verification'] = 'failed: archive missing'
                    issues.append(f'failure archive missing: {record.get("unit")}')
                elif launcher_sha_by_unit.get(record.get('unit')) != record.get('launcher_receipt_sha256'):
                    row['verification'] = 'failed: launcher receipt identity differs'
                    issues.append(f'failure preservation launcher identity mismatch: {record.get("unit")}')
                elif time.monotonic() >= deadline:
                    row['verification'] = 'unknown: deadline before archive digest'
                    issues.append(f'failure archive digest not completed before deadline: {record.get("unit")}')
                else:
                    actual = digest_bounded(archive, deadline, max_bytes=3 * GIB)
                    row['actual_archive_sha256'] = actual
                    row['verification'] = 'matched' if actual == record.get('archive_sha256') else 'mismatch'
                    if actual != record.get('archive_sha256'):
                        issues.append(f'failure archive hash mismatch: {record.get("unit")}')
                item['records'].append(row)
        except (OSError, RuntimeError, TimeoutError, json.JSONDecodeError) as exc:
            item['error'] = repr(exc)
            issues.append(f'failure preservation inventory error: {summary_path}: {exc!r}')
        entries.append(item)
    return entries


def service_summaries():
    result = {}
    for name in SERVICE_CASES:
        path = BASE / 'memory' / name / 'summary.json'
        if not path.is_file():
            result[name] = dict(status='absent', path=str(path))
            continue
        try:
            summary = json.loads(path.read_text())
            gates = summary.get('gates', {})
            result[name] = dict(path=str(path), sha256=service.digest(path),
                passed=summary.get('passed'), all_gates_true=bool(gates) and all(v is True for v in gates.values()),
                gates=gates, seed=summary.get('seed'), recovered_logs=summary.get('recovered_logs'),
                acknowledged_batches=summary.get('acknowledged_batches'),
                server_peak_rss_mib=summary.get('server_peak_rss_mib'),
                server_cpu_seconds=summary.get('server_cpu_seconds'),
                encoded_batch_bytes=summary.get('encoded_batch_bytes'))
        except (OSError, json.JSONDecodeError) as exc:
            result[name] = dict(status='error', path=str(path), error=repr(exc))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    service.resource_group.require_limits()
    started = time.monotonic()
    deadline = started + 49
    local_deadline = deadline - 10
    out = args.out.resolve()
    expected_parent = (BASE / 'coordinator').resolve()
    expected_out = expected_parent / Path(os.environ['FABRIC_SCRATCH_ROOT']).name / 'reconciliation'
    if out != expected_out or not out.parent.is_dir():
        parser.error('--out must be coordinator/<own-job-id>/reconciliation')
    out.mkdir(parents=False, exist_ok=False)
    own = Path(os.environ['FABRIC_SCRATCH_ROOT']).name
    issues = []
    unresolved = []
    report = dict(protocol=str(ROOT / 'docs/experiments/benchmarks/pressure-resumption-protocol.md'),
        protocol_sha256=service.digest(ROOT / 'docs/experiments/benchmarks/pressure-resumption-protocol.md'),
        created_unix_ns=time.time_ns(), own_running_job_excluded=own,
        strict_closeout='unrun: original strict checker cannot accept missing outer launcher evidence',
        launcher_jobs={}, ignored_launcher_records=[], missing_launcher=None, required_jobs={},
        failure_preservation=[], service_summaries={}, accounting=None, remote_cleanup=None,
        state='incomplete', errors=issues, unresolved=unresolved)
    try:
        launchers, ignored = launcher_index(service.ROOT / 'target/resource-containment/runs')
        report['ignored_launcher_records'] = ignored
        coordinator_dir = BASE / 'coordinator'
        paths = sorted(coordinator_dir.glob('*/receipt.json'))
        for path in paths:
            name = path.parent.name
            if name == own:
                continue
            if time.monotonic() >= local_deadline:
                unresolved.append('local reconciliation inspection stopped to preserve remote timeout reserve')
                break
            try:
                receipt = json.loads(path.read_text())
            except (OSError, json.JSONDecodeError) as exc:
                issues.append(f'coordinator receipt unreadable: {path}: {exc!r}')
                continue
            candidates = launchers.get(name, [])
            status, item_issues = check_launcher(name, path, receipt, candidates, local_deadline)
            row = dict(coordinator_path=str(path), coordinator_sha256=service.digest(path),
                state=receipt.get('state'), exit=receipt.get('exit'), elapsed_s=receipt.get('elapsed_s'),
                argv=receipt.get('argv'), lab=receipt.get('lab'), scratch=receipt.get('scratch'),
                limits=receipt.get('limits'), **status)
            if name == MISSING_ID:
                unresolved.append('identity-scan-02 original outer launcher receipt was absent; this report does not authenticate a reconstructed replacement')
                row['issues'] = list(item_issues)
                if not candidates:
                    row['status'] = 'unresolved_missing_original_launcher'
                    row['issues'].append('original launcher receipt absent; journal may identify invocation but cannot supply outer exit/cleanup receipt')
                elif status.get('status') == 'authenticated' and row.get('launcher_unit') == MISSING_UNIT:
                    row['status'] = 'unresolved_candidate_for_missing_original'
                else:
                    row['status'] = 'unresolved_candidate_launcher_mismatch'
                    row['issues'].append('candidate launcher does not establish the expected original unit identity')
                    issues.extend(f'{name}: {problem}' for problem in item_issues)
            if item_issues:
                issues.extend(f'{name}: {problem}' for problem in item_issues)
            elif status.get('status') == 'launcher_missing':
                unresolved.append(f'launcher receipt absent for completed coordinator case: {name}')
                if name != MISSING_ID:
                    row['issues'] = item_issues
            elif status.get('status') == 'mismatch':
                issues.extend(f'{name}: {problem}' for problem in item_issues)
            report['launcher_jobs'][name] = row

        existing = set(report['launcher_jobs'])
        retries = {'remote-reduce-two-01': 'remote-reduce-two-02',
                   'remote-reduce-eight-01': 'remote-reduce-eight-02',
                   'final-preflight-01': 'final-preflight-02'}
        report['separate_retry_outcomes'] = {}
        for name in REQUIRED:
            if name in existing:
                report['required_jobs'][name] = report['launcher_jobs'][name]['state']
                if report['required_jobs'][name] != 'passed':
                    retry = retries.get(name)
                    later = report['launcher_jobs'].get(retry, {})
                    report['separate_retry_outcomes'][name] = dict(retry=retry,
                        state=later.get('state'), authentication=later.get('status'),
                        original_outcome_unchanged=True)
                    if later.get('state') != 'passed' or later.get('status') != 'authenticated':
                        unresolved.append(f'required closeout case has no successful authenticated retry: {name}')
            else:
                report['required_jobs'][name] = 'unrun_or_receipt_absent'
                unresolved.append(f'required closeout case not recorded: {name}')

        missing_path = coordinator_dir / MISSING_ID / 'receipt.json'
        if missing_path.is_file():
            missing_receipt = json.loads(missing_path.read_text())
            report['missing_launcher'] = capture_missing_journal(out, missing_receipt, deadline)
            journal = report['missing_launcher']
            issues.extend('identity-scan-02 journal: ' + issue for issue in journal['issues'])
            if journal['invocation_match'] and journal['unit_inactive'] and journal.get('journal_raw_complete'):
                if report['launcher_jobs'].get(MISSING_ID, {}).get('status') != 'authenticated':
                    unresolved.append('identity-scan-02 journal confirms invocation but no authenticated original launcher receipt')
        else:
            unresolved.append('identity-scan-02 coordinator receipt itself is absent')

        launcher_sha_by_unit = {launcher.get('unit'): service.digest(path)
            for cases in launchers.values() for path, launcher, _wrapper in cases}
        maintenance = []
        for unit in MAINTENANCE_UNITS:
            path = ROOT / 'target/resource-containment/runs' / (unit+'.json')
            receipt = json.loads(path.read_text())
            if (receipt['unit'] != unit or receipt['command'] != ['python3', '-B', 'tools/bench/labs/hammer/archive_references.py']
                    or not finite_nonnegative(receipt['elapsed_s'])
                    or receipt['memory_max_bytes'] != 20*GIB or receipt['swap_max_bytes'] != 0
                    or not receipt['temporary_removed']):
                raise RuntimeError('maintenance launcher identity/limits/cleanup differ')
            launcher_sha_by_unit[unit] = service.digest(path)
            maintenance.append(dict(unit=unit, path=str(path), sha256=service.digest(path),
                                    elapsed_s=receipt['elapsed_s'], exit=receipt['exit'],
                                    retained_failure_evidence=receipt.get('retained_failure_evidence')))
        report['separately_charged_maintenance'] = maintenance
        report['failure_preservation'] = preservation_inventory(local_deadline, issues, launcher_sha_by_unit)
        preserved_units = {record.get('unit') for summary in report['failure_preservation']
            for record in summary.get('records', [])}
        for item in maintenance:
            if item['retained_failure_evidence'] and item['unit'] not in preserved_units:
                issues.append('maintenance failure lacks preservation: '+item['unit'])
        external_bytes = sum(record['archive_bytes'] for summary in report['failure_preservation']
            for record in summary.get('records', []) if record.get('relocated_archive'))
        local_bytes = job.base.footprint(BASE/'memory')
        report['archive_storage_accounting'] = dict(external_archive_bytes=external_bytes,
            local_memory_evidence_bytes=local_bytes, combined_bytes=external_bytes+local_bytes,
            cap_bytes=job.base.CAPS['memory']*MIB)
        if external_bytes+local_bytes > job.base.CAPS['memory']*MIB:
            issues.append('combined local/external memory evidence exceeds original cap')
        for name, row in report['launcher_jobs'].items():
            launcher = next((v[1] for v in launchers.get(name, []) if str(v[0]) == row.get('launcher_path')), None)
            if launcher and launcher.get('retained_failure_evidence') and launcher.get('unit') not in preserved_units:
                issues.append(f'{name}: failed launcher tree lacks authenticated preservation receipt')

        # Validate preservation helper provenance after checking all records.
        preserve_path = ROOT / 'tools/bench/labs/hammer/preserve.py'
        report['failure_preservation_source'] = dict(path=str(preserve_path), sha256=service.digest(preserve_path))
        report['service_summaries'] = service_summaries()
        for name, summary in report['service_summaries'].items():
            if summary.get('passed') is not True or summary.get('all_gates_true') is not True:
                unresolved.append(f'service semantic gates not confirmed: {name}')

        control_name = 'identity-walk-02'
        control_path = coordinator_dir / control_name / 'receipt.json'
        good = json.loads(control_path.read_text())
        report['receipt_rejection_controls'] = []
        for label, key, value in [('exit_mismatch', 'exit', 1),
                                  ('invalid_duration', 'elapsed_s', -1),
                                  ('wrong_limits', 'limits', {})]:
            bad = copy.deepcopy(good)
            bad[key] = value
            verdict, defects = check_launcher(control_name, control_path, bad,
                launchers.get(control_name, []), local_deadline)
            if verdict['status'] != 'mismatch' or not defects:
                raise RuntimeError('receipt control was admitted: ' + label)
            report['receipt_rejection_controls'].append(dict(defect=label, rejected=True, reasons=defects))

        try:
            ledgers, _ = job.load()
            ledgers['native-frontier-01'] = [r for r in ledgers['native-frontier-01'] if r.get('id') != own]
            report['accounting'] = dict(cumulative_excluding_own=job.totals(ledgers),
                                        excluded_running_id=own)
            extra = sum(item['elapsed_s'] for item in maintenance)
            report['accounting']['additional_maintenance_s'] = extra
            report['accounting']['remaining_round_s_after_maintenance'] = job.totals(ledgers)['remaining_round_s']-extra
            if report['accounting']['remaining_round_s_after_maintenance'] < 0:
                issues.append('round duration including maintenance exceeds allocation')
        except Exception as exc:
            unresolved.append('cumulative accounting unavailable: ' + repr(exc))

        report['remote_cleanup'] = remote_inspection(deadline)
        remote = report['remote_cleanup']
        if remote.get('state') != 'observed':
            unresolved.append('remote cleanup inspection unavailable or errored')
        elif not (remote.get('expected_host') and remote.get('expected_identity') and remote.get('cleanup_clear')):
            issues.append('remote cleanup host, identity or residue check did not match expected state')
    except Exception as exc:
        issues.append('reconciliation inspection failed: ' + repr(exc))

    report['elapsed_s'] = time.monotonic() - started
    report['state'] = 'complete' if not issues and not unresolved else 'incomplete'
    report['exit_semantics'] = '0 means diagnostic report produced; it does not mean strict closeout passed'
    try:
        (out / 'summary.json').write_text(json.dumps(report, indent=2) + '\n')
        (out / 'reconcile.py').write_bytes(Path(__file__).read_bytes())
    except Exception as exc:
        print(json.dumps(dict(state='incomplete', report_write_error=repr(exc), errors=issues)), file=sys.stderr)
        return 1
    print(json.dumps(dict(state=report['state'], jobs=len(report['launcher_jobs']),
        unresolved=len(unresolved), errors=len(issues), elapsed_s=report['elapsed_s'])))
    return 1 if issues else 0


if __name__ == '__main__':
    raise SystemExit(main())
