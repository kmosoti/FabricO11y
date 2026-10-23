"""Origin: diagnostic03 empty initial AT tree, then real console document events."""
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import announcement_probe as probe

class LateDocument(unittest.TestCase):
    def test_loaded_document_identity_after_actual_focus_gain(self):
        self.focus_snapshot(1,True)

    def test_focus_loss_does_not_snapshot_application(self):
        self.focus_snapshot(0,False)

    def focus_snapshot(self,focused,expected):
        queries=[]
        class Document:
            def queryDocument(self):
                queries.append('document')
                return SimpleNamespace(getAttributes=lambda:['DocURL:https://localhost:36093/console/'])
            def getApplication(self):return app
            def getRoleName(self):return 'document web'
            def getAttributes(self):return ['tag:body']
        class Application:
            childCount=1
            def queryDocument(self):raise NotImplementedError
            def __getitem__(self,index):return Document()
            def get_process_id(self):return 77
        app=Application()
        class Focus:
            def queryText(self):return SimpleNamespace(getText=lambda start,end:'Check session')
            def getApplication(self):return app
            def getRoleName(self):return 'push button'
            def getAttributes(self):return ['tag:button']
        class Desktop:
            childCount=0
            def queryDocument(self):raise NotImplementedError
        class Registry:
            def registerEventListener(self,callback,*events):self.callback=callback;self.events=events
            def getDesktopCount(self):return 1
            def getDesktop(self,index):return Desktop()
            def start(self):
                if 'object:state-changed:focused' in self.events:
                    self.callback(SimpleNamespace(type='object:state-changed:focused',detail1=focused,source=Focus()))
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as root:
            path=Path(root)/'events.jsonl'
            with patch.dict(sys.modules,{'pyatspi':SimpleNamespace(Registry=Registry())}):probe.observe(path)
            snapshots=[r for r in probe.read_rows(path) if r['type']=='document:snapshot']
        self.assertEqual(bool(snapshots),expected)
        self.assertEqual(queries,['document'] if expected else [])
        if expected:
            self.assertEqual(snapshots[0]['pid'],77)
            self.assertEqual(snapshots[0]['text'],'')
            self.assertEqual(snapshots[0]['document_attributes'],['DocURL:https://localhost:36093/console/'])

    def observe(self,event_type):
        queries=[]
        class Document:
            def queryDocument(self):
                queries.append('query')
                return SimpleNamespace(getAttributes=lambda:['DocURL:https://localhost:36093/console/'])
            def queryText(self):return SimpleNamespace(getText=lambda start,end:'Visible document')
            def getApplication(self):return SimpleNamespace(get_process_id=lambda:77)
            def getRoleName(self):return 'document web'
            def getAttributes(self):return ['tag:body']
        class Desktop:
            childCount=0
            def queryDocument(self):raise NotImplementedError
        class Registry:
            def registerEventListener(self,callback,*events):self.callback=callback
            def getDesktopCount(self):return 1
            def getDesktop(self,index):return Desktop()
            def start(self):self.callback(SimpleNamespace(type=event_type,source=Document()))
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as root:
            path=Path(root)/'events.jsonl'
            with patch.dict(sys.modules,{'pyatspi':SimpleNamespace(Registry=Registry())}):probe.observe(path)
            return probe.read_rows(path),queries

    def test_late_real_document_gets_a_separate_identity_read(self):
        rows,queries=self.observe('document:load-complete')
        self.assertEqual([row['type'] for row in rows],['document:load-complete','document:snapshot'])
        self.assertEqual(queries,['query','query'])
        self.assertEqual(rows[0]['text'],'Visible document')
        self.assertEqual(rows[1]['text'],'')
        self.assertEqual(rows[1]['pid'],77)
        self.assertEqual(rows[1]['document_attributes'],['DocURL:https://localhost:36093/console/'])

    def test_nondocument_event_does_not_invent_document_identity(self):
        rows,queries=self.observe('object:property-change:accessible-name')
        self.assertEqual([row['type'] for row in rows],['object:property-change:accessible-name'])
        self.assertEqual(queries,[])

if __name__=='__main__':unittest.main()
