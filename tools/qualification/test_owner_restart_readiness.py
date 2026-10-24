"""Type=exec is not a listener promise; auth outcomes remain one-shot checks."""
import io
import ssl
from contextlib import redirect_stdout
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import urllib.error
from owner_walkthrough import RESTART_READINESS


class RestartReadinessTests(unittest.TestCase):
    def run_observer(self, responses):
        clock = [0.0]
        response = SimpleNamespace(status=200)
        context = unittest.mock.MagicMock()
        context.__enter__.return_value = response
        answers = [context if x == 200 else x for x in responses]
        def advance(seconds):
            clock[0] += seconds
        script = RESTART_READINESS.split("python3 - <<'READY'\n", 1)[1].rsplit('\nREADY', 1)[0]
        with patch('ssl.create_default_context', return_value='verified-context') as ca, \
             patch('urllib.request.urlopen', side_effect=answers) as request, \
             patch('time.monotonic', side_effect=lambda: clock[0]), \
             patch('time.sleep', side_effect=advance), redirect_stdout(io.StringIO()):
            exec(compile(script, '<registered-restart-readiness>', 'exec'), {})
        ca.assert_called_once_with(cafile='/etc/fabrico11y/ca.pem')
        self.assertEqual(request.call_args.args[0], 'https://telemetry.example.com:7443/console/index.html')
        self.assertEqual(request.call_args.kwargs['context'], 'verified-context')
        return request.call_count

    def test_connection_refused_before_listener_is_observed_then_public_tls_succeeds(self):
        self.assertEqual(self.run_observer([urllib.error.URLError(ConnectionRefusedError()), 200]), 2)

    def test_bad_certificate_and_http_denial_do_not_become_readiness_success(self):
        for failure in [urllib.error.URLError(ssl.SSLCertVerificationError('untrusted')),
                        urllib.error.HTTPError('https://example', 401, 'denied', {}, None),
                        urllib.error.URLError('unexpected name resolution failure')]:
            with self.subTest(failure=type(failure).__name__), self.assertRaises(type(failure)):
                self.run_observer([failure])

    def test_absent_listener_expires_without_retrying_any_authorized_request(self):
        with self.assertRaisesRegex(TimeoutError, 'verified listener readiness deadline'):
            self.run_observer([urllib.error.URLError(ConnectionRefusedError())] * 300)
