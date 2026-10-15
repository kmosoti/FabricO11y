"""Finite supervised Firefox TLS trust controls, with no owner enrollment."""
import argparse
import ctypes
import hashlib
import http.server
import json
import os
from pathlib import Path
import shutil
import signal
import ssl
import subprocess
import sys
import tempfile
import threading
import time

from console_bridge import ConsoleBridge
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'ui'))
import browser_supervisor


def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def verify_tool_pins(pins):
    root = Path(pins['firefox_root'])
    actual = sorted(str(path) for path in root.rglob('*') if path.is_file() or path.is_symlink())
    if actual != pins['firefox_files']:
        raise ValueError('Firefox installation inventory changed')
    for path, target in pins['links'].items():
        if not Path(path).is_symlink() or os.readlink(path) != target:
            raise ValueError('Firefox installation link changed')
    for path, expected in pins['tools'].items():
        if sha(path) != expected or Path(path).is_symlink():
            raise ValueError('Firefox tool identity changed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case', required=True, choices=('trusted', 'wrong-san', 'untrusted-ca'))
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--pins', type=Path, required=True)
    parser.add_argument('--pins-sha256', required=True)
    args = parser.parse_args()
    browser = browser_supervisor.browser
    limits = browser.require_limits()
    require = browser_supervisor.require
    require(sha(args.pins) == args.pins_sha256, 'trust-tool-pins')
    pins = json.loads(args.pins.read_text())
    verify_tool_pins(pins)
    subprocess.run(['systemctl', '--user', 'set-property', '--runtime', limits.name,
                    'MemoryHigh=5G', 'MemoryMax=6G', 'MemorySwapMax=0', 'CPUQuota=200%', 'TasksMax=1536'], check=True, timeout=15)
    require(not args.out.exists() and args.out.parent.resolve().is_relative_to(browser.DATA / 'results'), 'trust-output')
    args.out.mkdir(mode=0o700)
    root = Path(tempfile.mkdtemp(prefix='fftrust-', dir=os.environ['FABRIC_SCRATCH_ROOT']))
    end = time.monotonic() + 180
    report = dict(case=args.case, exit=2, classification='finite TLS trust mechanism; no enrollment/package/physical browser acceptance', tool_pins_sha256=args.pins_sha256)
    old_alarm = signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(TimeoutError('Firefox TLS control180s')))
    signal.alarm(180)
    try:
        os.waitid(os.P_ALL, 0, os.WEXITED | os.WNOHANG | os.WNOWAIT | 0x40000000)
    except ChildProcessError:
        pass
    else:
        raise RuntimeError('owned TLS guardian starts with foreign children')
    libc, previous = ctypes.CDLL(None), ctypes.c_int()
    require(libc.prctl(37, ctypes.byref(previous), 0, 0, 0) == 0 and libc.prctl(36, 1, 0, 0, 0) == 0, 'trust-subreaper')
    parent = budget = server = loop = instance = None
    clean = False
    def command(argv):
        subprocess.run(argv, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=15)
    try:
        browser.cgroups.delegate()
        browser.cgroups.set_limits(limits / 'supervisor', 6 * 2**30, 5 * 2**30, 1536, 2)
        parent = browser.browser_hold_directory(limits)
        parent.update(namespaces=browser.browser_namespaces(), deadline_monotonic_ns=int(end * 1e9))
        path, _ = browser.cgroups.subgroup(limits, 'firefox-trust-budget', 2**30, 850 * 2**20, 512, 2)
        budget = browser.browser_hold_directory(path); budget['exclusive'] = True
        report['budget_limits'] = browser_supervisor.budget_limits(budget)
        for name in ('ca', 'decoy'):
            command(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', str(root / (name + '.key')),
                     '-out', str(root / (name + '.pem')), '-days', '1', '-subj', '/CN=Owned Firefox ' + name,
                     '-addext', 'basicConstraints=critical,CA:TRUE'])
        hostname = 'guide.fabric.test'
        san = 'wrong.fabric.test' if args.case == 'wrong-san' else hostname
        command(['openssl', 'req', '-new', '-newkey', 'rsa:2048', '-nodes', '-keyout', str(root / 'server.key'),
                 '-out', str(root / 'server.csr'), '-subj', '/CN=' + san])
        (root / 'san.ext').write_text('subjectAltName=DNS:' + san + '\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n')
        command(['openssl', 'x509', '-req', '-in', str(root / 'server.csr'), '-CA', str(root / 'ca.pem'), '-CAkey', str(root / 'ca.key'),
                 '-CAcreateserial', '-out', str(root / 'server.pem'), '-days', '1', '-extfile', str(root / 'san.ext')])
        public = args.out / 'public-certificates'; public.mkdir(mode=0o700)
        report['public_certificates'] = {}
        for name in ('ca.pem', 'decoy.pem', 'server.pem', 'san.ext'):
            shutil.copyfile(root / name, public / name)
            report['public_certificates'][name] = sha(public / name)
        requests = []
        class Page(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                if len(requests) < 16: requests.append(self.path)
                raw = b'<!doctype html><title>Owned TLS mechanism control</title><script>window.fabricPasskey=()=>{throw Error("no enrollment in TLS control")}</script>'
                self.send_response(200); self.send_header('Content-Type', 'text/html'); self.send_header('Content-Length', str(len(raw))); self.end_headers(); self.wfile.write(raw)
            def log_message(self, *_):
                pass
        server = http.server.HTTPServer(('127.0.0.1', 0), Page)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); context.load_cert_chain(root / 'server.pem', root / 'server.key')
        server.socket = context.wrap_socket(server.socket, server_side=True)
        loop = threading.Thread(target=server.serve_forever); loop.start()
        origin = 'https://' + hostname + ':' + str(server.server_port)
        instance = ConsoleBridge.__new__(ConsoleBridge)
        error = None
        try:
            instance.__init__(origin, root / 'server.pem', root / 'profile-work', args.out / 'bridge', browser_group=budget['path'],
                browser_parent=budget, browser_deadline=end, backend='firefox',
                trusted_ca=root / ('decoy.pem' if args.case == 'untrusted-ca' else 'ca.pem'), private_dns=hostname)
        except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as failure:
            error = type(failure).__name__ + ': ' + str(failure)
        if args.case == 'trusted':
            require(error is None, 'trusted-positive-startup')
            actual = instance._js('return {origin:location.origin,secure:isSecureContext,ready:typeof window.fabricPasskey==="function"}')
            require(actual == dict(origin=origin, secure=True, ready=True), 'trusted-positive-origin')
            require(instance.resident_credentials(instance.authenticators[0])['count'] == 0, 'trust-control-no-enrollment')
            report['trusted_origin'] = actual
        else:
            receipt = json.loads((args.out / 'bridge/receipt.json').read_text())
            diagnostic = receipt.get('tls_navigation_failure', {})
            expected = 'SSL_ERROR_BAD_CERT_DOMAIN' if args.case == 'wrong-san' else 'SEC_ERROR_UNKNOWN_ISSUER'
            require(error is not None and diagnostic.get('codes') == [expected]
                    and diagnostic.get('uri', '').startswith('about:certerror'), 'specific-browser-certificate-refusal')
            require(not requests, 'certificate-refusal-before-HTTP')
            report.update(expected_code=expected, browser_diagnostic=diagnostic, startup_refusal=error)
        report['http_paths'] = requests
        report['exit'] = 0
    except BaseException as error:
        report['error'] = type(error).__name__ + ': ' + str(error)
    finally:
        errors = []
        try:
            if instance is not None and getattr(instance, 'receipt', None) is not None:
                instance.shutdown_deadline = min(end, time.monotonic() + 20)
                instance.close()
            if budget is not None:
                browser.browser_directory(budget)
                descriptor = os.open('cgroup.kill', os.O_WRONLY | os.O_NOFOLLOW, dir_fd=budget['fd'])
                try: os.write(descriptor, b'1')
                finally: os.close(descriptor)
            if parent is not None:
                browser.browser_reap_domain(parent, [] if budget is None else [{'group': budget}])
                require(parent.get('own_children_empty') is True, 'trust-guardian-ECHILD')
            clean = True
        except BaseException as error:
            errors.append(type(error).__name__ + ': ' + str(error))
        if server is not None:
            if loop is not None and loop.is_alive():
                server.shutdown(); loop.join(timeout=5)
            server.server_close()
            if loop is not None and loop.is_alive(): errors.append('TLS thread not stopped')
        for name in ('ca.key', 'decoy.key', 'server.key'):
            (root / name).unlink(missing_ok=True)
        if clean:
            require(libc.prctl(36, previous.value, 0, 0, 0) == 0, 'trust-subreaper-restore')
            shutil.rmtree(root)
        else:
            report['retained_fixture'] = str(root)
        for held in (parent, budget):
            if held is not None: os.close(held['fd'])
        signal.alarm(0); signal.signal(signal.SIGALRM, old_alarm)
        try:
            require(sha(args.pins) == args.pins_sha256, 'trust-tool-pins-after')
            verify_tool_pins(pins)
            report['immutable_tools'] = True
        except BaseException as error:
            errors.append(type(error).__name__ + ': ' + str(error))
            report['immutable_tools'] = False
        report.update(cleanup_errors=errors, guardian_own_children_empty=clean, tls_stopped=loop is None or not loop.is_alive())
        if errors: report['exit'] = 2
        (args.out / 'receipt.json').write_text(json.dumps(report, indent=2) + '\n')
    return report['exit']


if __name__ == '__main__':
    raise SystemExit(main())
