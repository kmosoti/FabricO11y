"""Observe stopped/removed installation units without interpreting LSB status alone."""
import json
import os
from pathlib import Path
import subprocess
import time

UNITS = ('fabrico11y-server.service', 'fabrico11y-node.service')
PROJECT = ('fabric-server', 'fabric-node', 'fabricctl')
PROPERTIES = ('Id', 'LoadState', 'ActiveState', 'SubState', 'MainPID', 'ControlGroup')
CGROUP = Path('/sys/fs/cgroup/system.slice/system-fabrico11y.slice')
PROC = Path('/proc')


def require(ok, message):
    if not ok:
        raise ValueError(message)


def grade(observed):
    require(set(observed['units']) == set(UNITS), 'removal unit inventory incomplete')
    for name, unit in observed['units'].items():
        active, shown = unit['active'], unit['show']
        pairs = [line.split('=', 1) for line in shown['stdout'].splitlines()]
        require(all(len(pair) == 2 for pair in pairs) and len(pairs) == len(PROPERTIES), 'removal unit properties incomplete')
        values = dict(pairs)
        require(set(values) == set(PROPERTIES) and shown['exit'] == 0 and not shown['stderr']
            and not active['stderr'] and values['Id'] == name, 'removal unit query failed')
        state = values['ActiveState']
        stopped = (active['exit'] == 3 and values['LoadState'] == 'loaded'
            and state in ('inactive', 'failed') and values['SubState'] == {'inactive':'dead', 'failed':'failed'}[state])
        removed = (active['exit'] == 4 and values['LoadState'] == 'not-found'
            and state == 'inactive' and values['SubState'] == 'dead' and values['ControlGroup'] == '')
        require((stopped or removed) and active['stdout'] == state+'\n' and values['MainPID'] == '0'
            and values['ControlGroup'] in ('', '/system.slice/system-fabrico11y.slice/'+name), 'removal unit not stopped')
    require(observed['ps']['exit'] == 0 and not observed['ps']['stderr'] and not observed['project_processes']
        and not any(line.split(None, 5)[4] in PROJECT for line in observed['ps']['stdout'].splitlines()
                    if len(line.split(None, 5)) >= 5), 'removal project process remains')
    require(observed['cgroup']['complete'] is True
        and ((observed['cgroup']['exists'] and str(CGROUP/'cgroup.procs') in observed['cgroup']['members'])
             or (not observed['cgroup']['exists'] and not observed['cgroup']['members']))
        and all(not value.strip()
        for value in observed['cgroup']['members'].values()), 'removal cgroup descendant remains')


def collect(observed):
    deadline = min(float(os.environ.get('FABRIC_NATIVE_PHASE_DEADLINE', 'inf')), time.monotonic()+30)
    def command(argv):
        remaining = deadline-time.monotonic()
        require(remaining > 0, 'removal observer deadline')
        result = subprocess.run(argv, text=True, capture_output=True, timeout=min(10, remaining))
        return dict(argv=argv, exit=result.returncode, stdout=result.stdout, stderr=result.stderr)
    observed['units'] = {}
    for name in UNITS:
        observed['units'][name] = dict(active=command(['systemctl','is-active',name]),
            show=command(['systemctl','show',name,*[part for key in PROPERTIES for part in ('-p',key)]]))
    observed['ps'] = command(['ps','-e','-o','pid=,ppid=,uid=,gid=,comm=,args='])
    observed['project_processes'], observed['no_executable_proc_entries'] = [], []
    for directory in PROC.glob('[0-9]*'):
        try:
            executable = os.readlink(directory/'exe')
            if Path(executable.removesuffix(' (deleted)')).name in PROJECT:
                observed['project_processes'].append(dict(pid=int(directory.name), exe=executable,
                    stat=(directory/'stat').read_text(), cgroup=(directory/'cgroup').read_text()))
        except FileNotFoundError:
            observed['no_executable_proc_entries'].append(directory.name)
    existed = CGROUP.exists()
    members = {str(path):path.read_text() for path in CGROUP.rglob('cgroup.procs')}
    require(CGROUP.exists() == existed, 'removal cgroup changed during observation')
    observed['cgroup'] = dict(path=str(CGROUP), exists=existed, members=members, complete=True)


def main():
    require(os.geteuid() == 0 and Path('/root/accept').is_dir(), 'owned disposable installation root required')
    observed = dict(state='observing')
    try:
        collect(observed)
        grade(observed)
        observed['state'] = 'accepted'
    except BaseException as error:
        observed.update(state='failed', error=repr(error))
        raise
    finally:
        with Path('/root/accept/removal-observer.json').open('x') as stream:
            json.dump(observed, stream, indent=2); stream.write('\n')
        print(json.dumps(observed, sort_keys=True))


if __name__ == '__main__':
    main()
