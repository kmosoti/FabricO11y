"""Origin: native exact-package401 denial refused by the old404 assumption."""
import json
from pathlib import Path
import unittest
import test_browser_migration as fixture
from migration_contract import grade

OBSERVED=json.loads((Path(__file__).resolve().parents[2]/
    'docs/experiments/formal/fixtures/migration-admin-denial.json').read_text())

class NativeAdminDenial(unittest.TestCase):
    def test_observed_native_denial_is_the_exact_required_refusal(self):
        row=fixture.MigrationContract().literal()
        row.update(OBSERVED['observed']);row['master']=tuple(row['master'])
        self.assertTrue(grade(row))

if __name__=='__main__':unittest.main()
