"""Literal caller controls, never packaged AT or pointer evidence."""
import copy
import unittest
from announcement_contract import check_alert,grade_targets,square_fits


class AnnouncementCallTests(unittest.TestCase):
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
