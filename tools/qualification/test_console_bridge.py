"""Campaign bridge boundary controls; browser/WebAuthn smoke is separate."""
import os
from pathlib import Path
import tempfile
import unittest

from console_bridge import protected_secret, validate_route


class BridgeBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=os.environ["FABRIC_SCRATCH_ROOT"])
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def test_production_route_only_and_canonical_encoding(self):
        validate_route("/v1/console/query", "POST")
        validate_route("/v1/console/nodes/source-1/config", "PUT")
        for route in ("/v1/admin/query", "https://localhost/v1/console/query",
                      "/v1/console/../query", "/v1/console/%2e%2e/query",
                      "/v1/console//query", "/v1/console/query?token=synthetic"):
            with self.assertRaises(ValueError):
                validate_route(route, "POST")

    def test_bootstrap_requires_protected_regular_owned_scratch_file(self):
        secret = self.root / "bootstrap"
        secret.write_text("synthetic-once-only-fixture\n")
        secret.chmod(0o600)
        self.assertEqual(protected_secret(secret), "synthetic-once-only-fixture")
        secret.chmod(0o644)
        with self.assertRaises(ValueError):
            protected_secret(secret)
        secret.chmod(0o600)
        link = self.root / "link"
        link.symlink_to(secret)
        with self.assertRaises(ValueError):
            protected_secret(link)
        hard = self.root / "hard"
        os.link(secret, hard)
        with self.assertRaises(ValueError):
            protected_secret(secret)

    def test_oversized_secret_is_not_consumed(self):
        secret = self.root / "oversized"
        secret.write_text("x" * 257)
        secret.chmod(0o600)
        with self.assertRaises(ValueError):
            protected_secret(secret)


if __name__ == "__main__":
    unittest.main()
