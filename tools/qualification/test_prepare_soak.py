"""Source census controls: vendor/UI/package bytes must survive a freeze."""
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import prepare_soak


class SourceCensusTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=os.environ["FABRIC_SCRATCH_ROOT"])
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        # Independent representative inputs for real patched dependencies,
        # browser packaging and production configuration examples.
        self.required = {
            "vendor/reactive_graph/src/owner/arena.rs",
            "vendor/reactive_graph/Cargo.toml", "vendor/provenance.json",
            "crates/fabric-ui/bootstrap.js", "crates/fabric-ui/style.css",
            "crates/fabric-ui/index.html", "crates/fabric-ui/manifest.webmanifest",
            "crates/fabric-ui/icon.svg", "crates/fabric-ui/src/live.rs",
            "packaging/build-rpm.sh", "packaging/etc/access.conf.example",
            "packaging/systemd/fabrico11y-server.service",
            "packaging/licenses/webauthn-rs-0.5.5/LICENSE.md",
            "tools/packaging/stage_candidate.py", "tools/ui/build.py",
            ".cargo/config.toml", "Cargo.lock", "src/bin/fabricctl.rs",
        }
        self.excluded = {"operator-token", "local-server.conf", ".env",
                         "docs/experiments/formal/data/run/access.json"}
        self.deleted = "vendor/reactive_graph/src/deleted.rs"
        for name in self.required | self.excluded:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("synthetic input\n")
        census = sorted(self.required | self.excluded | {self.deleted})
        self.git = patch.object(prepare_soak, "git", return_value=("\0".join(census) + "\0").encode())
        self.git.start()
        self.addCleanup(self.git.stop)
        self.location = patch.object(prepare_soak, "ROOT", self.root)
        self.location.start()
        self.addCleanup(self.location.stop)

    def assert_coverage(self, selected):
        self.assertFalse(self.required - set(selected), "required build inputs omitted")

    def test_current_compilation_and_package_inputs_are_censused(self):
        self.assert_coverage(prepare_soak.source_paths())
        self.assertFalse(self.excluded & set(prepare_soak.source_paths()))

    def test_deleted_vendor_input_remains_in_diff_scope(self):
        self.assertNotIn(self.deleted, prepare_soak.source_paths())
        self.assertIn(self.deleted, prepare_soak.source_paths(include_missing=True))

    def test_representative_vendor_proto_only_mutant_is_rejected(self):
        selected = prepare_soak.source_paths()
        mutant = [name for name in selected if not name.startswith("vendor/")
                  or name.endswith(".proto")]
        with self.assertRaisesRegex(AssertionError, "required build inputs omitted"):
            self.assert_coverage(mutant)

    def test_unsafe_git_name_is_rejected(self):
        for name in ("../vendor/escape.rs", "/vendor/absolute.rs"):
            with patch.object(prepare_soak, "git", return_value=(name + "\0").encode()):
                with self.assertRaisesRegex(ValueError, "unsafe source path"):
                    prepare_soak.source_paths()


if __name__ == "__main__":
    unittest.main()
