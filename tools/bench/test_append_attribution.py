"""Discriminating checks for S0 CSV validation; no benchmark timings here."""
import csv
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

PATH = Path(__file__).with_name('append_attribution.py')
spec = importlib.util.spec_from_file_location('attribution', PATH)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class CsvChecks(unittest.TestCase):
    def test_clean_and_three_mutations(self):
        rows = []
        for i in [1, 2]:
            rows.append(['append',i,100,'','','','','',''])
            rows.extend([[phase,i,10,'','','','','',''] for phase in module.PHASES])
        rows.append(['ingest','',250,2,0,296,100,'',''])
        cases = {'clean': rows,
                 'missing_phase': rows[:1]+rows[2:],
                 'duplicate_phase': rows[:2]+[rows[1]]+rows[2:],
                 'phase_exceeds_outer': [r if r[0]!='data_sync' else [*r[:2],101,*r[3:]] for r in rows]}
        with tempfile.TemporaryDirectory() as directory:
            for name, case in cases.items():
                path = Path(directory)/(name+'.csv')
                with path.open('w',newline='') as stream:
                    writer = csv.writer(stream)
                    writer.writerow(module.HEADER)
                    writer.writerows(case)
                command = [sys.executable,'-B',str(PATH),'validate',str(path),'--instrumented','--count','2']
                result = subprocess.run(command,text=True,capture_output=True)
                print(json.dumps({'case':name,'input':case,'command':command,'exit':result.returncode,'stdout':result.stdout,'stderr':result.stderr}))
                self.assertEqual(result.returncode,0 if name=='clean' else 1)
                if name=='clean':
                    self.assertEqual(json.loads(result.stdout)['sync_fraction'],.2)


if __name__=='__main__':
    unittest.main()
