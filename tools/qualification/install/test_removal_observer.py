"""Literal unit ABI controls plus owned filesystem collector counterexamples."""
import copy
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import removal_observer as observer


def literal(missing=True):
    units={}
    for name in observer.UNITS:
        values=dict(Id=name,LoadState='not-found' if missing else 'loaded',ActiveState='inactive',
                    SubState='dead',MainPID='0',ControlGroup='')
        units[name]=dict(active=dict(exit=4 if missing else 3,stdout='inactive\n',stderr=''),
            show=dict(exit=0,stdout=''.join(k+'='+v+'\n' for k,v in values.items()),stderr=''))
    return dict(units=units,ps=dict(exit=0,stdout='',stderr=''),project_processes=[],
                cgroup=dict(complete=True,exists=False,members={}))


class RemovedServiceControls(unittest.TestCase):
    def test_complete_missing_and_loaded_stopped_units_are_admitted(self):
        for missing in (True,False): observer.grade(literal(missing))

    def test_query_error_incomplete_properties_and_live_states_refuse(self):
        for variant in ('missing','duplicate','show-error','active-error','stderr','pid','active','substate','group','load'):
            with self.subTest(variant=variant):
                record=literal();unit=record['units'][observer.UNITS[0]];shown=unit['show']
                if variant=='missing': shown['stdout']=shown['stdout'].replace('MainPID=0\n','')
                elif variant=='duplicate': shown['stdout']=shown['stdout'].replace('MainPID=0\n','Id=duplicate\n')
                elif variant=='show-error': shown['exit']=1
                elif variant=='active-error': unit['active']['exit']=1
                elif variant=='stderr': shown['stderr']='query failed'
                elif variant=='pid': shown['stdout']=shown['stdout'].replace('MainPID=0','MainPID=71')
                elif variant=='active': shown['stdout']=shown['stdout'].replace('ActiveState=inactive','ActiveState=active')
                elif variant=='substate': shown['stdout']=shown['stdout'].replace('SubState=dead','SubState=running')
                elif variant=='group': shown['stdout']=shown['stdout'].replace('ControlGroup=','ControlGroup=/unexpected')
                elif variant=='load': shown['stdout']=shown['stdout'].replace('LoadState=not-found','LoadState=loaded')
                with self.assertRaisesRegex(ValueError,'removal unit'): observer.grade(record)

    def test_process_and_cgroup_omission_or_survival_refuse(self):
        for variant in ('ps','exe','member','incomplete','missing-root-member'):
            with self.subTest(variant=variant):
                record=literal()
                if variant=='ps': record['ps']['stdout']='71 1 987 987 fabric-node /usr/bin/fabric-node\n'
                elif variant=='exe': record['project_processes']=[dict(pid=71,exe='/usr/bin/fabric-node (deleted)')]
                elif variant=='member': record['cgroup'].update(exists=True,members={str(observer.CGROUP/'cgroup.procs'):'71\n'})
                elif variant=='incomplete': record['cgroup']['complete']=False
                else: record['cgroup']['exists']=True
                with self.assertRaisesRegex(ValueError,'removal (project|cgroup)'): observer.grade(record)

    def test_collector_reads_original_proc_and_all_cgroup_descendants(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as root:
            root=Path(root);proc=root/'proc';proc.mkdir();group=root/'group';group.mkdir()
            (group/'cgroup.procs').write_text('');child=group/'child';child.mkdir();(child/'cgroup.procs').write_text('')
            baseline=literal()
            def call(argv,**kwargs):
                row=baseline['ps'] if argv[0]=='ps' else baseline['units'][argv[2] if argv[1]=='show' else argv[-1]]['show' if argv[1]=='show' else 'active']
                return subprocess.CompletedProcess(argv,row['exit'],row['stdout'],row['stderr'])
            with patch.object(observer,'PROC',proc),patch.object(observer,'CGROUP',group),patch.object(observer.subprocess,'run',side_effect=call):
                record={};observer.collect(record);observer.grade(record)
                self.assertEqual(len(record['cgroup']['members']),2)
                (child/'cgroup.procs').write_text('71\n');record={};observer.collect(record)
                with self.assertRaisesRegex(ValueError,'removal cgroup'): observer.grade(record)
                (child/'cgroup.procs').write_text('');pid=proc/'71';pid.mkdir();exe=root/'fabric-node';exe.touch()
                (pid/'exe').symlink_to(exe);(pid/'stat').write_text('71 (fabric-node)');(pid/'cgroup').write_text('0::/project\n')
                record={};observer.collect(record)
                with self.assertRaisesRegex(ValueError,'removal project'): observer.grade(record)
