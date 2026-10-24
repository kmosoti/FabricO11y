"""Reader-authority probe controls from the failed fresh-owner trial."""
import contextlib
import io
import json
import os
from pathlib import Path
import shlex
import ssl
import tempfile
import time
import types
import unittest
import urllib.error
from unittest.mock import patch

from owner_walkthrough import Walkthrough


class ReaderProbeTests(unittest.TestCase):
    def exercise(self, status=200, failure=None):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as temporary:
            output = Path(temporary) / 'stdout'
            walk = Walkthrough.__new__(Walkthrough)
            walk.edge = object()
            token = 'a' * 64
            calls = []

            def request(value, *, context, timeout):
                self.assertEqual(value.full_url, 'https://telemetry.example.com:7443/v1/console/query')
                query = json.loads(value.data)
                self.assertEqual((query['kind'], query['node'], query['limit']), ('logs', 'edge', 1))
                self.assertEqual(query['to_ns'] - query['from_ns'], 900 * 10**9)
                self.assertLess(abs(time.time_ns() - query['to_ns']), 5 * 10**9)
                self.assertEqual(value.get_header('Authorization'), 'Bearer ' + token)
                self.assertEqual(timeout, 5)
                calls.append(value)
                if failure is not None:
                    raise failure
                if status != 200:
                    raise urllib.error.HTTPError(value.full_url, status, 'fixture denial', {}, None)
                return types.SimpleNamespace(status=status, close=lambda: None)

            def prose(guest, heading, needle, command, **kwargs):
                self.assertIs(guest, walk.edge)
                self.assertNotIn(token, command)
                self.assertEqual(kwargs['stdin'], token.encode())
                argv = shlex.split(command)
                self.assertEqual(argv[:2], ['python3', '-c'])
                stdout = io.StringIO()
                with patch('sys.stdin', io.StringIO(token)), contextlib.redirect_stdout(stdout), \
                        patch('ssl.create_default_context', return_value=object()) as trust, \
                        patch('urllib.request.urlopen', side_effect=request):
                    exec(compile(argv[2], '<actual-reader-probe>', 'exec'), {})
                trust.assert_called_once_with(cafile='/home/operator/.config/fabrico11y/ca.pem')
                output.write_text(stdout.getvalue())
                return {'stdout': {'path': str(output)}}

            walk.prose = prose
            value = walk.reader_status(token)
            self.assertEqual(len(calls), 1)
            return value

    def test_authorized_probe_uses_a_valid_recent_query_and_private_stdin(self):
        self.assertEqual(self.exercise(), 200)

    def test_http_refusals_keep_the_original_status(self):
        for status in (401, 403, 400):
            with self.subTest(status=status):
                self.assertEqual(self.exercise(status), status)

    def test_tls_failure_is_not_an_authorization_denial(self):
        with self.assertRaises(ssl.SSLError):
            self.exercise(failure=ssl.SSLError('fixture certificate refusal'))
