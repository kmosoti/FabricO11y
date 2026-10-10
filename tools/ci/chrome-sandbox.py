"""Diagnose hosted Chrome startup; permit only its exact Ubuntu userns attachment.

Ubuntu's documented solution preserves Chromium's own sandbox:
https://discourse.ubuntu.com/t/ubuntu-24-04-lts-noble-numbat-release-notes/39890
The first attempt and stderr remain evidence even when the narrow retry succeeds.
"""
import argparse
import hashlib
import html
import json
import os
from pathlib import Path
import re
import resource
import shutil
import signal
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import require_limits

DATA = Path('/run/media/kmosoti/data/FabricO11y')
CHROME = DATA / 'tools/ui/chrome-for-testing-155.0.8059.39/chrome-linux64/chrome'
OUT = DATA / 'results/ci-chrome-sandbox'
PROFILE = OUT / 'apparmor.profile'


def profile_text(binary):
    if binary != CHROME or binary.is_symlink():
        raise RuntimeError('AppArmor attachment must be the exact pinned Chrome path')
    return ('abi <abi/4.0>,\ninclude <tunables/global>\n'
            f'profile fabrico11y-ci-chrome {binary} flags=(unconfined) {{\n  userns,\n}}\n')


def restricted_sandbox_failure(stderr, restriction):
    return restriction == '1' and ('No usable sandbox!' in stderr or
                                  'apparmor_restrict_unprivileged_userns' in stderr)


def sandbox_enabled(document):
    text = html.unescape(re.sub('<[^>]+>', ' ', document))
    return all(re.search(label + r'\s+Yes\b', text) for label in
               ('PID namespaces', 'Network namespaces', 'Seccomp-BPF sandbox'))


def attempt(label):
    profile = Path(tempfile.mkdtemp(prefix='ci-chrome-' + label + '-', dir=os.environ['FABRIC_SCRATCH_ROOT']))
    command = [str(CHROME), '--headless=new', '--disable-dev-shm-usage',
               '--no-first-run', '--no-default-browser-check',
               '--user-data-dir=' + str(profile), '--enable-logging=stderr',
               '--allow-chrome-scheme-url', '--dump-dom', 'chrome://sandbox']
    with (OUT / (label + '.stdout')).open('w') as stdout, (OUT / (label + '.stderr')).open('w') as stderr:
        process = subprocess.Popen(command, stdout=stdout, stderr=stderr, start_new_session=True)
        try:
            code = process.wait(timeout=30)
        finally:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
            shutil.rmtree(profile)
    return {'command': command, 'exit_code': code,
            'namespace_and_seccomp_enabled': sandbox_enabled((OUT / (label + '.stdout')).read_text())}


def main():
    require_limits()
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    parser = argparse.ArgumentParser()
    parser.add_argument('--cleanup', action='store_true')
    args = parser.parse_args()
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise SystemExit('hosted CI only; refuse local AppArmor mutation')
    if args.cleanup:
        if PROFILE.exists():
            subprocess.run(['sudo', '-n', 'apparmor_parser', '-R', str(PROFILE)], check=True)
            PROFILE.unlink()
        return
    OUT.mkdir(parents=True, exist_ok=False)
    restriction_path = Path('/proc/sys/kernel/apparmor_restrict_unprivileged_userns')
    restriction = restriction_path.read_text().strip() if restriction_path.exists() else 'unavailable'
    receipt = {'state': 'failed', 'binary': str(CHROME),
               'binary_sha256': hashlib.sha256(CHROME.read_bytes()).hexdigest(),
               'apparmor_restrict_unprivileged_userns': restriction, 'attempts': []}
    try:
        receipt['attempts'].append(attempt('original'))
        first = receipt['attempts'][0]
        if first['exit_code'] != 0 and restricted_sandbox_failure((OUT / 'original.stderr').read_text(), restriction):
            # Exact attachment, no wildcard, global sysctl change, or disabled Chrome sandbox.
            PROFILE.write_text(profile_text(CHROME))
            subprocess.run(['sudo', '-n', 'apparmor_parser', '-r', str(PROFILE)], check=True)
            receipt['apparmor_profile'] = PROFILE.read_text()
            receipt['attempts'].append(attempt('exact-profile'))
        last = receipt['attempts'][-1]
        if last['exit_code'] != 0 or not last['namespace_and_seccomp_enabled']:
            raise RuntimeError('Chrome sandbox preflight failed; inspect archived stdout/stderr')
        receipt['state'] = 'passed'
    except BaseException as error:
        receipt['error'] = str(error)
        raise
    finally:
        (OUT / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')


if __name__ == '__main__':
    main()
