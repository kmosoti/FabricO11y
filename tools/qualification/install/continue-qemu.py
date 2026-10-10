#!/usr/bin/env python3
"""Registered continuation: L2/removal only; original failed trial stays failed."""
from __future__ import annotations
import argparse
import importlib.util
import json
import re
import shutil
import subprocess
import time
from pathlib import Path

SPEC = importlib.util.spec_from_file_location('qemu_install', Path(__file__).with_name('run-qemu.py'))
Q = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(Q)


def validate_prior(prior: Path):
    receipt = json.loads((prior / 'receipt.json').read_text())
    text = (prior / 'acceptance.txt').read_text()
    if (receipt['acceptance_exit'] != 2 or receipt['guest_acceptance_exit'] != 0
            or receipt['error'] != 'RuntimeError: guest did not complete a distinct reboot'
            or receipt['package_family'] != 'fedora' or receipt['mutation'] is not None
            or 'ACCEPT L1 PASS' not in text or re.search(r'^ACCEPT \S+ (FAIL|NOT-RUN)', text, re.M)
            or not receipt['qemu_process_cleanup_confirmed']):
        raise ValueError('prior trial is not the registered stopped-reboot continuation case')
    for key, digest_key in [('package', 'package_sha256'), ('upgrade_from', 'upgrade_from_sha256')]:
        if Q.digest(Path(receipt[key]), 'sha256') != receipt[digest_key]:
            raise ValueError('prior package identity changed')
    if Q.digest(prior / 'sources/lifecycle.sh', 'sha256') != receipt['lifecycle_sha256']:
        raise ValueError('frozen lifecycle helper changed')
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prior', required=True, type=Path)
    parser.add_argument('--run-id', required=True)
    args = parser.parse_args()
    if not re.fullmatch(r'[a-z0-9][a-z0-9-]{0,90}', args.run_id):
        raise ValueError('invalid run identity')
    prior = args.prior.resolve()
    prior.relative_to(Q.DATA / 'results/installation-qemu')
    saved = validate_prior(prior)
    old_work = Path(saved['scratch_retained']).resolve()
    old_work.relative_to(Q.DATA / 'scratch')
    old_disk = old_work / 'guest.qcow2'
    admission = Q.require_data_budget()
    containment = Q.require_containment()
    result = Q.DATA / 'results/installation-qemu' / args.run_id
    work = Q.DATA / 'scratch' / ('installation-qemu-' + args.run_id)
    result.mkdir(exist_ok=False)
    work.mkdir(exist_ok=False)
    facts = {'run_id': args.run_id, 'classification': 'stopped-trial L2/removal continuation; not final candidate qualification',
             'prior_run_id': saved['run_id'], 'prior_receipt_sha256': Q.digest(prior / 'receipt.json', 'sha256'),
             'prior_acceptance_sha256': Q.digest(prior / 'acceptance.txt', 'sha256'),
             'prior_overlay': str(old_disk), 'prior_overlay_sha256': Q.digest(old_disk, 'sha256'),
             'source_snapshot_commit': saved['source_snapshot_commit'],
             'package_sha256': saved['package_sha256'], 'predecessor_sha256': saved['upgrade_from_sha256'],
             'lifecycle_sha256': saved['lifecycle_sha256'], 'host_cgroup': containment,
             'storage_admission': admission, 'phases': {}, 'exit': 2}
    # Preserve the exact effective harness too, separate from historical source.
    shutil.copy2(__file__, result / 'continue-qemu.py')
    shutil.copy2(Path(__file__).with_name('run-qemu.py'), result / 'run-qemu.py')
    process = None
    started = time.monotonic()
    try:
        chain = json.loads(Q.run([Q.executable('qemu-img'), 'info', '--backing-chain', '--output=json', str(old_disk)], timeout=30).stdout)
        if len(chain) != 2:
            raise ValueError('unexpected historical overlay backing chain')
        backing = Path(chain[1]['filename']).resolve()
        backing.relative_to(Q.DATA / 'cache/installation-qemu')
        if Q.digest(backing, saved['image_digest_algorithm']) != saved['image_digest']:
            raise ValueError('official backing image changed')
        facts['backing_chain'] = chain
        Q.run([Q.executable('qemu-img'), 'create', '-q', '-f', 'qcow2', '-F', 'qcow2', '-b', str(old_disk), str(work / 'guest.qcow2')], timeout=30)
        for name in ('seed.iso', 'id_ed25519'):
            shutil.copy2(old_work / name, work / name)
        port = Q.free_port()
        command = [Q.executable('qemu-system-x86_64'), '-accel', 'tcg,thread=multi', '-cpu', 'max', '-smp', '2', '-m', str(Q.GUEST_MEMORY_MIB),
                   '-drive', f'file={work / "guest.qcow2"},if=virtio,format=qcow2',
                   '-drive', f'file={work / "seed.iso"},media=cdrom,readonly=on',
                   '-netdev', f'user,id=net0,hostfwd=tcp:127.0.0.1:{port}-:22',
                   '-device', 'virtio-net-pci,netdev=net0', '-display', 'none',
                   '-serial', f'file:{result / "serial.txt"}', '-monitor', 'none']
        with (result / 'qemu-stderr.txt').open('wb') as stream:
            process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=stream)
        ssh = Q.ssh_base(port, work / 'id_ed25519', work / 'known_hosts')
        deadline = time.monotonic() + Q.BOOT_TIMEOUT_S
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError('continuation QEMU stopped before SSH')
            try:
                probe = Q.changed_boot(ssh)
                if probe.returncode == 0:
                    break
            except subprocess.TimeoutExpired:
                pass
            time.sleep(2)
        else:
            raise RuntimeError('distinct privileged boot witness not observed')
        witness = Q.run([*ssh, 'sudo', 'bash', '-s'], input_text='set -e\nfor f in /proc/sys/kernel/random/boot_id /root/accept/lifecycle-boot-id /root/accept/lifecycle-config.sha256 /root/accept/lifecycle-account /root/accept/lifecycle-identity.json; do echo "WITNESS $f"; cat "$f"; echo; done\nsha256sum /home/accept/lifecycle.sh /home/accept/fabrico11y.deb /home/accept/fabrico11y-previous.pkg\n', timeout=30)
        (result / 'pre-phase-witness.txt').write_text(witness.stdout)
        # Validate transferred helpers/payload identities before privileged phases.
        for identity in (saved['lifecycle_sha256'], saved['package_sha256'], saved['upgrade_from_sha256']):
            if not re.search(r'^' + identity + r'\s+/', witness.stdout, re.M):
                raise ValueError('installed guest fixture identity mismatch')
        facts['pre_phase_witness_sha256'] = Q.digest(result / 'pre-phase-witness.txt', 'sha256')
        for phase in ('reboot', 'remove'):
            call = Q.run([*ssh, 'sudo', 'bash', '/home/accept/lifecycle.sh', phase, 'fedora', '/home/accept/fabrico11y.deb', '/home/accept/fabrico11y-previous.pkg'], timeout=180, check=False)
            (result / (phase + '.txt')).write_text(call.stdout + call.stderr)
            facts['phases'][phase] = {'exit': call.returncode, 'log_sha256': Q.digest(result / (phase + '.txt'), 'sha256')}
            required = ('L2',) if phase == 'reboot' else ('A13a', 'A13b', 'F4')
            if call.returncode or any(f'ACCEPT {label} PASS' not in call.stdout for label in required):
                raise RuntimeError(f'continuation {phase} failed, exit={call.returncode}')
        facts['exit'] = 0
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as exc:
        facts['error'] = f'{type(exc).__name__}: {exc}'
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)
        facts['qemu_process_cleanup_confirmed'] = process is None or process.poll() is not None
        facts['prior_overlay_unchanged'] = Q.digest(old_disk, 'sha256') == facts['prior_overlay_sha256']
        if not facts['prior_overlay_unchanged']:
            facts['exit'] = 2
            facts['error'] = 'original stopped overlay changed'
        facts['elapsed_seconds'] = round(time.monotonic() - started, 3)
        if facts['exit'] == 0 and facts['qemu_process_cleanup_confirmed']:
            shutil.rmtree(work)
        facts['scratch_removed'] = not work.exists()
        facts['scratch_retained'] = str(work) if work.exists() else None
        (result / 'receipt.json').write_text(json.dumps(facts, indent=2) + '\n')
        print(json.dumps(facts, indent=2))
    return facts['exit']


if __name__ == '__main__':
    raise SystemExit(main())
