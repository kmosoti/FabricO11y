"""Literal ledger association controls; no native processes or sockets."""
import ast
import base64
import hashlib
import inspect
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

import simulator_native_controls as native
from test_simulator_native_controls import wire


class AccountingControls(unittest.TestCase):
    def fixture(self, pending=False):
        rows, raw, streams, events = [], {}, {}, []
        for index in range(2):
            node = bytes([index + 1]) * 16
            streams[str(index)] = ((node.hex(), 1), 0 if pending else 2)
            for sequence in (1, 2):
                payload = wire(sequence).replace(bytes(range(16)), node, 1)
                identity = dict(node_id=node.hex(), generation=1, sequence=sequence)
                digest = base64.b64encode(hashlib.sha256(payload).digest()).decode()
                rows.append(dict(type='source', bytes=digest, **identity))
                events.append(dict(e='created', id=index, seq=sequence))
                if not pending or sequence == 1:
                    raw[(node.hex(), 1, sequence)] = payload
                    rows.append(dict(type='attempt', bytes=digest, injected_conflict=False, **identity))
                    rows.append(dict(type='response', kind='unavailable' if pending else 'ack',
                                     **({} if pending else {'committed_through': sequence}), **identity))
            rows.append(dict(type='node_state', node_id=node.hex(), generation=1,
                             ack_cursor=0 if pending else 2, retained_sequences=[1, 2] if pending else []))
        return rows, events, SimpleNamespace(raw=raw, streams=streams, errors=[])

    def check(self, fixture, pending=False):
        rows, events, sink = fixture
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as name:
            root = Path(name)
            for file, values in (('transcript.jsonl', rows), ('events.jsonl', events)):
                (root / file).write_text(''.join(json.dumps(row) + '\n' for row in values))
            native.original_records(root, sink, 2, pending)

    def test_literal_complete_and_pending_ledgers(self):
        for pending in (False, True): self.check(self.fixture(pending), pending)

    def test_missing_attempt_refused(self):
        rows, events, sink = self.fixture(); rows.remove(next(row for row in rows if row['type'] == 'attempt'))
        with self.assertRaises(ValueError): self.check((rows, events, sink))

    def test_unrelated_source_cannot_replace_an_accepted_source(self):
        fixture = self.fixture(); fixture[0][0]['node_id'] = 'ff' * 16
        with self.assertRaises(ValueError): self.check(fixture)

    def test_unrelated_attempt_cannot_replace_a_wire_attempt(self):
        fixture = self.fixture(); next(row for row in fixture[0] if row['type'] == 'attempt')['node_id'] = 'ff' * 16
        with self.assertRaises(ValueError): self.check(fixture)

    def test_response_identity_and_multiplicity_refused(self):
        for duplicate in (False, True):
            fixture = self.fixture(); replies = [row for row in fixture[0] if row['type'] == 'response']
            if duplicate: replies[-1].update(replies[0])
            else: replies[0]['node_id'] = 'ff' * 16
            with self.subTest(duplicate=duplicate), self.assertRaises(ValueError): self.check(fixture)

    def test_state_identity_and_multiplicity_refused(self):
        for pending in (False, True):
            fixture = self.fixture(pending); states = [row for row in fixture[0] if row['type'] == 'node_state']
            states[-1].update(states[0])
            with self.subTest(pending=pending), self.assertRaises(ValueError): self.check(fixture, pending)

    def test_pending_unoffered_source_still_belongs_to_expected_stream(self):
        fixture = self.fixture(True)
        next(row for row in fixture[0] if row['type'] == 'source' and row['sequence'] == 2)['node_id'] = 'ff' * 16
        with self.assertRaises(ValueError): self.check(fixture, True)

    def test_boolean_identity_cannot_alias_an_integer(self):
        for kind in ('source', 'attempt', 'response', 'node_state'):
            fixture = self.fixture()
            next(row for row in fixture[0] if row['type'] == kind)['generation'] = True
            with self.subTest(kind=kind), self.assertRaises(ValueError): self.check(fixture)

    def test_canonical_argv_uses_only_sealed_supported_options(self):
        body = next(node for node in ast.parse(inspect.getsource(native.main)).body[0].body if isinstance(node, ast.Try)).body
        start = next(i for i, node in enumerate(body) if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name) and node.targets[0].id == 'command')
        end = next(i for i in range(start + 1, len(body)) if isinstance(body[i], ast.Assign))
        namespace = dict(fd=42, sink=SimpleNamespace(server_port=1234), root=Path('/literal'),
                         tokens=Path('/literal/tokens'), workers=2, begin=Path('/literal/begin.json'),
                         args=SimpleNamespace(case='canonical-red'))
        exec(compile(ast.Module(body=body[start:end], type_ignores=[]), '<actual-argv>', 'exec'), namespace)
        self.assertEqual(namespace['command'], ['/proc/self/fd/42', '--server-url', 'https://127.0.0.1:1234',
            '--ca', '/literal/ca.pem', '--tokens', '/literal/tokens', '--seed', '0xA11FA001',
            '--seconds', '2', '--workers', '2', '--out', '/literal'])
