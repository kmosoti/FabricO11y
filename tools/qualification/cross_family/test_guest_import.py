"""The actual guest loader must work without an ambient install PYTHONPATH."""
import os
from pathlib import Path
import subprocess
import sys
import unittest


class IsolatedGuestImport(unittest.TestCase):
    def test_production_loader_resolves_its_own_native_helpers(self):
        guest = Path(__file__).resolve().with_name('guest.py')
        program = '''import importlib.util, pathlib
path=pathlib.Path(%r)
spec=importlib.util.spec_from_file_location('production_guest_probe',path)
module=importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
assert pathlib.Path(module.Q.custody_native.__file__).resolve()==path.parent.parent/'install/custody_native.py'
assert module.Q.DATA==pathlib.Path('/run/media/kmosoti/data/FabricO11y')
print('isolated-production-import-ok')
''' % str(guest)
        outcome = subprocess.run([sys.executable, '-I', '-B', '-c', program],
            cwd=os.environ['FABRIC_SCRATCH_ROOT'], capture_output=True, text=True, timeout=10)
        self.assertEqual(outcome.returncode, 0, outcome.stderr)
        self.assertEqual(outcome.stdout.strip(), 'isolated-production-import-ok')
