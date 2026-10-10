import hashlib
import io
import json
import os
import tarfile
import tempfile
import unittest
from pathlib import Path

import preserve_soak_r2 as preserve


class PreserveSoakR2Tests(unittest.TestCase):
    def setUp(self):
        scratch = Path(os.environ["FABRIC_SCRATCH_ROOT"]).resolve(strict=True)
        self.temp = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def make_archive(self, name, payloads):
        archive_path = self.root / f"{name}.tar.gz"
        with tarfile.open(archive_path, "w:gz") as archive:
            root_info = tarfile.TarInfo(name + "/")
            root_info.type = tarfile.DIRTYPE
            root_info.mode = 0o700
            archive.addfile(root_info)
            for member, content in payloads.items():
                info = tarfile.TarInfo(f"{name}/{member}")
                info.size = len(content)
                info.mode = 0o600
                archive.addfile(info, io.BytesIO(content))
        return archive_path

    def rows(self, payloads):
        result = [{"path": ".", "type": "directory", "mode": 0o700,
                   "size": 0, "sha256": None}]
        result.extend(sorted(({"path": name, "type": "file", "mode": 0o600,
                               "size": len(content), "sha256": hashlib.sha256(content).hexdigest()}
                              for name, content in payloads.items()),
                             key=lambda row: row["path"]))
        return result

    def test_archive_verifier_accepts_exact_members_and_hashes(self):
        payloads = {"one.bin": b"frozen source", "nested/two.bin": b"full soak"}
        archive = self.make_archive("soak-r2-02", payloads)
        verified = preserve.verify_archive(archive, "soak-r2-02", self.rows(payloads))
        self.assertEqual(verified["member_count"], 3)

    def test_archive_verifier_rejects_missing_member(self):
        archive = self.make_archive("soak-r2-02", {"one.bin": b"one"})
        with self.assertRaisesRegex(ValueError, "missing, duplicated or extra"):
            preserve.verify_archive(archive, "soak-r2-02",
                                    self.rows({"one.bin": b"one", "two.bin": b"two"}))

    def test_archive_verifier_rejects_tampered_member(self):
        archive = self.make_archive("soak-r2-02", {"one.bin": b"changed"})
        with self.assertRaisesRegex(ValueError, "archive bytes differ"):
            preserve.verify_archive(archive, "soak-r2-02",
                                    self.rows({"one.bin": b"original"}))

    def test_cleanup_refuses_missing_or_changed_preservation_receipt(self):
        with self.assertRaisesRegex(ValueError, "preservation receipt"):
            preserve.validate_preservation({}, {}, "/owned/freeze", "archive-hash", "members-hash")

    def test_launcher_receipt_requires_confirmed_stop(self):
        launcher = {"unit": preserve.EXPECTED_LAUNCHER_UNIT, "stop_confirmed": False}
        child = {"state": "completed"}
        command = ["runner", "/run/alpha-soak-r2-full"]
        with self.assertRaisesRegex(ValueError, "does not prove"):
            preserve.validate_launcher_receipt(launcher, child, command)

    def test_postcheck_requires_exact_current_manifest(self):
        manifest = Path("/data/soak-r2-02/provenance/manifest.json")
        with self.assertRaisesRegex(ValueError, "exact current manifest"):
            preserve.validate_postcheck({"passed": True, "label": "full-post",
                                         "manifest": "/data/other/manifest.json",
                                         "manifest_sha256": "a" * 64}, manifest, "a" * 64)


if __name__ == "__main__":
    unittest.main()
