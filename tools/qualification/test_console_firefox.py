"""Finite backend boundary controls; real TLS refusals are separate native cells."""
from pathlib import Path
import unittest
from unittest.mock import Mock, patch

from console_bridge import ConsoleBridge, bridge_origin, firefox_capabilities


class FirefoxBridgeTests(unittest.TestCase):
    def bridge(self, backend):
        value = ConsoleBridge.__new__(ConsoleBridge)
        value.backend, value.authenticators, value.receipt = backend, [], {}
        value._cdp, value._gecko_auth = Mock(), Mock()
        return value

    def test_existing_chrome_origin_stays_localhost(self):
        self.assertEqual(bridge_origin('https://localhost:8443', 'chrome', None).hostname, 'localhost')
        with self.assertRaises(ValueError): bridge_origin('https://guide.example:8443', 'chrome', None)

    def test_firefox_exact_private_dns_origin(self):
        self.assertEqual(bridge_origin('https://guide.example:8443', 'firefox', 'guide.example').hostname, 'guide.example')

    def test_noncanonical_or_unconfined_origins_refuse(self):
        for origin, name in (('http://guide.example:8443', 'guide.example'), ('https://guide.example:8443/path', 'guide.example'),
                             ('https://user@guide.example:8443', 'guide.example'), ('https://guide.example:8443', 'other.example'),
                             ('https://127.0.0.1:8443', '127.0.0.1'), ('https://guide.example:8443', '*.example')):
            with self.subTest(origin=origin, name=name), self.assertRaises(ValueError): bridge_origin(origin, 'firefox', name)

    def test_strict_capabilities_and_profile_only_dns(self):
        value = firefox_capabilities(Path('/owned/profile'), 'guide.example')
        self.assertIs(value['acceptInsecureCerts'], False)
        options = value['moz:firefoxOptions']; prefs = options['prefs']
        self.assertEqual(options['args'], ['-headless', '-profile', '/owned/profile'])
        self.assertIs(prefs['security.enterprise_roots.enabled'], False)
        self.assertEqual(prefs['network.dns.localDomains'], 'guide.example')
        self.assertEqual((prefs['network.proxy.type'], prefs['network.trr.mode']), (0, 5))
        self.assertNotIn('goog:chromeOptions', value)

    def test_chrome_virtual_options_preserved(self):
        value = self.bridge('chrome'); value._cdp.return_value = {'authenticatorId': 'first'}
        self.assertEqual(value.add_authenticator(), 'first')
        options = value._cdp.call_args.args[1]['options']
        self.assertEqual((options['protocol'], options['transport']), ('ctap2', 'internal'))
        self.assertTrue(options['hasResidentKey'] and options['hasUserVerification'] and options['isUserVerified'])

    def test_firefox_camel_case_ctap21_virtual_device(self):
        value = self.bridge('firefox'); value._gecko_auth.return_value = 'fresh'
        self.assertEqual(value.add_authenticator(), 'fresh')
        options = value._gecko_auth.call_args.args[1]
        self.assertEqual((options['protocol'], options['transport']), ('ctap2_1', 'usb'))
        self.assertTrue(options['isUserConsenting'] and options['isUserVerified'])
        value._cdp.assert_not_called()

    def test_foreign_virtual_device_is_not_removed(self):
        value = self.bridge('firefox')
        with self.assertRaises(ValueError): value.remove_authenticator('foreign')
        value._gecko_auth.assert_not_called()

    def test_replacement_removes_old_devices_before_fresh_add(self):
        value = self.bridge('firefox'); value.authenticators = ['old1', 'old2']
        events = []
        def remove(identifier): events.append(('remove', identifier)); value.authenticators.remove(identifier)
        value.remove_authenticator = remove
        value.add_authenticator = lambda: events.append(('add', 'fresh')) or 'fresh'
        self.assertEqual(value.replace_authenticator(), 'fresh')
        self.assertEqual(events, [('remove', 'old1'), ('remove', 'old2'), ('add', 'fresh')])

    def test_firefox_inventory_cannot_return_private_keys(self):
        value = self.bridge('firefox'); value.authenticators = ['owned']
        value._gecko_auth.return_value = [{'resident': True, 'rp': 'guide.example', 'privateKey': 'must-not-escape'}]
        self.assertEqual(value.resident_credentials('owned'), {'count': 1, 'credentials': [{'resident': True, 'rp': 'guide.example'}], 'uv_configured': True})

    def test_chrome_inventory_projects_public_fields(self):
        value = self.bridge('chrome'); value.authenticators = ['owned']
        value._cdp.return_value = {'credentials': [{'isResidentCredential': True, 'rpId': 'localhost', 'privateKey': 'must-not-escape'}]}
        self.assertEqual(value.resident_credentials('owned')['credentials'], [{'resident': True, 'rp': 'localhost'}])

    def test_gecko_privileged_context_restored_on_failure(self):
        value = ConsoleBridge.__new__(ConsoleBridge); value._wd = Mock(); value._js = Mock(side_effect=RuntimeError('controlled'))
        with self.assertRaises(RuntimeError): value._gecko_auth('throw 1')
        self.assertEqual([call.args for call in value._wd.call_args_list], [('/moz/context', {'context': 'chrome'}), ('/moz/context', {'context': 'content'})])

    def test_firefox_missing_supervision_refuses_before_effects(self):
        with patch('console_bridge.require_limits') as limits:
            with self.assertRaises(ValueError): ConsoleBridge('https://guide.example:8443', '/owned/leaf', '/owned/work', '/owned/out', backend='firefox', trusted_ca='/owned/ca', private_dns='guide.example')
            limits.assert_not_called()

class FirefoxToolPinsTests(unittest.TestCase):
    def test_actual_inventory_content_and_link_controls(self):
        import tempfile, hashlib, os
        from pathlib import Path
        from console_firefox_trust import verify_tool_pins
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);binary=root/'firefox';binary.write_bytes(b'original')
            alias=root/'alias';alias.symlink_to('firefox')
            pins=dict(firefox_root=str(root),firefox_files=sorted([str(binary),str(alias)]),
                      links={str(alias):'firefox'},tools={str(binary):hashlib.sha256(b'original').hexdigest()})
            verify_tool_pins(pins)
            binary.write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError,'tool identity'):verify_tool_pins(pins)
            binary.write_bytes(b'original');alias.unlink();alias.symlink_to('./firefox')
            with self.assertRaisesRegex(ValueError,'link changed'):verify_tool_pins(pins)
            alias.unlink();alias.symlink_to('firefox');(root/'new').write_bytes(b'new')
            with self.assertRaisesRegex(ValueError,'inventory changed'):verify_tool_pins(pins)

