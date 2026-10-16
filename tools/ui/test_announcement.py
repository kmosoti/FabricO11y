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

    def test_orca_observer_preserves_speak_arguments_result_and_capture_gate(self):
        import json,os,tempfile
        from types import SimpleNamespace
        from unittest.mock import patch
        import announcement_probe as probe
        calls=[];answer=(225,'queued',['123'])
        class Client:
            def speak(self,*args,**kwargs):calls.append((args,kwargs));return answer
            def get_output_module(self):return 'espeak-ng'
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            work=Path(directory);out=work/'out';out.mkdir()
            with patch.dict('sys.modules',speechd=SimpleNamespace(SSIPClient=Client)):
                exec(probe.customization(work,out),{})
                client=Client();self.assertIs(client.speak('before capture'),answer)
                self.assertFalse((out/'speech.jsonl').exists())
                (work/'capture').touch();callback=object()
                self.assertIs(client.speak('Public probe complete',callback=callback),answer)
            self.assertEqual(calls,[(('before capture',),{}),(('Public probe complete',),{'callback':callback})])
            rows=[json.loads(line) for line in (out/'speech.jsonl').read_text().splitlines()]
            self.assertEqual(len(rows),1);self.assertEqual((rows[0]['ssip_code'],rows[0]['message_id'],rows[0]['module']),(225,123,'espeak-ng'))

    def test_daemon_diagnostic_allowance_has_exact_boundary_and_other_streams_stay_bounded(self):
        import os,tempfile
        import announcement_probe as probe
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root=Path(directory);(root/'speech-logs').mkdir()
            daemon=root/'speech-logs/speech-dispatcher.log';other=root/'other.log'
            for path,limit in ((daemon,4*1024**2),(other,1024**2)):
                with path.open('wb') as stream:stream.truncate(limit)
                probe.check_stream(path,root)
                with path.open('r+b') as stream:stream.truncate(limit+1)
                with self.subTest(path=path.name),self.assertRaisesRegex(ValueError,'announcement-stream-bound'):
                    probe.check_stream(path,root)


    def test_actual_marked_ssip_format_preserves_words_and_rejects_markup_defects(self):
        import json
        fixture=json.loads((Path(__file__).resolve().parents[2]/'docs/experiments/formal/fixtures/orca-marked-speech.json').read_text())
        expected=fixture['expected_spoken_text'];raw=fixture['actual_row']['text']
        self.assertNotIn(expected,raw)
        values=list(copy.deepcopy(self.evidence()))
        values[0]['text']=values[1][0]['text']=expected
        values[2][0]['text']=raw
        oracle.check_announcement(*values)
        for invalid in ('<speak><mark name="0:6"/>Fabric public AT probe',
                '<speak><emphasis>Fabric public AT probe</emphasis></speak>',
                '<speak><mark name="Fabric public AT probe"/></speak>',
                '<speak><mark name="0:6"/>Different words</speak>',
                '<other>Fabric public AT probe</other>',
                '<speak voice="other">Fabric public AT probe</speak>',
                '<!DOCTYPE speak [<!ENTITY x "Fabric public AT probe">]><speak>&x;</speak>',
                '<speak><?extra data?>Fabric public AT probe</speak>'):
            values[2][0]['text']=invalid
            with self.subTest(raw=invalid),self.assertRaisesRegex(ValueError,'announcement-(utterance|speech-format)'):
                oracle.check_announcement(*values)

    def test_reader_admission_rejects_changed_missing_extra_and_symlink_source(self):
        import hashlib,os,tempfile
        import announcement_probe as probe
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root=Path(directory)/'extract';binary=root/'usr/bin/orca'
            purelib=root/'usr/lib/python3.15/site-packages';schemas=root/'usr/share/glib-2.0/schemas'
            for folder in (binary.parent,purelib/'orca',schemas):folder.mkdir(parents=True,exist_ok=True)
            binary.write_text('literal launcher control');code=purelib/'orca/orca_platform.py';code.write_text('literal source control')
            compiled=schemas/'gschemas.compiled';compiled.write_bytes(b'literal schema control')
            digest=lambda path:hashlib.sha256(path.read_bytes()).hexdigest()
            record=dict(state='prepared',exit=0,package_identity='orca|51.1|1.fc45|noarch',binary=str(binary),
                pythonpath=str(purelib),schemas=str(schemas),binary_sha256=digest(binary),schema_sha256=digest(compiled),
                extracted_files={str(path.relative_to(root)):digest(path) for path in root.rglob('*') if path.is_file()})
            self.assertEqual(probe.admitted_orca(record)[0],binary)
            for path in (binary,code,compiled):
                original=path.read_bytes();path.write_bytes(original+b'changed')
                with self.subTest(path=path),self.assertRaisesRegex(ValueError,'announcement-orca-admission'):probe.admitted_orca(record)
                path.write_bytes(original)
            extra=purelib/'unexpected.py';extra.write_text('not in receipt')
            with self.assertRaisesRegex(ValueError,'announcement-orca-admission'):probe.admitted_orca(record)

            extra.unlink();original=code.read_bytes();code.unlink()
            with self.assertRaisesRegex(ValueError,'announcement-orca-admission'):probe.admitted_orca(record)
            code.write_bytes(original);extra.symlink_to(code)
            with self.assertRaisesRegex(ValueError,'announcement-orca-admission'):probe.admitted_orca(record)

    def test_shutdown_waits_for_reader_before_its_dependencies(self):
        import signal,time
        from unittest.mock import patch
        import announcement_probe as probe
        actions=[]
        class Process:
            def __init__(self,pid,label):self.pid=pid;self.label=label;self.returncode=None
            def poll(self):return self.returncode
            def wait(self,timeout):
                self.assert_dependencies()
                actions.append(('wait',self.label));self.returncode=0;return 0
            def assert_dependencies(self):
                if self.label=='orca':self_outer.assertIsNone(display.returncode)
        self_outer=self;display=Process(910,'display');reader=Process(911,'orca');driver=Process(912,'webdriver')
        processes=[display,reader,driver];commands=[dict(label=process.label) for process in processes]
        with patch('announcement_probe.os.killpg',side_effect=lambda pid,sig:actions.append(('signal',pid,sig))):
            rows,errors=probe.stop_processes(processes,commands,time.monotonic()+25)
        self.assertEqual(errors,[]);self.assertEqual([row['label'] for row in rows],['webdriver','orca','display'])
        self.assertEqual(actions,[('signal',912,signal.SIGTERM),('wait','webdriver'),
            ('signal',911,signal.SIGTERM),('wait','orca'),('signal',910,signal.SIGTERM),('wait','display')])

    def test_forced_or_failed_cleanup_cannot_credit_success(self):
        import subprocess,time
        from unittest.mock import patch
        import announcement_probe as probe
        class Process:
            pid=913;returncode=None
            def poll(self):return self.returncode
            def wait(self,timeout):
                if self.returncode is None:self.returncode=-9;raise subprocess.TimeoutExpired('literal stuck reader',timeout)
                return self.returncode
        with patch('announcement_probe.os.killpg'):
            rows,errors=probe.stop_processes([Process()],[dict(label='orca')],time.monotonic()+25)
        self.assertTrue(errors);self.assertTrue(rows[0]['kill_sent']);self.assertEqual(rows[0]['exit'],-9)
