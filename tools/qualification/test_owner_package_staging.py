"""Reversed owner pair02: canonical Debian '~' must not widen remote paths."""
import hashlib
import os
from pathlib import Path
import re
import tempfile
from types import SimpleNamespace
import unittest
from owner_walkthrough import Walkthrough


class PackageStagingTests(unittest.TestCase):
    def exercise(self, role, family, candidate):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root = Path(directory)
            package = root / ('fabrico11y_0.1.0~alpha.1_amd64.deb' if family == 'debian'
                              else 'fabrico11y-0.1.0-0.alpha.1.x86_64.rpm')
            package.write_bytes(b'literal unchanged package input')
            copied, events = {}, []
            guest = SimpleNamespace(role=role, family=family,
                copy=lambda path, destination: copied.update({destination: Path(path).read_bytes()}))
            walk = Walkthrough.__new__(Walkthrough)
            walk.root, walk.artifacts = root, {family: package}
            walk.fixture = {'packages': {family: {'package': str(package)}}}
            walk.prose = lambda *args, **kw: None
            walk.fence = lambda *args, **kw: None
            walk.installed_payload = lambda *args: None
            walk.event = lambda kind, **kw: events.append(kw)
            walk.install(guest, candidate)
            name = next(path.removeprefix('/home/accept/') for path in copied if path != '/home/accept/SHA256SUMS')
            self.assertEqual(copied['/home/accept/' + name], package.read_bytes())
            self.assertEqual(copied['/home/accept/SHA256SUMS'].decode(), hashlib.sha256(package.read_bytes()).hexdigest() + '  ' + name + '\n')
            self.assertEqual(events[0]['package'], package.name)
            return name, events[0]

    def test_both_debian_edge_variants_fit_the_unchanged_remote_guard(self):
        for candidate in (False, True):
            name, event = self.exercise('edge', 'debian', candidate)
            self.assertRegex('/home/accept/' + name, r'^/home/accept/[a-zA-Z0-9_.-]+$')
            self.assertEqual(name, 'fabrico11y_' + ('candidate' if candidate else 'predecessor') + '.deb')
            self.assertEqual(event['guest_filename'], name)
        self.assertIsNone(re.fullmatch(r'/home/accept/[a-zA-Z0-9_.-]+', '/home/accept/fabrico11y_0.1.0~alpha.1_amd64.deb'))

    def test_accepted_first_direction_filenames_and_event_shape_are_unchanged(self):
        for role, family in [('server', 'debian'), ('edge', 'fedora'), ('server', 'fedora')]:
            name, event = self.exercise(role, family, True)
            self.assertEqual(name, event['package'])
            self.assertNotIn('guest_filename', event)
