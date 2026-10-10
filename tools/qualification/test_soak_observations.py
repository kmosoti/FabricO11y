import json
import tempfile
import unittest
from pathlib import Path

import soak_observations


class SupplementalSoakObservationsTests(unittest.TestCase):
    def write_inputs(self, root, events):
        event_path = root / "events.jsonl"
        event_path.write_text("".join(json.dumps(event) + "\n" for event in events))
        sim_path = root / "sim-summary.json"
        sim_path.write_text(json.dumps({"began_unix_ns": 0, "seconds": 5460,
                                        "identities": 100, "seed": 17}))
        soak_path = root / "soak-summary.json"
        soak_path.write_text(json.dumps({"windows": [], "batches_created": 1, "batches_acked": 1,
                                         "smoke": False, "protocol_revision": "R2",
                                         "seconds": 5460, "identities": 100, "seed": 17,
                                         "gates": {"oracle": True}}))
        return event_path, sim_path, soak_path

    def test_retry_delay_and_attempt_rates_are_separate(self):
        base = 61_000_000_000
        events = [
            {"e": "created", "id": 3, "seq": 9, "t": base, "generated_ns": base},
            {"e": "applied", "id": 3, "rev": 2, "t": base + 50_000_000},
            {"e": "attempt", "id": 3, "seq": 9, "created": base,
             "start": base + 100_000_000, "end": base + 200_000_000, "kind": "unavailable"},
            {"e": "attempt", "id": 3, "seq": 9, "created": base,
             "start": base + 900_000_000, "end": base + 1_100_000_000, "kind": "ack"},
        ]
        with tempfile.TemporaryDirectory() as directory:
            paths = self.write_inputs(Path(directory), events)
            report = soak_observations.analyze(*paths)
        window = report["retry_aware_windows"][0]
        self.assertEqual(window["attempt_count"], 2)
        self.assertEqual(window["ack_kind_attempt_rate"], 0.5)
        self.assertEqual(window["http_503_unavailable_kind_attempt_rate"], 0.5)
        self.assertEqual(window["created_to_first_ack_ms_created_t_to_first_ack_end_p50"], 1100)
        self.assertEqual(window["attempt_latency_ms_end_minus_start_p50"], 100)
        self.assertEqual(window["created_batches_with_ack_kind_by_event_log_end"], 1)
        self.assertFalse(report["registered_oracle_or_gates_modified"])

    def test_rejects_attempt_with_inconsistent_creation_timestamp(self):
        base = 61_000_000_000
        events = [
            {"e": "created", "id": 1, "seq": 1, "t": base, "generated_ns": base},
            {"e": "attempt", "id": 1, "seq": 1, "created": base + 1,
             "start": base, "end": base + 1, "kind": "ack"},
        ]
        with tempfile.TemporaryDirectory() as directory:
            paths = self.write_inputs(Path(directory), events)
            with self.assertRaisesRegex(ValueError, "created timestamp mismatch"):
                soak_observations.analyze(*paths)

    def test_rejects_duplicate_ack_identity(self):
        base = 61_000_000_000
        attempt = {"e": "attempt", "id": 1, "seq": 1, "created": base,
                   "start": base, "end": base + 1, "kind": "ack"}
        events = [
            {"e": "created", "id": 1, "seq": 1, "t": base, "generated_ns": base},
            attempt,
            attempt,
        ]
        with tempfile.TemporaryDirectory() as directory:
            paths = self.write_inputs(Path(directory), events)
            with self.assertRaisesRegex(ValueError, "duplicate ACK identity"):
                soak_observations.analyze(*paths)

    def test_rejects_short_smoke_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = self.write_inputs(root, [])
            summary = json.loads(paths[2].read_text())
            summary["smoke"] = True
            summary["seconds"] = 70
            paths[2].write_text(json.dumps(summary))
            with self.assertRaisesRegex(ValueError, "full R2 trial"):
                soak_observations.analyze(*paths)

    def test_rejects_unknown_event_type(self):
        with tempfile.TemporaryDirectory() as directory:
            paths = self.write_inputs(Path(directory), [{"e": "mystery"}])
            with self.assertRaisesRegex(ValueError, "unknown event type"):
                soak_observations.analyze(*paths)

    def test_rejects_attempt_before_creation(self):
        base = 61_000_000_000
        events = [
            {"e": "created", "id": 1, "seq": 1, "t": base, "generated_ns": base},
            {"e": "attempt", "id": 1, "seq": 1, "created": base,
             "start": base - 1, "end": base + 1, "kind": "ack"},
        ]
        with tempfile.TemporaryDirectory() as directory:
            paths = self.write_inputs(Path(directory), events)
            with self.assertRaisesRegex(ValueError, "starts before creation"):
                soak_observations.analyze(*paths)

    def test_rejects_event_counts_that_disagree_with_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            paths = self.write_inputs(Path(directory), [])
            with self.assertRaisesRegex(ValueError, "created count disagrees"):
                soak_observations.analyze(*paths)

    def test_rejects_nonmonotonic_ack_sequence(self):
        base = 61_000_000_000
        events = [
            {"e": "created", "id": 1, "seq": 1, "t": base, "generated_ns": base},
            {"e": "created", "id": 1, "seq": 2, "t": base + 1, "generated_ns": base + 1},
            {"e": "attempt", "id": 1, "seq": 2, "created": base + 1,
             "start": base + 2, "end": base + 3, "kind": "ack"},
            {"e": "attempt", "id": 1, "seq": 1, "created": base,
             "start": base + 4, "end": base + 5, "kind": "ack"},
        ]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = self.write_inputs(root, events)
            summary = json.loads(paths[2].read_text())
            summary["batches_created"] = 2
            summary["batches_acked"] = 2
            paths[2].write_text(json.dumps(summary))
            with self.assertRaisesRegex(ValueError, "nonmonotonic ACK sequence"):
                soak_observations.analyze(*paths)


if __name__ == "__main__":
    unittest.main()
