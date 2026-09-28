"""Positive and negative controls for trace_check.py.

The fixture is the transcript of a real `server-kill` fault run (seed 1,
three Spindles, three server SIGKILLs) produced by
tools/qualification/delivery_faults.py on the architecture-foundation head,
with Batch bytes replaced by "-" because the model has no bytes. Each
negative control changes one fact and must make TLC reject the trace.

Run: TLA_JAR=/path/tla2tools.jar python3 -B formal/delivery/test_trace_check.py
"""

import json
import os
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import trace_check  # noqa: E402

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "server-kill-seed1.jsonl"
JAR = os.environ.get("TLA_JAR", "")


def records():
    return [json.loads(line) for line in FIXTURE.read_text().splitlines() if line.strip()]


def lines(rs):
    return [json.dumps(r) for r in rs]


def last_ack(rs, node_id):
    return max(i for i, r in enumerate(rs)
               if r["type"] == "response" and r["kind"] == "ack" and r["node_id"] == node_id)


class TraceCheck(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not Path(JAR).is_file():
            raise RuntimeError("TLA_JAR must name tla2tools.jar; this check cannot run without it")

    def verdict(self, rs):
        return trace_check.check(lines(rs), JAR)[0]

    def test_real_server_kill_transcript_is_accepted(self):
        self.assertEqual(self.verdict(records()), "accepted")

    def test_early_forget_of_an_unacknowledged_batch_is_rejected(self):
        rs = records()
        state = next(r for r in rs if r["type"] == "node_state")
        top = max(state["retained_sequences"])
        # The last ACK never arrived, yet the Spindle stopped retaining the batch.
        rs[last_ack(rs, state["node_id"])]["committed_through"] = top - 1
        state["retained_sequences"].remove(top)
        self.assertEqual(self.verdict(rs), "rejected")

    def test_forgetting_an_acknowledged_batch_is_accepted(self):
        rs = records()
        state = next(r for r in rs if r["type"] == "node_state")
        state["retained_sequences"].remove(min(state["retained_sequences"]))
        self.assertEqual(self.verdict(rs), "accepted")

    def test_acknowledged_batch_missing_from_recovery_is_rejected(self):
        rs = records()
        i = next(i for i, r in enumerate(rs) if r["type"] == "recovered")
        del rs[i]
        self.assertEqual(self.verdict(rs), "rejected")

    def test_resending_after_forget_is_rejected(self):
        rs = records()
        state = next(r for r in rs if r["type"] == "node_state")
        low = min(state["retained_sequences"])
        state["retained_sequences"].remove(low)
        at = rs.index(state) + 1
        sent = {"type": "attempt", "node_id": state["node_id"], "generation": state["generation"],
                "sequence": low, "bytes": "-", "injected_conflict": False}
        rs.insert(at, sent)
        self.assertEqual(self.verdict(rs), "rejected")


if __name__ == "__main__":
    unittest.main()
