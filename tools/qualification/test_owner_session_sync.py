"""Native bridge login must be reflected in the PWA before a UI mutation."""
import time
from types import SimpleNamespace
import unittest
from owner_walkthrough import Walkthrough


class SessionSyncTests(unittest.TestCase):
    def walk(self, status=401, principal='owner'):
        events = []
        walk = Walkthrough.__new__(Walkthrough)
        walk.record, walk.principal = {}, 'owner'
        walk.bridge = SimpleNamespace(principal_id=principal,
            _request=lambda route: (status, {}),
            login=lambda: events.append('native-login'),
            session_view=lambda: dict(principal_id=principal, fresh_until_unix_s=int(time.time()) + 300))
        walk.browser_observers = lambda: events.append('observe-new-view')
        walk.click = lambda text: events.append(text)
        walk.ui_wait = lambda predicate: events.append('wait-pwa-response')
        walk.event = lambda *args, **kw: events.append('record-fresh-login')
        return walk, events

    def test_native_login_is_followed_by_real_pwa_refresh_before_completion(self):
        walk, events = self.walk()
        walk.restart_login()
        self.assertEqual(events, ['native-login', 'observe-new-view', 'Check session',
                                  'wait-pwa-response', 'record-fresh-login'])
        self.assertEqual(walk.record['stale_session_http'], 401)

    def test_authorized_stale_session_and_wrong_fresh_principal_fail(self):
        for status, principal in [(200, 'owner'), (401, 'different')]:
            walk, _ = self.walk(status, principal)
            with self.subTest(status=status, principal=principal), self.assertRaises(ValueError):
                walk.restart_login()
