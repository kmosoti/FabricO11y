"""Retain alert-owner-debian-fedora-03's shell-expanded dpkg format failure."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest

import owner_walkthrough as W


class NativeVersion(unittest.TestCase):
    def inspect(self, family, candidate, actual=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            name = 'dpkg-query' if family == 'debian' else 'rpm'
            wanted = (['-W', '-f=${Version}', 'fabrico11y'] if family == 'debian'
                      else ['-q', '--qf', '%{VERSION}-%{RELEASE}', 'fabrico11y'])
            command = root / name
            command.write_text('#!' + sys.executable + '\nimport os,sys\n'
                + 'assert sys.argv[1:] == ' + repr(wanted) + ', sys.argv[1:]\n'
                + "print(os.environ['PROBE_VERSION'])\n")
            command.chmod(0o700)
            expected = ('0.1.0~alpha.' if family == 'debian' else '0.1.0-0.alpha.') + ('1' if candidate else '0')
            env = dict(os.environ, PATH=str(root) + ':' + os.environ['PATH'],
                       Version='unexpected-shell-expansion', PROBE_VERSION=actual or expected)
            observations = []
            def observed(guest, script, label):
                if label.startswith('payload-'):
                    return '{}'
                result = subprocess.run(['bash', '-s'], input=script, env=env,
                                        capture_output=True, text=True, check=True)
                return result.stdout
            walk = types.SimpleNamespace(artifacts={'identity': {'payload_identity': {}}},
                record={'events': []}, observed=observed,
                event=lambda *args, **values: observations.append(values))
            W.Walkthrough.installed_payload(walk, types.SimpleNamespace(family=family, role='server'), candidate)
            return observations

    def test_shell_preserves_literal_format_for_both_native_versions_and_families(self):
        for family in ('debian', 'fedora'):
            for candidate in (False, True):
                with self.subTest(family=family, candidate=candidate):
                    self.assertEqual(len(self.inspect(family, candidate)), 1)

    def test_wrong_installed_version_still_refuses(self):
        for family in ('debian', 'fedora'):
            with self.subTest(family=family), self.assertRaisesRegex(ValueError, 'installed native package version differs'):
                self.inspect(family, True, actual='unexpected-installed-version')


if __name__ == '__main__':
    unittest.main()
