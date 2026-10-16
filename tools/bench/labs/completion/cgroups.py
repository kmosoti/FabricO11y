"""Owned delegated descendants only. Parent aggregate cap remains in force."""
import os
from pathlib import Path

ROOT = Path('/sys/fs/cgroup')

def current():
    return ROOT / next(x[3:] for x in Path('/proc/self/cgroup').read_text().splitlines()
                       if x.startswith('0::')).lstrip('/')

def memory_limit_values(maximum, high, page_bytes):
    if page_bytes <= 0 or maximum <= 0 or high <= 0:
        raise ValueError('memory limits and page size must be positive')
    if high > maximum:
        raise ValueError('memory.high cannot exceed memory.max')
    aligned_max = maximum // page_bytes * page_bytes
    aligned_high = high // page_bytes * page_bytes
    if aligned_max == 0 or aligned_high == 0 or aligned_high > aligned_max:
        raise ValueError('page-rounded memory limits are invalid')
    return {'memory.max': str(aligned_max), 'memory.high': str(aligned_high)}

def set_limits(group, maximum, high, tasks, cores):
    values = {**memory_limit_values(maximum, high, os.sysconf('SC_PAGE_SIZE')),
              'memory.swap.max': '0', 'pids.max': str(tasks),
              'cpu.max': f'{int(cores * 100000)} 100000'}
    for name, value in values.items():
        (group / name).write_text(value)
    actual = {k: (group / k).read_text().strip() for k in values}
    if actual != values:
        raise RuntimeError(f'cgroup enforcement mismatch: {actual} != {values}')
    return actual

def enable_controllers(group):
    required = {'memory', 'cpu', 'pids', 'io'}
    available = set((group / 'cgroup.controllers').read_text().split())
    if not required <= available:
        raise RuntimeError(f'required cgroup controllers unavailable: {sorted(required-available)}; available={sorted(available)}')
    (group / 'cgroup.subtree_control').write_text('+memory +cpu +pids +io')
    enabled = set((group / 'cgroup.subtree_control').read_text().split())
    if not required <= enabled:
        raise RuntimeError(f'required cgroup controllers not enabled: {sorted(required-enabled)}')


def delegate():
    parent = current()
    if not parent.name.startswith('fabric-work-'):
        from resource_guardian import workload_context
        context = workload_context()
        if context is None or context['workload'] != parent:
            raise RuntimeError('expected fresh owned launcher service or held workload root')
    supervisor = parent / 'supervisor'
    supervisor.mkdir()
    for pid in (parent / 'cgroup.procs').read_text().split():
        (supervisor / 'cgroup.procs').write_text(pid)
    enable_controllers(parent)
    set_limits(supervisor, 20*2**30, 16*2**30, 1024, 4)
    return parent

def subgroup(parent, name, maximum, high, tasks, cores, children=False):
    group = parent / name
    group.mkdir()
    limits = set_limits(group, maximum, high, tasks, cores)
    if children:
        enable_controllers(group)
    return group, limits

def enter(group):
    # Called before exec; all subsequent children inherit this exact group.
    (Path(group) / 'cgroup.procs').write_text(str(os.getpid()))
    if current() != Path(group):
        raise RuntimeError('child did not enter its assigned group')

def snapshot(parent):
    return {str(g.relative_to(parent)): {k: (g/k).read_text().strip()
            for k in ('memory.max','memory.high','memory.swap.max','memory.peak','memory.current',
                      'memory.events','memory.swap.current','cpu.max','cpu.stat','io.stat',
                      'pids.max','pids.current','pids.peak','cgroup.events','cgroup.procs')}
            for g in (parent, *(g for g in parent.rglob('*') if g.is_dir()))}
