import copy
import unittest
from migration_contract import CANARY, FORMAT_ERROR, grade

class MigrationContract(unittest.TestCase):
    def literal(self):
        answer={'rows':[{'node':'migration-edge','body':CANARY,'observed_ns':1234567890123456789}], 'next_page':None}
        return dict(legacy=copy.deepcopy(answer),owner=copy.deepcopy(answer),reader=copy.deepcopy(answer),cli=copy.deepcopy(answer),master=(401,'sign in required'),admin_status=401,admin_error='use a console credential',format_exit=1,format_stderr=FORMAT_ERROR,stopped_unchanged=True,prefixes_preserved=True,format_unchanged=True)
    def test_literal_valid(self): self.assertTrue(grade(self.literal()))
    def test_canary_changed_dropped_duplicated_or_paginated(self):
        for field in ('owner','reader','cli'):
            for kind in ('changed','dropped','duplicated','paginated'):
                with self.subTest(field=field,kind=kind):
                    r=self.literal()
                    if kind=='changed':r[field]['rows'][0]['observed_ns']+=1
                    if kind=='dropped':r[field]['rows']=[]
                    if kind=='duplicated':r[field]['rows']*=2
                    if kind=='paginated':r[field]['next_page']='hidden'
                    with self.assertRaisesRegex(ValueError,'exact complete canary'):grade(r)
    def test_unrelated_master_or_format_failure_is_not_detection(self):
        for field,value in [('master',(429,'sign in required')),('master',(401,'wrong')),('master',(200,None)),('admin_status',404),('admin_status',200),('admin_status',429),('admin_status',None),('admin_error','wrong'),('admin_error',''),('admin_error',None),('format_exit',127),('format_stderr','TLS unavailable')]:
            r=self.literal();r[field]=value
            with self.assertRaises(ValueError):grade(r)
    def test_missing_admin_denial_body_is_refused(self):
        r=self.literal();del r['admin_error']
        with self.assertRaisesRegex(ValueError,'old-master refusal'):grade(r)
    def test_changed_original_state_is_refused(self):
        for field in ('stopped_unchanged','prefixes_preserved','format_unchanged'):
            r=self.literal();r[field]=False
            with self.assertRaisesRegex(ValueError,'original state'):grade(r)
