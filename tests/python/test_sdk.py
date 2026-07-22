from __future__ import annotations

import gc
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from fabrico11y import AdmissionError, Engine


ROOT = Path(os.environ.get("FABRICO11Y_REPO_ROOT", Path(__file__).parents[2]))
OBSERVATION = json.loads(
    (ROOT / "fixtures/golden/contracts/valid/observation.json").read_text(encoding="utf-8")
)
CORRECTION = json.loads(
    (ROOT / "fixtures/golden/contracts/valid/correction.json").read_text(encoding="utf-8")
)


class EmbeddedSdkTests(unittest.TestCase):
    def test_clean_install_round_trip_and_rust_cli_parity(self) -> None:
        with tempfile.TemporaryDirectory() as data_root:
            engine = Engine(data_root)
            first = engine.admit(OBSERVATION)
            second = engine.admit(CORRECTION)
            self.assertEqual(first.record_id, "obs-0001")
            self.assertEqual(second.pending_count, 2)
            sealed = engine.seal()
            self.assertEqual(sealed.row_count, 2)
            python_state = engine.replay()
            self.assertEqual(engine.validate().archived_record_count, 2)
            location = engine.locate("obs-0001")
            self.assertIsNotNone(location)
            self.assertEqual(location.record_kind, "observation")
            del engine
            gc.collect()

            fabricctl = os.environ.get("FABRICCTL")
            if fabricctl:
                completed = subprocess.run(
                    [fabricctl, data_root, "replay"],
                    check=True,
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(json.loads(completed.stdout), python_state)

            reopened = Engine(data_root)
            self.assertEqual(reopened.replay(), python_state)
            self.assertEqual(reopened.recovery.registered_orphan_segments, 0)

    def test_typed_error_preserves_stable_category(self) -> None:
        invalid = dict(OBSERVATION)
        invalid["epistemic_class"] = "assumption"
        with tempfile.TemporaryDirectory() as data_root:
            engine = Engine(data_root)
            with self.assertRaises(AdmissionError) as caught:
                engine.admit(invalid)
            self.assertEqual(caught.exception.category, "schema_invalid")
            self.assertEqual(engine.pending_count, 0)


if __name__ == "__main__":
    unittest.main()
