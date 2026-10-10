"""Independent-source and custody rejection controls for the opt-in adapter."""
import base64
import copy
import json
import unittest

import delivery_oracle
from soak_companion import inspect_records, project, validate_recovery


def fixture():
    sources = [{"type": "source", "node_id": "07" * 16, "generation": 1,
                "sequence": n, "bytes": base64.b64encode(f"batch-{n}".encode()).decode()}
               for n in range(1, 4)]
    return sources + [{"type": "node_state", "node_id": "07" * 16,
                       "generation": 1, "ack_cursor": 2, "retained_sequences": [1, 2, 3]}]


def recovery(records):
    return [dict(project(r), type="recovered") for r in records[:-1]]


class CompanionCustody(unittest.TestCase):
    def test_complete_sources_and_retained_unacked(self):
        records = fixture()
        observation = inspect_records(records)
        self.assertEqual(observation["unacked_sequences"], [3])
        report = validate_recovery(observation, recovery(records)[:2])
        self.assertTrue(report["passed"])
        self.assertEqual(report["acked_sources"], 2)
        self.assertFalse(report["wire_attempts_observed"])

    def test_source_controls_fail_closed(self):
        records = fixture()
        cases = []
        cases.append(records[1:])  # reclaimed prefix
        cases.append([records[0], records[2], records[3]])  # internal gap
        cases.append([records[1], records[0], records[2], records[3]])
        bad = copy.deepcopy(records)
        bad[1]["generation"] = 2
        cases.append(bad)
        bad = copy.deepcopy(records)
        bad[-1]["retained_sequences"] = [1, 2]
        cases.append(bad)
        bad = copy.deepcopy(records)
        bad[-1]["ack_cursor"] = 4
        cases.append(bad)
        bad = copy.deepcopy(records)
        bad[-1]["ack_cursor"] = 0
        cases.append(bad)
        cases.append(records[:-1])
        for case in cases:
            with self.subTest(case=case), self.assertRaises(ValueError):
                inspect_records(case)

    def test_custody_controls_reject_missing_changed_duplicate_and_fabricated(self):
        records = fixture()
        observation = inspect_records(records)
        recovered = recovery(records)
        cases = [recovered[1:], recovered + [recovered[0]]]
        changed = copy.deepcopy(recovered)
        changed[0]["bytes"] = project(dict(records[0], bytes="d3Jvbmc="))["bytes"]
        cases.append(changed)
        fabricated = copy.deepcopy(recovered)
        fabricated[0]["sequence"] = 4
        cases.append(fabricated)
        for case in cases:
            with self.subTest(case=case), self.assertRaises(ValueError):
                validate_recovery(observation, case)

    def test_whole_ledger_oracle_keeps_companion_and_simulator(self):
        records = fixture()
        observation = inspect_records(records)
        simulator = dict(records[0], node_id="08" * 16)
        source = project(simulator)
        attempt = dict(source, type="attempt", injected_conflict=False)
        response = {"type": "response", "node_id": simulator["node_id"],
                    "generation": 1, "sequence": 1, "kind": "ack", "committed_through": 1}
        ledger = [source, attempt, response] + observation["projected_sources"]
        ledger += [dict(source, type="recovered")] + recovery(records)
        def verdict(items):
            return delivery_oracle.check([json.dumps(r) + "\n" for r in items]
                                         + ['{"type":"end"}\n'])
        self.assertTrue(verdict(ledger).passed)
        # Removing independently collected companion source leaves its recovered
        # telemetry visible to the unchanged oracle's NO-FABRICATION check.
        missing = [r for r in ledger if not (r["type"] == "source"
                   and r["node_id"] == "07" * 16 and r["sequence"] == 1)]
        self.assertFalse(verdict(missing).passed)
        changed = copy.deepcopy(ledger)
        changed[-1]["bytes"] = source["bytes"]
        self.assertFalse(verdict(changed).passed)


if __name__ == "__main__":
    unittest.main()
