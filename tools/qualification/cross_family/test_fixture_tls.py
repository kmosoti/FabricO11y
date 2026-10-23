"""Smoke02 CA extension counterexample, checked by strict OpenSSL verification."""
from pathlib import Path
import subprocess
import tempfile
import unittest

import run as campaign


class FixtureTLS(unittest.TestCase):
    def test_fixture_chain_is_strict_and_missing_ca_usage_is_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            campaign.fixture_tls(root)
            verify = ['openssl', 'verify', '-x509_strict', '-purpose', 'sslserver',
                      '-verify_ip', '127.0.0.1', '-CAfile', str(root / 'ca.pem'),
                      str(root / 'server.pem')]
            positive = subprocess.run(verify, text=True, capture_output=True)
            self.assertEqual(positive.returncode, 0, positive.stderr)
            # Reproduce the retained CA defect independently of the producer.
            subprocess.run(['openssl', 'req', '-x509', '-new', '-key', str(root / 'ca.key'),
                '-config', '/dev/null', '-subj', '/CN=cross-family-owned-ca',
                '-addext', 'basicConstraints=critical,CA:TRUE', '-days', '2',
                '-out', str(root / 'bad-ca.pem')], check=True, capture_output=True)
            negative = subprocess.run([x.replace('/ca.pem', '/bad-ca.pem') for x in verify],
                                      text=True, capture_output=True)
            self.assertNotEqual(negative.returncode, 0)
            self.assertIn('CA cert does not include key usage extension', negative.stderr)


class PersistentFixtureTLS(unittest.TestCase):
    def fixture(self):
        import contextlib
        import http.server
        import json
        import socket
        import ssl
        import threading
        @contextlib.contextmanager
        def owned():
            with tempfile.TemporaryDirectory() as name:
                root = Path(name)
                campaign.fixture_tls(root)
                state = {'requests': [], 'accepted': 0}
                class Handler(http.server.BaseHTTPRequestHandler):
                    protocol_version = 'HTTP/1.1'
                    def log_message(self, *args):
                        pass
                    def do_GET(self):
                        self.handle_fixture()
                    def do_POST(self):
                        self.handle_fixture()
                    def handle_fixture(self):
                        body = self.rfile.read(int(self.headers.get('content-length', '0')))
                        state['requests'].append((self.path, self.headers.get('authorization'), body))
                        if self.path == '/disconnect':
                            self.close_connection = True
                            self.connection.shutdown(socket.SHUT_RDWR)
                            self.connection.close()
                            return
                        status = 401 if self.headers.get('authorization') != 'Bearer fixture-token' else 200
                        raw = json.dumps({'ok': True}).encode()
                        if self.path == '/error':
                            status, raw = 403, b'{"error":"fixture refusal"}'
                        if self.path == '/oversized':
                            raw = b'x' * 128
                        if self.path == '/partial':
                            self.send_response(200)
                            self.send_header('Content-Length', str(len(raw)+5))
                            self.end_headers()
                            self.wfile.write(raw)
                            self.wfile.flush()
                            self.close_connection = True
                            return
                        self.send_response(status)
                        self.send_header('Content-Length', str(len(raw)))
                        self.end_headers()
                        self.wfile.write(raw)
                class Server(http.server.ThreadingHTTPServer):
                    daemon_threads = False
                    def get_request(self):
                        answer = super().get_request()
                        state['accepted'] += 1
                        return answer
                server = Server(('127.0.0.1', 0), Handler)
                context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
                context.load_cert_chain(root/'server.pem', root/'server.key')
                server.socket = context.wrap_socket(server.socket, server_side=True)
                thread = threading.Thread(target=lambda: server.serve_forever(poll_interval=.01))
                thread.start()
                clients = []
                def client(host='127.0.0.1', context=None):
                    value = campaign.FixtureHTTPS(server.server_port,
                        context or ssl.create_default_context(cafile=str(root/'ca.pem')),
                        'fixture-token', host=host)
                    clients.append(value)
                    return value
                try:
                    yield client, state, server, root
                finally:
                    for value in clients:
                        self.assertTrue(value.close())
                    server.shutdown()
                    server.server_close()
                    thread.join(timeout=3)
                    self.assertFalse(thread.is_alive())
        return owned()

    def test_reuses_two_owned_connections_with_authentication_on_every_request(self):
        import threading
        with self.fixture() as (make, state, server, root):
            client = make()
            self.assertEqual(client.call('/ok'), {'ok': True})
            self.assertEqual(client.call('/ok', {'original': 1}), {'ok': True})
            errors = []
            def second():
                try:
                    client.call('/ok')
                    client.call('/ok')
                except BaseException as error:
                    errors.append(error)
            thread = threading.Thread(target=second)
            thread.start()
            thread.join(timeout=3)
            self.assertFalse(thread.is_alive())
            self.assertFalse(errors)
            self.assertEqual(state['accepted'], 2)
            self.assertEqual(len(state['requests']), 4)
            self.assertTrue(all(auth == 'Bearer fixture-token' for _, auth, _ in state['requests']))
            def third():
                try:
                    client.call('/ok')
                except BaseException as error:
                    errors.append(error)
            thread = threading.Thread(target=third)
            thread.start()
            thread.join(timeout=3)
            self.assertEqual(len(errors), 1)
            self.assertIsInstance(errors[0], ValueError)
            self.assertIn('connection budget', str(errors[0]))
            self.assertEqual(state['accepted'], 2)
            self.assertTrue(client.close())
            self.assertTrue(all(c.sock is None for c in client.connections))
            with self.assertRaisesRegex(ValueError, 'already closed'):
                client.call('/ok')

    def test_real_tls_requires_trusted_ca_and_matching_hostname(self):
        import ssl
        with self.fixture() as (make, state, server, root):
            self.assertEqual(make().call('/ok'), {'ok': True})
            for value in (make(context=ssl.create_default_context()), make(host='localhost')):
                with self.assertRaises(ssl.SSLCertVerificationError):
                    value.call('/ok')
            with self.assertRaisesRegex(ValueError, 'strict fixture TLS'):
                make(context=ssl._create_unverified_context())

    def test_error_body_and_disconnect_remain_original_failures_without_retry(self):
        import urllib.error
        import http.client
        with self.fixture() as (make, state, server, root):
            client = make()
            with self.assertRaises(urllib.error.HTTPError) as raised:
                client.call('/error')
            self.assertEqual(raised.exception.code, 403)
            self.assertEqual(raised.exception.read(), b'{"error":"fixture refusal"}')
            raised.exception.close()
            with self.assertRaises(OSError):
                client.call('/disconnect')
            self.assertEqual(sum(path == '/disconnect' for path, _, _ in state['requests']), 1)
            self.assertTrue(all(c.sock is None for c in client.connections))
            with self.assertRaises(http.client.IncompleteRead):
                client.call('/partial')
            self.assertEqual(sum(path == '/partial' for path, _, _ in state['requests']), 1)
            self.assertTrue(all(c.sock is None for c in client.connections))
            client.MAX_RESPONSE_BYTES = 64
            with self.assertRaisesRegex(ValueError, 'bounded allowance'):
                client.call('/oversized')
            self.assertTrue(all(c.sock is None for c in client.connections))


if __name__ == '__main__':
    unittest.main()
