"""Literal caller controls, never packaged AT or pointer evidence."""
import copy
import unittest
from pathlib import Path
from types import SimpleNamespace
from announcement_contract import check_alert,grade_targets,square_fits


class AnnouncementCallTests(unittest.TestCase):
    def test_modes_fail_before_starting_incompatible_cells(self):
        import browser_announcements as caller
        class Parser:
            def error(self,reason):raise ValueError(reason)
        args=SimpleNamespace(announcement_probes=True,suppress_announcement=False,browser='firefox',
            announcement_xvfb_admission=Path('/prepared-xvfb'),announcement_orca_admission=Path('/prepared-orca'))
        caller.validate_mode(args,Parser())
        for mode in caller.MODES:
            changed=copy.copy(args);setattr(changed,mode,True)
            with self.subTest(mode=mode),self.assertRaisesRegex(ValueError,'separate Firefox cell'):
                caller.validate_mode(changed,Parser())
        for field,value in (('browser','chrome'),('announcement_orca_admission',None),
                ('announcement_xvfb_admission',None),('announcement_probes',False)):
            changed=copy.copy(args);setattr(changed,field,value)
            with self.subTest(field=field),self.assertRaises(ValueError):caller.validate_mode(changed,Parser())
        changed=copy.copy(args);changed.announcement_probes=False;changed.suppress_announcement=True
        with self.assertRaisesRegex(ValueError,'suppression requires'):caller.validate_mode(changed,Parser())

    def test_document_snapshot_is_identity_only_and_cannot_substitute_for_events(self):
        from browser_announcements import document_identity
        from announcement_oracle import check_announcement
        row=dict(type='document:snapshot',pid=700,document_attributes=['DocURL:https://localhost:443/console/'],
            text='Public probe complete',monotonic_ns=150)
        self.assertEqual(document_identity([row],700,'https://localhost:443'),[row])
        self.assertEqual(document_identity([row],701,'https://localhost:443'),[])
        self.assertEqual(document_identity([row],700,'https://localhost:444'),[])
        row['document_attributes']=['DocURL:https://localhost:443/console-attacker/']
        self.assertEqual(document_identity([row],700,'https://localhost:443'),[])
        with self.assertRaisesRegex(ValueError,'announcement-event'):
            check_announcement(dict(before_ns=100,after_ns=200,text=row['text'],browser_pid=700,
                role='status',live='polite',focus_elsewhere=True),[row],
                [dict(before_ns=160,after_ns=170,text=row['text'],module='espeak-ng',ssip_code=225,message_id=8)],
                dict(module='espeak-ng',pid=701,libespeak_mapped=True,pcm_before_bytes=0,pcm_after_bytes=4096))

    def test_snapshot_reads_document_attributes_without_walking_or_capturing_its_contents(self):
        import os,tempfile
        from announcement_probe import snapshot_documents,read_rows
        class Document:
            childCount=1
            def queryDocument(self):return SimpleNamespace(getAttributes=lambda:['DocURL:https://fixture.invalid/console/'])
            def getApplication(self):return SimpleNamespace(get_process_id=lambda:700)
            def getRoleName(self):return 'document web'
            def getAttributes(self):return ['tag:body']
            def __getitem__(self,index):raise AssertionError('must not inspect document contents')
        class Desktop:
            childCount=1
            def queryDocument(self):raise NotImplementedError
            def __getitem__(self,index):return Document()
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            path=Path(directory)/'events.jsonl';snapshot_documents(Desktop(),path);rows=read_rows(path)
        self.assertEqual(len(rows),1);self.assertEqual(rows[0]['type'],'document:snapshot')
        self.assertEqual(rows[0]['text'],'');self.assertEqual(rows[0]['pid'],700)

    def target(self,name,x=0,y=0,width=24,height=24):
        return dict(id=name,kind='button',text=name,x=x,y=y,width=width,height=height,
            radii=[[0,0]]*4,transform='none',clip='none',inline={})

    def test_size_spacing_and_rounding_have_literal_counterexamples(self):
        self.assertEqual(grade_targets([self.target('size')])[0]['qualification'],'size')
        spaced=[self.target('a',width=10,height=10),self.target('b',x=24,width=10,height=10)]
        self.assertEqual([row['qualification'] for row in grade_targets(spaced)],['spacing','spacing'])
        spaced[1]['x']=20
        with self.assertRaisesRegex(ValueError,'announcement-target-spacing'):grade_targets(spaced)
        circle=self.target('circle');circle['radii']=[[12,12]]*4
        self.assertFalse(square_fits(circle));self.assertEqual(grade_targets([circle])[0]['qualification'],'spacing')
        rounded=self.target('rounded',width=32,height=32);rounded['radii']=[[8,8]]*4
        self.assertTrue(square_fits(rounded))
        for defect in ('duplicate','overlap','nan','clip'):
            rows=[self.target('a',width=10,height=10),self.target('b',x=40)]
            if defect=='duplicate':rows[1]['id']='a'
            elif defect=='overlap':rows[1]['x']=8
            elif defect=='nan':rows[0]['x']=float('nan')
            else:rows[0]['clip']='circle(50%)'
            with self.subTest(defect=defect),self.assertRaises(ValueError):grade_targets(rows)

    def test_inline_exception_does_not_exempt_an_arbitrary_control(self):
        inline=self.target('inline',width=10,height=10);inline.update(kind='a',text='help',
            inline=dict(display='inline',ancestor='p',context='Read the help for details.'))
        self.assertEqual(grade_targets([inline])[0]['qualification'],'inline-text')
        inline['kind']='button'
        with self.assertRaisesRegex(ValueError,'announcement-target-spacing'):
            grade_targets([inline,self.target('adjacent',x=10)])

    def test_alert_requires_actual_assertive_event_speech_and_synthesis(self):
        case=dict(before_ns=100,after_ns=200,text='Validation failed.',browser_pid=700,role='alert',live=None,focus_elsewhere=True)
        events=[dict(pid=700,monotonic_ns=150,text=case['text'],type='object:children-changed:add',attributes=['xml-roles:alert','live:assertive'])]
        speech=[dict(before_ns=160,after_ns=170,text=case['text'],module='espeak-ng',ssip_code=225,message_id=8)]
        backend=dict(module='espeak-ng',pid=701,libespeak_mapped=True,pcm_before_bytes=0,pcm_after_bytes=4096)
        check_alert(case,events,speech,backend)
        for defect in ('role','live','event-role','event-pid','speech','pcm'):
            c,e,s,b=copy.deepcopy((case,events,speech,backend))
            if defect=='role':c['role']='status'
            elif defect=='live':c['live']='off'
            elif defect=='event-role':e[0]['attributes']=['xml-roles:status','live:assertive']
            elif defect=='event-pid':e[0]['pid']=702
            elif defect=='speech':s=[]
            else:b['pcm_after_bytes']=0
            with self.subTest(defect=defect),self.assertRaises(ValueError):check_alert(c,e,s,b)
