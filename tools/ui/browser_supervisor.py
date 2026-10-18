"""Isolated browser wait domain; failed supervision fails the whole campaign."""
import argparse
import json
import os
from pathlib import Path
import re
import select
import signal
import socket
import subprocess
import sys
import time
import uuid

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import live_browser_check as browser

MAX_MESSAGE = 8192


def require(value, reason):
    if not value:
        raise RuntimeError('browser-supervisor-' + reason)


def send(control, value):
    raw = json.dumps(value, sort_keys=True).encode()
    require(len(raw) <= MAX_MESSAGE and control.send(raw) == len(raw), 'message')


def receive(control, deadline):
    require(select.select([control], [], [], max(0, deadline - time.monotonic()))[0], 'deadline')
    raw = control.recv(MAX_MESSAGE + 1)
    require(0 < len(raw) <= MAX_MESSAGE, 'message')
    value = json.loads(raw)
    require(type(value) is dict, 'message')
    return value


def original_service():
    current = browser.cgroups.current()
    service = next((path for path in (current, *current.parents)
                    if path.name.startswith('fabric-work-') and path.name.endswith('.service')), None)
    require(service is not None and current.is_relative_to(service), 'ancestor')
    maximum = (service / 'memory.max').read_text().strip()
    require(maximum != 'max' and 0 < int(maximum) <= 6 * 2**30
            and (service / 'memory.swap.max').read_text().strip() == '0', 'aggregate')
    require({'memory', 'cpu', 'pids', 'io'} <= set((service / 'cgroup.subtree_control').read_text().split()), 'delegation')
    return browser.browser_hold_directory(service)


def budget_limits(record):
    browser.browser_directory(record)
    values = {key: (record['path'] / key).read_text().strip() for key in
              ('memory.max', 'memory.high', 'memory.swap.max', 'pids.max', 'cpu.max')}
    require(0 < int(values['memory.high']) <= 850 * 2**20
            and int(values['memory.high']) <= int(values['memory.max']) <= 2**30
            and values['memory.swap.max'] == '0' and values['pids.max'] == '512'
            and values['cpu.max'] == '200000 100000', 'budget')
    return values


class BrowserSupervisor:
    """Owns one exclusively created domain and its actual supervisor Popen."""
    def __init__(self, command, environment, log, deadline, out, *, fault=None, browser_parent=None,
                 browser_entry_helper=None, browser_entry_helper_sha256=None):
        self.deadline, self.out = deadline, Path(out)
        self.process = self.control = self.domain = self.service = None
        self.budget = None
        self.closed = False
        self.cleanup_confirmed = False
        self.receipt = {'state': 'starting', 'cleanup_confirmed': False}
        require(deadline > time.monotonic(), 'deadline')
        require(fault in (None, 'before-ready', 'during-cleanup'), 'fault')
        require(isinstance(browser_entry_helper, Path) and browser_entry_helper.is_absolute()
                and isinstance(browser_entry_helper_sha256, str)
                and re.fullmatch('[0-9a-f]{64}', browser_entry_helper_sha256) is not None,
                'entry-helper-pin')
        require(self.out.is_absolute() and not self.out.exists()
                and self.out.parent.resolve().is_relative_to(browser.DATA / 'results'), 'output')
        self.out.mkdir(mode=0o700)
        try:
            self.service = original_service()
            domain_parent = self.service['path']
            if browser_parent is not None:
                browser.browser_directory(browser_parent)
                require(browser_parent.get('exclusive') is True
                        and browser_parent['path'] != self.service['path']
                        and browser_parent['path'].is_relative_to(self.service['path']), 'budget-ancestor')
                self.budget = dict(browser_parent, fd=os.dup(browser_parent['fd']))
                self.budget['limits'] = budget_limits(self.budget)
                require(not (self.budget['path'] / 'cgroup.procs').read_text().strip()
                        and 'populated 0' in (self.budget['path'] / 'cgroup.events').read_text().splitlines(), 'budget-empty')
                (self.budget['path'] / 'cgroup.subtree_control').write_text('+memory +cpu +pids +io')
                domain_parent = self.budget['path']
                self.receipt['inherited_browser_budget'] = dict(path=str(domain_parent),
                    dev=self.budget['dev'], ino=self.budget['ino'], limits=self.budget['limits'])
            path, _ = browser.cgroups.subgroup(domain_parent,
                'fabric-work-browser-controller-' + uuid.uuid4().hex, 4000000000, 3500000000, 512, 2)
            self.domain = browser.browser_hold_directory(path)
            self.domain['exclusive'] = True
            self.control, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET | socket.SOCK_CLOEXEC)
            invocation = [sys.executable, '-B', str(browser.ROOT / 'tools/bench/labs/completion/enter_group.py'),
                          str(path), sys.executable, '-B', str(Path(__file__).resolve()),
                          '--control-fd', str(child.fileno()), '--service-fd', str(self.service['fd']),
                          '--domain-fd', str(self.domain['fd']), '--deadline', repr(deadline),
                          '--browser-entry-helper', str(browser_entry_helper),
                          '--browser-entry-helper-sha256', browser_entry_helper_sha256,
                          *(['--budget-fd', str(self.budget['fd'])] if self.budget else []),
                          '--out', str(self.out), *(['--fault', fault] if fault else []), '--', *command]
            try:
                self.process = subprocess.Popen(invocation, env=environment, stdout=log, stderr=subprocess.STDOUT,
                    pass_fds=(child.fileno(), self.service['fd'], self.domain['fd'],
                              *([self.budget['fd']] if self.budget else [])))
            finally:
                child.close()
            self.pidfd = os.pidfd_open(self.process.pid)
            self.start_ticks = browser.browser_pid(self.process.pid)[0]
            self.receipt['supervisor'] = dict(pid=self.process.pid, start_ticks=self.start_ticks)
            ready = receive(self.control, min(deadline, time.monotonic() + 10))
            require(ready.get('kind') == 'ready' and ready.get('supervisor_pid') == self.process.pid
                    and ready.get('domain') == str(path) and ready.get('own_domain_acquired') is True, 'ready')
            browser.browser_directory(self.service)
            browser.browser_directory(self.domain)
            if self.budget is not None:
                require(budget_limits(self.budget) == self.budget['limits'] and path.parent == self.budget['path'], 'budget-changed')
            require(browser.browser_pidfd(self.pidfd) == self.process.pid
                    and browser.browser_pid(self.process.pid)[0] == self.start_ticks
                    and browser.cgroups.ROOT / next(row[3:] for row in
                        (Path('/proc') / str(self.process.pid) / 'cgroup').read_text().splitlines()
                        if row.startswith('0::')).lstrip('/') == path / 'supervisor', 'placement')
            self.receipt.update(state='ready', ready=ready)
        except BaseException:
            self._fallback()
            try: self._archive()
            finally: self._release()
            raise

    @property
    def pid(self):
        return self.process.pid

    def poll(self):
        return self.process.poll()

    def _archive(self):
        if self.out.exists():
            (self.out / 'parent-receipt.json').write_text(json.dumps(self.receipt, indent=2) + '\n')

    def _fallback(self, deadline=None):
        self.receipt['state'] = 'unconfirmed-whole-campaign-stop-required'
        try:
            if self.domain is not None:
                browser.browser_directory(self.service)
                browser.browser_directory(self.domain)
                descriptor = os.open('cgroup.kill', os.O_WRONLY | os.O_NOFOLLOW, dir_fd=self.domain['fd'])
                try: os.write(descriptor, b'1')
                finally: os.close(descriptor)
            if self.process is not None:
                self.process.wait(timeout=min(5, max(0, min(self.deadline, deadline if deadline is not None else self.deadline) - time.monotonic())))
        except BaseException as error:
            self.receipt['fallback_error'] = type(error).__name__ + ': ' + str(error)

    def _release(self):
        if self.control is not None:
            self.control.close()
            self.control = None
        for name in ('domain', 'budget', 'service'):
            record = getattr(self, name)
            if record is not None and record.get('fd') is not None:
                os.close(record['fd'])
                record['fd'] = None
        if getattr(self, 'pidfd', None) is not None:
            os.close(self.pidfd)
            self.pidfd = None

    def close(self, *, deadline=None):
        if self.closed:
            require(self.cleanup_confirmed, 'unconfirmed')
            return
        self.closed = True
        phase_end = min(self.deadline, time.monotonic() + 20, deadline if deadline is not None else self.deadline)
        try:
            require(time.monotonic() < phase_end, 'deadline')
            send(self.control, {'kind': 'stop', 'deadline_monotonic_ns': int(phase_end * 1e9)})
            answer = receive(self.control, phase_end)
            require(answer.get('kind') == 'closed' and answer.get('own_children_empty') is True
                    and answer.get('known_children_absent') is True, 'cleanup')
            require(self.process.wait(timeout=max(0, phase_end - time.monotonic())) == 0, 'exit')
            browser.browser_directory(self.service)
            browser.browser_directory(self.domain)
            if self.budget is not None:
                require(budget_limits(self.budget) == self.budget['limits'], 'budget-changed')
            require('populated 0' in (self.domain['path'] / 'cgroup.events').read_text().splitlines()
                    and not (self.domain['path'] / 'cgroup.procs').read_text().strip()
                    and time.monotonic() <= phase_end, 'population')
            self.cleanup_confirmed = True
            self.receipt.update(state='closed', cleanup_confirmed=True, cleanup=answer)
        except BaseException:
            self.receipt['phase_expired'] = time.monotonic() >= phase_end
            self._fallback(phase_end)
            raise
        finally:
            try: self._archive()
            finally: self._release()


def supervise(args):
    control = socket.socket(fileno=args.control_fd)
    require(control.getsockopt(socket.SOL_SOCKET, socket.SO_TYPE) == socket.SOCK_SEQPACKET, 'socket')
    current = browser.cgroups.current()
    domain = browser.browser_hold_directory(current)
    service = original_service()
    for actual, descriptor in ((domain, args.domain_fd), (service, args.service_fd)):
        held = os.fstat(descriptor)
        require((held.st_dev, held.st_ino) == (actual['dev'], actual['ino']), 'descriptor')
    budget = None
    if args.budget_fd is not None:
        budget = browser.browser_hold_directory(current.parent)
        held = os.fstat(args.budget_fd)
        require((held.st_dev, held.st_ino) == (budget['dev'], budget['ino']), 'budget-descriptor')
        budget_limits(budget)
    require(current.is_relative_to(service['path']) and current != service['path']
            and current.parent == (budget['path'] if budget is not None else service['path'])
            and current.name.startswith('fabric-work-browser-controller-')
            and (current / 'cgroup.procs').read_text().split() == [str(os.getpid())], 'exclusive-placement')
    # Descriptor identity and actual placement authorize this domain; its name is syntax only.
    parent, bindings = None, []
    log = None
    result = 1
    report = {'state': 'starting', 'own_children_empty': False, 'known_children_absent': False}
    def pause_fault(name):
        if args.fault == name:
            (args.out / ('fault-' + name + '.json')).write_text(json.dumps(
                dict(pid=os.getpid(), start_ticks=browser.browser_pid(os.getpid())[0], point=name)))
            os.kill(os.getpid(), signal.SIGSTOP)
    try:
        parent = browser.begin_browser_domain(current, args.deadline)
        log = (args.out / 'driver.log').open('xb')
        proc = browser.browser_group_launch(parent, bindings, 'webdriver', args.command, log, dict(os.environ),
            entry_helper=args.browser_entry_helper,
            entry_helper_sha256=args.browser_entry_helper_sha256)
        ready = dict(kind='ready', supervisor_pid=os.getpid(), domain=str(current),
                     own_domain_acquired=True, driver_pid=proc.pid,
                     driver_start_ticks=bindings[0]['group']['start_ticks'])
        pause_fault('before-ready')
        send(control, ready)
        report['ready'] = ready
        while True:
            require(time.monotonic() < args.deadline, 'deadline')
            require(proc.poll() is None, 'driver-exit')
            if select.select([control], [], [], min(.1, args.deadline - time.monotonic()))[0]:
                request = receive(control, args.deadline)
                stop_ns = request.get('deadline_monotonic_ns')
                require(set(request) == {'kind', 'deadline_monotonic_ns'} and request['kind'] == 'stop'
                        and type(stop_ns) is int and time.monotonic_ns() < stop_ns <= int(args.deadline * 1e9), 'control')
                parent['deadline_monotonic_ns'] = stop_ns
                for binding in bindings:
                    binding['parent']['deadline_monotonic_ns'] = stop_ns
                break
        result = 0
    except BaseException as error:
        report['error'] = type(error).__name__ + ': ' + str(error)
    finally:
        try:
            if parent is not None:
                browser.stop_browser_groups(parent, bindings)
                pause_fault('during-cleanup')
                browser.browser_reap_domain(parent, bindings)
                report['own_children_empty'] = parent.get('own_children_empty') is True
                report['known_children_absent'] = all(not (Path('/proc') / str(b['group']['pid'])).exists() for b in bindings)
                browser.end_browser_domain(parent, bindings)
            require(report['own_children_empty'] and report['known_children_absent'], 'cleanup')
            if result == 0:
                send(control, dict(kind='closed', own_children_empty=True, known_children_absent=True))
        except BaseException as error:
            report['cleanup_error'] = type(error).__name__ + ': ' + str(error)
            result = 1
        if log is not None:
            log.close()
        report.update(state='closed' if result == 0 else 'failed', exit=result)
        (args.out / 'supervisor-receipt.json').write_text(json.dumps(report, indent=2) + '\n')
        control.close()
        os.close(domain['fd'])
        os.close(service['fd'])
        if budget is not None: os.close(budget['fd'])
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ('control-fd', 'service-fd', 'domain-fd'):
        parser.add_argument('--' + option, type=int, required=True)
    parser.add_argument('--deadline', type=float, required=True)
    parser.add_argument('--browser-entry-helper', type=Path, required=True)
    parser.add_argument('--browser-entry-helper-sha256', required=True)
    parser.add_argument('--budget-fd', type=int)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--fault', choices=('before-ready', 'during-cleanup'))
    parser.add_argument('command', nargs=argparse.REMAINDER)
    arguments = parser.parse_args()
    if arguments.command[:1] == ['--']:
        arguments.command.pop(0)
    raise SystemExit(supervise(arguments))
