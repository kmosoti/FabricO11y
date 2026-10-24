"""Read-only guest process and kernel observations for the owner walkthrough."""
from owner_evidence import require


NATIVE = r'''
import json,os,pwd,subprocess
from pathlib import Path
account=pwd.getpwnam('fabricolly')
physical_memory_bytes=os.sysconf('SC_PHYS_PAGES')*os.sysconf('SC_PAGE_SIZE')
ancestor_memory={}
for relative in ('/', '/system.slice'):
 path=Path('/sys/fs/cgroup')/relative.lstrip('/')
 ancestor_memory[relative]={name:(path/name).read_text().strip() for name in ('memory.high','memory.max')}
units={}
for unit in ('fabrico11y-server.service','fabrico11y-node.service','system-fabrico11y.slice'):
 text=subprocess.check_output(['systemctl','show',unit,'--property=ActiveState,MainPID,ControlGroup,User,Group,EffectiveMemoryHigh,EffectiveMemoryMax,EffectiveTasksMax'],text=True)
 properties=dict(line.split('=',1) for line in text.splitlines())
 group=properties.get('ControlGroup')
 kernel={}
 if group:
  path=Path('/sys/fs/cgroup')/group.lstrip('/')
  for name in ('memory.high','memory.max','memory.swap.max','pids.max','cgroup.events'):
   kernel[name]=(path/name).read_text().strip()
 units[unit]={'properties':properties,'kernel':kernel}
processes=[];disappeared=[]
for path in sorted(Path('/proc').iterdir()):
 if not path.name.isdigit():continue
 try:
  status=dict(line.split(':',1) for line in (path/'status').read_text().splitlines())
  uids=list(map(int,status['Uid'].split()))
  exe=os.readlink(path/'exe')
  if account.pw_uid not in uids and exe not in ('/usr/bin/fabric-server','/usr/bin/fabric-node'):continue
  processes.append({'pid':int(path.name),'start_ticks':int((path/'stat').read_text().rsplit(')',1)[1].split()[19]),
   'uids':uids,'gids':list(map(int,status['Gid'].split())),'cap_eff':int(status['CapEff'],16),
   'no_new_privs':int(status['NoNewPrivs']),'exe':exe,
   'cgroup':(path/'cgroup').read_text().strip()})
 except (FileNotFoundError,ProcessLookupError):disappeared.append(int(path.name))
print(json.dumps({'boot_id':Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
 'account':{'uid':account.pw_uid,'gid':account.pw_gid,'shell':account.pw_shell},
 'physical_memory_bytes':physical_memory_bytes,'ancestor_memory':ancestor_memory,
 'units':units,'processes':processes,'disappeared_during_census':disappeared}))
'''


def verify(value, role, boot_id, *, active):
    require(value['boot_id'] == boot_id, 'guest boot identity changed')
    account = value['account']
    require(account['uid'] > 0 and account['gid'] > 0
            and account['shell'] in ('/usr/sbin/nologin', '/sbin/nologin'), 'static service account differs')
    units = value['units']
    if not active:
        require(not value['processes'], 'owned writers remain after stop')
        for name in ('fabrico11y-server.service', 'fabrico11y-node.service'):
            properties = units[name]['properties']
            require(properties['ActiveState'] == 'inactive' and properties['MainPID'] == '0',
                    'project unit remains active after stop')
            kernel = units[name]['kernel']
            require(not kernel or 'populated 0' in kernel['cgroup.events'].splitlines(),
                    'project cgroup remains populated after stop')
        return value
    service = 'fabrico11y-' + ('server' if role == 'server' else 'node') + '.service'
    path = '/system.slice/system-fabrico11y.slice/' + service
    limits = (2560, 3072, 512) if role == 'server' else (128, 256, 128)
    physical = value.get('physical_memory_bytes')
    allocated = (4096 if role == 'server' else 1536) * 1024**2
    require(type(physical) is int and limits[1] * 1024**2 < physical <= allocated,
            'physical guest memory witness missing or outside registered role')
    ancestors = value.get('ancestor_memory')
    require(type(ancestors) is dict and set(ancestors) == {'/', '/system.slice'}
            and all(row == {'memory.high': 'max', 'memory.max': 'max'}
                    for row in ancestors.values()), 'unexpected guest ancestor memory limit')
    properties = units[service]['properties']
    require(properties['ActiveState'] == 'active' and int(properties['MainPID']) > 0
            and properties['User'] == properties['Group'] == 'fabricolly'
            and properties['ControlGroup'] == path, 'active service identity or placement differs')
    expected = sorted(['/usr/bin/fabric-server', '/usr/bin/fabric-node'] if role == 'server'
                      else ['/usr/bin/fabric-node'])
    require(sorted(row['exe'] for row in value['processes']) == expected,
            'missing or extra native writer/companion')
    require(int(properties['MainPID']) in {row['pid'] for row in value['processes']},
            'service main PID absent from independent inventory')
    for row in value['processes']:
        require(row['uids'] == [account['uid']] * 4 and row['gids'] == [account['gid']] * 4
                and row['cap_eff'] == 0 and row['no_new_privs'] == 1 and row['start_ticks'] > 0
                and row['cgroup'] == '0::' + path, 'native writer privilege or cgroup differs')
    for name, (high, maximum, tasks) in ((service, limits), ('system-fabrico11y.slice', (2816, 3328, 640))):
        row = units[name]
        for key, expected_value in (('EffectiveMemoryHigh', min(high * 1024**2, physical)),
                                    ('EffectiveMemoryMax', min(maximum * 1024**2, physical)), ('EffectiveTasksMax', tasks)):
            require(row['properties'][key] == str(expected_value), 'effective guest limit differs')
        kernel_limits = [('memory.high', high * 1024**2), ('memory.max', maximum * 1024**2), ('pids.max', tasks)]
        if name == service:
            kernel_limits.append(('memory.swap.max', 0))
        for key, expected_value in kernel_limits:
            require(row['kernel'][key] == str(expected_value), 'kernel guest limit differs')
    return value
