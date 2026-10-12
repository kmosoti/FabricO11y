"""ADR-derived route coverage; origin: issue45/C-027 and the missing legacy rows."""
import ast
import copy
import json
from pathlib import Path
import re
from types import SimpleNamespace
import unittest

import live_browser_check as browser

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / 'docs/experiments/formal/fixtures/console-route-roles.json'


def registered_routes(source):
    routes = set()
    for match in re.finditer(r'\.route\(\s*"([^"]+)"\s*,', source):
        depth, end = 1, match.end()
        while depth:
            depth += (source[end] == '(') - (source[end] == ')')
            end += 1
        for method in re.findall(r'\b(get|post|put|patch|delete|head|options)\s*\(', source[match.end():end]):
            routes.add((method.upper(), match.group(1)))
    return routes


def witnessed_calls(request, check):
    if hasattr(browser, 'run_role_matrix'):
        browser.run_role_matrix(request, check)
        return
    # Execute ONLY the retained selected-matrix AST against synthetic transports.
    # No main(), browser, sockets, subprocesses or native authority are invoked.
    main = next(node for node in ast.parse(Path(browser.__file__).read_text()).body
                if isinstance(node, ast.FunctionDef) and node.name == 'main')
    statements = next(node.body for node in ast.walk(main) if isinstance(node, ast.Try)
                      and any(isinstance(n, ast.Assign) and any(isinstance(t, ast.Name) and t.id == 'owner_routes'
                                                               for t in n.targets) for n in node.body))
    begin = next(i for i, node in enumerate(statements) if isinstance(node, ast.Assign)
                 and any(isinstance(t, ast.Name) and t.id == 'owner_routes' for t in node.targets))
    end = next(i for i in range(begin + 1, len(statements)) if isinstance(statements[i], ast.Assign)
               and any(isinstance(t, ast.Name) and t.id == 'read' for t in statements[i].targets))
    namespace = {'credential': 'reader', 'spindle_token': 'spindle', 'check': check,
                 'bearer_api': lambda role, path, method='GET', body=None: request(role or 'anonymous', method, path, body),
                 'api': lambda path, method='GET', body=None: request('owner', method, path, body),
                 'expect': lambda answer, status, message, label: check(browser.semantic_rejection(answer, status, message), label)}
    exec(compile(ast.Module(body=statements[begin:end], type_ignores=[]), '<retained-matrix>', 'exec'), namespace)


class RoleMatrixTests(unittest.TestCase):
    def setUp(self):
        self.fixture = json.loads(FIXTURE.read_text())
        self.expected = {(role, method, path): outcome
                         for method, _, path, _, outcomes in self.fixture['rows']
                         for role, outcome in zip(self.fixture['roles'], outcomes)}

    def request(self, role, method, path, body=None):
        # The retained fixture targeted browser-edge; the successor owns matrix.
        path = path.replace('/v1/console/nodes/browser-edge/', '/v1/console/nodes/matrix/')
        status, error = self.expected.get((role, method, path), (200, None))
        field = self.fixture['refusal_fields'].get(path, 'error')
        return status, {field: error} if error else {}

    def test_fixture_covers_both_registered_routers_and_all_control_actions(self):
        rows = self.fixture['rows']
        self.assertEqual(self.fixture['roles'], ['anonymous', 'spindle', 'reader', 'owner'])
        self.assertEqual(len(rows), 36)
        self.assertEqual(len(self.expected), 144)
        registered = set().union(*(registered_routes((ROOT / 'crates/fabric-server/src' / name).read_text())
                                  for name in ('console.rs', 'http.rs')))
        self.assertEqual({(row[0], row[1]) for row in rows}, registered)
        for prefix in ('/v1/console', '/v1/admin'):
            self.assertEqual({row[2].rsplit('/', 1)[1] for row in rows
                              if row[1] == prefix + '/nodes/{name}/{action}'}, {'pause', 'resume', 'revoke'})

    def test_live_matrix_calls_all_credentials_on_every_route_and_legacy_action(self):
        calls = []

        def request(role, method, path, body):
            calls.append((role, method, path))
            return self.request(role, method, path, body)

        witnessed_calls(request, self.assertTrue)
        self.assertEqual(set(calls), set(self.expected),
                         'live role matrix missing cells: ' + repr(sorted(set(self.expected) - set(calls))))
        self.assertEqual(len(calls), 144, 'each registered role-route cell must run exactly once')

    def test_reader_admission_mutation_is_rejected_by_the_live_checker(self):
        def request(role, method, path, body):
            if (role, method, path) == ('reader', 'GET', '/v1/console/audit'):
                return 200, {'events': []}
            return self.request(role, method, path, body)

        with self.assertRaisesRegex(AssertionError, 'reader.*GET /v1/console/audit'):
            witnessed_calls(request, self.assertTrue)

    def test_independent_outcomes_never_accept_admission_refusal_or_role_crossover(self):
        for role, path, expected in [
            ('spindle', '/v1/console/status', [401, 'sign in required']),
            ('reader', '/v1/console/audit', [403, 'role not permitted']),
            ('owner', '/v1/admin/nodes', [401, 'use a console credential']),
        ]:
            self.assertEqual(self.expected[(role, 'GET', path)], expected)
            self.assertFalse(browser.semantic_rejection((429, {'error': expected[1]}), *expected))
            self.assertFalse(browser.semantic_rejection((200, {'error': expected[1]}), *expected))
            self.assertFalse(browser.semantic_rejection((expected[0], {'error': 'wrong refusal'}), *expected))
        missing = copy.deepcopy(self.fixture['rows'])
        missing.remove(next(row for row in missing if row[0:3] ==
                            ['POST', '/v1/admin/nodes/{name}/{action}', '/v1/admin/nodes/matrix/resume']))
        self.assertEqual(len({(r[0], r[1]) for r in missing}), len({(r[0], r[1]) for r in self.fixture['rows']}))
        self.assertNotEqual({(r[0], r[2]) for r in missing}, {(r[0], r[2]) for r in self.fixture['rows']})

    def test_transports_refuse_expired_or_storage_limited_dispatch_and_progressing_body(self):
        # Origin: Miso's independent review of0de4b46; no main/network/process.
        source = ast.parse(Path(browser.__file__).read_text())
        main = next(n for n in source.body if isinstance(n, ast.FunctionDef) and n.name == 'main')
        functions = [n for n in source.body if isinstance(n, ast.FunctionDef) and n.name != 'main']
        functions += [n for n in main.body if isinstance(n, ast.FunctionDef) and n.name in ('call', 'bearer_api')]
        for transport in ('call', 'bearer_api'):
            for case in ('expired', 'storage', 'paced_expiry', 'timeout', 'progressing'):
                if transport == 'call' and case == 'paced_expiry':continue
                with self.subTest(transport=transport, case=case):
                    now, opened, reads = [2.0 if case == 'expired' else 0.0], [], []
                    class Response:
                        status = 200
                        fp = SimpleNamespace(raw=SimpleNamespace(_sock=SimpleNamespace(settimeout=lambda _: None)))
                        def __enter__(self):return self
                        def __exit__(self, *_):pass
                        def read(self, size=None):
                            reads.append('whole')
                            if case == 'progressing':now[0] = 2.0
                            return b'{"value":{"ready":true}}'
                        def read1(self, size):
                            reads.append('chunk')
                            if case == 'progressing':now[0] = 2.0
                            return b'{"value":{"ready":true}}' if len(reads) == 1 else b''
                    def open_response(request, **kwargs):
                        opened.append(kwargs['timeout'])
                        return Response()
                    namespace = dict(vars(browser))
                    namespace.update(time=SimpleNamespace(monotonic=lambda:now[0], monotonic_ns=lambda:int(now[0]*1e9)),
                                     deadline=1.0, storage_violation=SimpleNamespace(is_set=lambda:case == 'storage'),
                                     endpoint='http://localhost:1', origin='https://localhost:1', receipt={},
                                     args=SimpleNamespace(inject_role_defect=False), tls_opener=SimpleNamespace(open=open_response),
                                     pace_api=lambda:now.__setitem__(0, 2.0 if case == 'paced_expiry' else now[0]),
                                     cadence=SimpleNamespace(clock=lambda:now[0], last=None),
                                     urllib=SimpleNamespace(request=SimpleNamespace(Request=browser.urllib.request.Request, urlopen=open_response),
                                                            error=browser.urllib.error))
                    exec(compile(ast.Module(body=functions, type_ignores=[]), '<transport-boundary>', 'exec'), namespace)
                    invoke = (lambda:namespace['call']('GET', '/status')) if transport == 'call' else (lambda:namespace['bearer_api'](None, '/v1/console/nodes'))
                    if case == 'timeout':
                        invoke()
                        self.assertEqual(len(opened), 1)
                        self.assertGreater(opened[0], 0)
                        self.assertLessEqual(opened[0], 1.0, 'socket timeout exceeds remaining inner deadline')
                    else:
                        with self.assertRaisesRegex(RuntimeError, 'deadline|storage|512 MiB'):
                            invoke()
                        if case == 'progressing':self.assertEqual(reads, ['chunk'], 'wall bound must stop a progressively readable body')
                        else:self.assertEqual(opened, [], 'expired/storage-limited dispatch reached network')


if __name__ == '__main__':
    unittest.main()
