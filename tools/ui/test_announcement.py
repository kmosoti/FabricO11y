"""Literal controls; these records are not native accessibility evidence."""
import copy
from pathlib import Path
import unittest
import announcement_oracle as oracle


class AnnouncementTests(unittest.TestCase):
    def evidence(self):
        return (dict(before_ns=100,after_ns=200,text='Public probe complete',browser_pid=700,role='status',live='polite',focus_elsewhere=True),
                [dict(pid=700,monotonic_ns=150,text='Public probe complete',type='object:text-changed:insert')],
                [dict(before_ns=160,after_ns=170,text='Public probe complete',module='espeak-ng',ssip_code=225,message_id=4)],
                dict(module='espeak-ng',pid=701,libespeak_mapped=True,pcm_before_bytes=0,pcm_after_bytes=4096))

    def test_actual_observer_source_is_available(self):
        self.assertTrue((Path(__file__).parent/'announcement_probe.py').is_file(),'missing actual AT-SPI/Orca observer')

    def test_literal_speech_event_and_synthesis_defects_are_rejected(self):
        oracle.check_announcement(*self.evidence())
        for item,field,value,refusal in ((3,'module','dummy','announcement-backend'),
                (3,'pcm_after_bytes',0,'announcement-synthesis'),(0,'focus_elsewhere',False,'announcement-dom')):
            changed=list(copy.deepcopy(self.evidence()));changed[item][field]=value
            with self.subTest(field=field),self.assertRaisesRegex(ValueError,refusal):oracle.check_announcement(*changed)
        for index,refusal in ((1,'announcement-event'),(2,'announcement-utterance')):
            changed=list(self.evidence());changed[index]=[]
            with self.assertRaisesRegex(ValueError,refusal):oracle.check_announcement(*changed)
        changed=list(self.evidence());changed[1][0]['pid']=702
        with self.assertRaisesRegex(ValueError,'announcement-event'):oracle.check_announcement(*changed)
        changed=list(self.evidence());changed[2][0]['after_ns']=201
        with self.assertRaisesRegex(ValueError,'announcement-utterance'):oracle.check_announcement(*changed)
