"""External whole-job observations; does not change frozen fixture gates."""
import os,time
from pathlib import Path

def pids(group):
 result=set()
 for path in [group/'cgroup.procs',*group.rglob('*/cgroup.procs')]:
  try:result.update(map(int,path.read_text().split()))
  except FileNotFoundError:pass
 return sorted(result)

def sample(group):
 row={'wall_ns':time.time_ns(),'monotonic_ns':time.monotonic_ns(),'boottime_ns':time.clock_gettime_ns(time.CLOCK_BOOTTIME),'cgroup':{},'processes':{}}
 for key in ('memory.current','memory.peak','memory.stat','memory.events','memory.swap.current','cpu.stat','io.stat','memory.pressure','io.pressure'):
  row['cgroup'][key]=(group/key).read_text().strip()
 for pid in pids(group):
  root=Path('/proc')/str(pid)
  try:
   status=dict(line.split(':',1) for line in (root/'status').read_text().splitlines() if ':' in line)
   stat=(root/'stat').read_text().rpartition(') ')[2].split()
   row['processes'][str(pid)]={'name':status['Name'].strip(),'rss_kib':int(status.get('VmRSS','0 kB').split()[0]),'hwm_kib':int(status.get('VmHWM','0 kB').split()[0]),'user_ticks':int(stat[11]),'system_ticks':int(stat[12]),'io':(root/'io').read_text()}
  except (FileNotFoundError,ProcessLookupError,PermissionError):pass
 return row
