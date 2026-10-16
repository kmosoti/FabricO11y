import copy
import unittest
import browser_responses as responses


def complete():
    rows=[]
    for case in ('empty','incomplete','gap','410','429','503','transport','timeout','403','401'):
        row=dict(case=case,text='',alert='',rows=0,dispatches=1,busy=False,locked=case in ('401','403'),sensitive_present=False,abort_observed=True,elapsed_ns=15_000_000_000)
        if case=='empty':row['text']='0 observations'
        elif case=='incomplete':row['text']='Incomplete coverage'
        elif case=='gap':row['text']='1 reported gaps'
        elif case in ('401','403','410','429','503'):
            detail={'401':'sign in required','403':'access denied','410':'page expired; run the query again','429':'request rate limit','503':'service unavailable'}[case]
            row['alert']='HTTP '+case+': '+detail+'; no automatic retry'
        else:row['alert']='Request interrupted, offline, or exceeded its 15-second deadline.'
        rows.append(row)
    return rows


class UIResponseControls(unittest.TestCase):
    def test_literal_positive(self):self.assertTrue(responses.grade(complete()))
    def test_missing_duplicate_case(self):
        for i in range(10):
            with self.assertRaisesRegex(ValueError,'inventory'):responses.grade(complete()[:i]+complete()[i+1:])
            with self.assertRaisesRegex(ValueError,'inventory'):responses.grade(complete()+[complete()[i]])
    def test_each_property_and_automatic_retry(self):
        for i in range(10):
            for field,value in [('dispatches',2),('busy',True),('text',''),('alert','')]:
                if field=='text' and i>=3 or field=='alert' and i<3:continue
                rows=complete();rows[i][field]=value
                with self.assertRaisesRegex(ValueError,'property rejected: '+rows[i]['case']):responses.grade(rows)
    def test_denial_does_not_leak_rows(self):
        for i in (8,9):
            for field,value in [('locked',False),('rows',1),('sensitive_present',True)]:
                rows=complete();rows[i][field]=value
                with self.assertRaisesRegex(ValueError,'property rejected'):responses.grade(rows)
    def test_timeout_requires_actual_abort_and_elapsed(self):
        for field,value in [('abort_observed',False),('elapsed_ns',14_499_999_999)]:
            rows=complete();rows[7][field]=value
            with self.assertRaisesRegex(ValueError,'timeout'):responses.grade(rows)

if __name__=='__main__':unittest.main()
