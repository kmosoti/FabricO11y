import base64
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from delivery_oracle import MalformedTranscript, check

NID1 = "a0" * 16
NID2 = "b1" * 16
D1, D2, D3, D4 = b"payload-one", b"payload-two-", b"payload-three", b"payload-four"
ORACLE = Path(__file__).with_name("delivery_oracle.py")


def b64(data: bytes) -> str:
    return base64.b64encode(data).decode("ascii")


def source(node, gen, seq, data):
    return {"type": "source", "node_id": node, "generation": gen, "sequence": seq,
            "bytes": b64(data)}


def attempt(node, gen, seq, data, conflict=False):
    return {"type": "attempt", "node_id": node, "generation": gen, "sequence": seq,
            "bytes": b64(data), "injected_conflict": conflict}


def ack(node, gen, seq, through):
    return {"type": "response", "node_id": node, "generation": gen, "sequence": seq,
            "kind": "ack", "committed_through": through}


def resp(node, gen, seq, kind):
    return {"type": "response", "node_id": node, "generation": gen, "sequence": seq, "kind": kind}


def node_state(node, gen, ack_cursor, retained):
    return {"type": "node_state", "node_id": node, "generation": gen, "ack_cursor": ack_cursor,
            "retained_sequences": list(retained)}


def recovered(node, gen, seq, data):
    return {"type": "recovered", "node_id": node, "generation": gen, "sequence": seq,
            "bytes": b64(data)}


def fault(label):
    return {"type": "fault", "label": label}


def end():
    return {"type": "end"}


def lines(records):
    return [json.dumps(r) for r in records]


def rules(verdict):
    return {v["rule"] for v in verdict.violations}


def clean_transcript():
    """One node, one generation: 3 acked+recovered batches and a 4th sourced
    but not yet acked, correctly retained. Used directly and as the base for
    the mutation controls."""
    return [
        source(NID1, 1, 1, D1), source(NID1, 1, 2, D2), source(NID1, 1, 3, D3),
        attempt(NID1, 1, 1, D1), ack(NID1, 1, 1, 1),
        attempt(NID1, 1, 2, D2), ack(NID1, 1, 2, 2),
        attempt(NID1, 1, 3, D3), ack(NID1, 1, 3, 3),
        source(NID1, 1, 4, D4), attempt(NID1, 1, 4, D4),
        node_state(NID1, 1, 3, [4]),
        recovered(NID1, 1, 1, D1), recovered(NID1, 1, 2, D2), recovered(NID1, 1, 3, D3),
        end(),
    ]


def run_cli(text):
    from runner import scratch_root
    with tempfile.NamedTemporaryFile("w", dir=scratch_root(), suffix=".jsonl", delete=False) as handle:
        handle.write(text)
        path = handle.name
    try:
        result = subprocess.run([sys.executable, "-B", str(ORACLE), path],
                                 capture_output=True, text=True, timeout=30)
        return result.returncode, result.stdout
    finally:
        Path(path).unlink(missing_ok=True)


class CleanAndScenarioTests(unittest.TestCase):
    def test_clean_run_passes(self):
        verdict = check(lines(clean_transcript()))
        self.assertEqual(verdict.violations, [])
        self.assertTrue(verdict.passed)
        self.assertEqual(verdict.counts["streams"], 1)

    def test_lost_ack_retry_same_bytes(self):
        records = [
            source(NID1, 1, 1, D1),
            attempt(NID1, 1, 1, D1), ack(NID1, 1, 1, 1),
            attempt(NID1, 1, 1, D1), ack(NID1, 1, 1, 1),  # server never saw first ACK arrive
            recovered(NID1, 1, 1, D1), end(),
        ]
        verdict = check(lines(records))
        self.assertTrue(verdict.passed, verdict.violations)

    def test_equal_payload_new_sequence_kept_distinct(self):
        records = [
            source(NID1, 1, 1, D1), source(NID1, 1, 2, D1),  # identical bytes, distinct seq
            attempt(NID1, 1, 1, D1), ack(NID1, 1, 1, 1),
            attempt(NID1, 1, 2, D1), ack(NID1, 1, 2, 2),
            recovered(NID1, 1, 1, D1), recovered(NID1, 1, 2, D1), end(),
        ]
        verdict = check(lines(records))
        self.assertTrue(verdict.passed, verdict.violations)

    def test_generation_change_is_independent(self):
        records = [
            source(NID1, 1, 1, D1), attempt(NID1, 1, 1, D1), ack(NID1, 1, 1, 1),
            recovered(NID1, 1, 1, D1),
            source(NID1, 2, 1, D2), attempt(NID1, 2, 1, D2), ack(NID1, 2, 1, 1),
            recovered(NID1, 2, 1, D2),
            end(),
        ]
        verdict = check(lines(records))
        self.assertTrue(verdict.passed, verdict.violations)
        self.assertEqual(verdict.counts["streams"], 2)

    def test_identity_conflict_rejected_original_kept(self):
        records = [
            source(NID1, 1, 1, D1),
            attempt(NID1, 1, 1, D1, conflict=True), resp(NID1, 1, 1, "conflict"),
            # a later, honest retry with the true bytes succeeds
            attempt(NID1, 1, 1, D1), ack(NID1, 1, 1, 1),
            recovered(NID1, 1, 1, D1), end(),
        ]
        verdict = check(lines(records))
        self.assertTrue(verdict.passed, verdict.violations)

    def test_identity_conflict_wrongly_acked_fails(self):
        bad = b"not-the-source-bytes"
        records = [
            source(NID1, 1, 1, D1),
            attempt(NID1, 1, 1, bad, conflict=True), ack(NID1, 1, 1, 1),  # must not happen
            recovered(NID1, 1, 1, D1), end(),
        ]
        verdict = check(lines(records))
        self.assertFalse(verdict.passed)
        self.assertIn("CONFLICT-NOT-REPLACED", rules(verdict))

    def test_acked_record_missing_after_restart(self):
        records = [
            source(NID1, 1, 1, D1), attempt(NID1, 1, 1, D1), ack(NID1, 1, 1, 1),
            end(),  # no `recovered` record at all for an acked identity
        ]
        verdict = check(lines(records))
        self.assertFalse(verdict.passed)
        self.assertIn("ACKED-DURABLE", rules(verdict))

    def test_duplicate_logical_record_in_recovered(self):
        records = [
            source(NID1, 1, 1, D1), attempt(NID1, 1, 1, D1), ack(NID1, 1, 1, 1),
            recovered(NID1, 1, 1, D1), recovered(NID1, 1, 1, D1), end(),
        ]
        verdict = check(lines(records))
        self.assertFalse(verdict.passed)
        self.assertIn("NO-DUPLICATE", rules(verdict))

    def test_wrong_recovered_bytes(self):
        records = [
            source(NID1, 1, 1, D1), attempt(NID1, 1, 1, D1), ack(NID1, 1, 1, 1),
            recovered(NID1, 1, 1, D2),  # wrong payload for this identity
            end(),
        ]
        verdict = check(lines(records))
        self.assertFalse(verdict.passed)
        self.assertIn("NO-FABRICATION", rules(verdict))

    def test_unknown_extra_recovered_identity(self):
        records = [
            source(NID1, 1, 1, D1), attempt(NID1, 1, 1, D1), ack(NID1, 1, 1, 1),
            recovered(NID1, 1, 1, D1),
            recovered(NID2, 1, 1, D2),  # never sourced by any node
            end(),
        ]
        verdict = check(lines(records))
        self.assertFalse(verdict.passed)
        self.assertIn("NO-FABRICATION", rules(verdict))

    def test_same_payload_wrongly_collapsed(self):
        # Two distinct, both-acked identities sharing one payload; the server
        # keeps only one of them, "collapsing" by content instead of identity.
        records = [
            source(NID1, 1, 1, D1), source(NID1, 1, 2, D1),
            attempt(NID1, 1, 1, D1), ack(NID1, 1, 1, 1),
            attempt(NID1, 1, 2, D1), ack(NID1, 1, 2, 2),
            recovered(NID1, 1, 1, D1),  # sequence 2 silently missing
            end(),
        ]
        verdict = check(lines(records))
        self.assertFalse(verdict.passed)
        self.assertIn("ACKED-DURABLE", rules(verdict))

    def test_node_missing_unacked_batch(self):
        records = [
            source(NID1, 1, 1, D1), attempt(NID1, 1, 1, D1),  # never acked
            node_state(NID1, 1, 0, []),  # spool dropped it before any ack
            end(),
        ]
        verdict = check(lines(records))
        self.assertFalse(verdict.passed)
        self.assertIn("NODE-RETAINS-UNACKED", rules(verdict))

    def test_early_node_forget_cursor_overclaim(self):
        records = [
            source(NID1, 1, 1, D1), attempt(NID1, 1, 1, D1),  # no ack seen yet
            node_state(NID1, 1, 1, []),  # claims ack_cursor=1 the node never observed
            end(),
        ]
        verdict = check(lines(records))
        self.assertFalse(verdict.passed)
        self.assertIn("NODE-RETAINS-UNACKED", rules(verdict))

    def test_committed_through_decreasing(self):
        records = [
            source(NID1, 1, 1, D1), source(NID1, 1, 2, D2),
            attempt(NID1, 1, 1, D1), ack(NID1, 1, 1, 2),
            attempt(NID1, 1, 2, D2), ack(NID1, 1, 2, 1),  # decreased
            recovered(NID1, 1, 1, D1), recovered(NID1, 1, 2, D2), end(),
        ]
        verdict = check(lines(records))
        self.assertFalse(verdict.passed)
        self.assertIn("IN-ORDER-ACK", rules(verdict))

    def test_rejected_identity_not_committed(self):
        # unauthorized/bad_request/too_large without any covering ack: absence
        # is fine, and if recovered anyway the bytes must still be exact.
        ok = [
            source(NID1, 1, 1, D1), attempt(NID1, 1, 1, D1), resp(NID1, 1, 1, "unauthorized"),
            end(),
        ]
        self.assertTrue(check(lines(ok)).passed)

        present_but_correct = ok[:-1] + [recovered(NID1, 1, 1, D1), end()]
        self.assertTrue(check(lines(present_but_correct)).passed)

        present_but_wrong = ok[:-1] + [recovered(NID1, 1, 1, D2), end()]
        verdict = check(lines(present_but_wrong))
        self.assertFalse(verdict.passed)
        self.assertIn("NO-FABRICATION", rules(verdict))

    def test_exact_retry_mismatch_without_conflict_flag(self):
        records = [
            source(NID1, 1, 1, D1),
            attempt(NID1, 1, 1, D2),  # different bytes, not flagged as a conflict
            resp(NID1, 1, 1, "gap"),
            end(),
        ]
        verdict = check(lines(records))
        self.assertFalse(verdict.passed)
        self.assertIn("EXACT-RETRY", rules(verdict))

    def test_source_sequenced_after_snapshot_is_not_a_violation(self):
        # A node_state taken before a later batch is even sourced must not be
        # blamed for "forgetting" a batch that did not exist yet.
        records = [
            node_state(NID1, 1, 0, []),
            source(NID1, 1, 1, D1), attempt(NID1, 1, 1, D1),
            end(),
        ]
        verdict = check(lines(records))
        self.assertTrue(verdict.passed, verdict.violations)


class MutationControlTests(unittest.TestCase):
    """Proves the oracle can fail: each mutant of one clean, passing
    transcript must fail with its documented rule; the original must pass."""

    def setUp(self):
        self.clean = clean_transcript()
        self.baseline = check(lines(self.clean))
        self.assertTrue(self.baseline.passed, self.baseline.violations)

    def _index(self, predicate):
        for i, record in enumerate(self.clean):
            if predicate(record):
                return i
        raise AssertionError("no matching record")

    def test_mutant_drop_one_acked_recovered_record(self):
        i = self._index(lambda r: r["type"] == "recovered" and r["sequence"] == 2)
        mutant = self.clean[:i] + self.clean[i + 1:]
        verdict = check(lines(mutant))
        self.assertFalse(verdict.passed)
        self.assertIn("ACKED-DURABLE", rules(verdict))

    def test_mutant_duplicate_one_recovered_record(self):
        i = self._index(lambda r: r["type"] == "recovered" and r["sequence"] == 1)
        mutant = self.clean[:i + 1] + [self.clean[i]] + self.clean[i + 1:]
        verdict = check(lines(mutant))
        self.assertFalse(verdict.passed)
        self.assertIn("NO-DUPLICATE", rules(verdict))

    def test_mutant_replace_one_recovered_bytes(self):
        i = self._index(lambda r: r["type"] == "recovered" and r["sequence"] == 3)
        mutant = list(self.clean)
        mutant[i] = recovered(NID1, 1, 3, D4)
        verdict = check(lines(mutant))
        self.assertFalse(verdict.passed)
        self.assertIn("NO-FABRICATION", rules(verdict))

    def test_mutant_node_forgets_unacked_batch_early(self):
        i = self._index(lambda r: r["type"] == "node_state")
        mutant = list(self.clean)
        mutant[i] = node_state(NID1, 1, 3, [])  # sequence 4 dropped before any ack
        verdict = check(lines(mutant))
        self.assertFalse(verdict.passed)
        self.assertIn("NODE-RETAINS-UNACKED", rules(verdict))


class MalformedInputTests(unittest.TestCase):
    """The five explicitly required malformed classes, checked through the
    actual CLI subprocess, not just the library function."""

    def test_duplicate_json_keys_via_cli(self):
        text = '{"type": "end", "type": "end"}\n'
        code, _ = run_cli(text)
        self.assertEqual(code, 2)

    def test_nan_via_cli(self):
        text = json.dumps(source(NID1, 1, 1, D1))[:-1] + ', "extra": NaN}\n' + json.dumps(end())
        code, _ = run_cli(text + "\n")
        self.assertEqual(code, 2)

    def test_bad_base64_via_cli(self):
        rec = source(NID1, 1, 1, D1)
        rec["bytes"] = "not-valid-base64!!"
        code, _ = run_cli(json.dumps(rec) + "\n" + json.dumps(end()) + "\n")
        self.assertEqual(code, 2)

    def test_unknown_field_via_cli(self):
        rec = source(NID1, 1, 1, D1)
        rec["mystery"] = 1
        code, _ = run_cli(json.dumps(rec) + "\n" + json.dumps(end()) + "\n")
        self.assertEqual(code, 2)

    def test_missing_end_via_cli(self):
        rec = source(NID1, 1, 1, D1)
        code, _ = run_cli(json.dumps(rec) + "\n")
        self.assertEqual(code, 2)

    def test_clean_transcript_exits_zero_via_cli(self):
        text = "\n".join(lines(clean_transcript())) + "\n"
        code, stdout = run_cli(text)
        self.assertEqual(code, 0, stdout)
        self.assertTrue(json.loads(stdout)["passed"])

    def test_failing_transcript_exits_one_via_cli(self):
        records = [source(NID1, 1, 1, D1), attempt(NID1, 1, 1, D1), ack(NID1, 1, 1, 1), end()]
        code, stdout = run_cli("\n".join(lines(records)) + "\n")
        self.assertEqual(code, 1, stdout)
        self.assertFalse(json.loads(stdout)["passed"])


class MalformedInputLibraryTests(unittest.TestCase):
    """Additional malformed classes, checked directly against `check` for
    speed; each is still exercised by the CLI at least once above."""

    def assertMalformed(self, records, pattern):
        with self.assertRaisesRegex(MalformedTranscript, pattern):
            check(lines(records))

    def test_records_after_end(self):
        self.assertMalformed([end(), source(NID1, 1, 1, D1)], "after `end`")

    def test_unknown_record_type(self):
        self.assertMalformed([{"type": "bogus"}, end()], "unknown or missing record type")

    def test_missing_type_field(self):
        self.assertMalformed([{"node_id": NID1}, end()], "unknown or missing record type")

    def test_node_id_not_32_lowercase_hex(self):
        rec = source(NID1, 1, 1, D1)
        rec["node_id"] = NID1.upper()
        self.assertMalformed([rec, end()], "node_id must be 32 lowercase hex")

    def test_negative_sequence(self):
        rec = source(NID1, 1, 1, D1)
        rec["sequence"] = -1
        self.assertMalformed([rec, end()], "u64 range")

    def test_sequence_over_u64_max(self):
        rec = source(NID1, 1, 1, D1)
        rec["sequence"] = 2**64
        self.assertMalformed([rec, end()], "u64 range")

    def test_non_integer_generation(self):
        rec = source(NID1, 1, 1, D1)
        rec["generation"] = 1.5
        self.assertMalformed([rec, end()], "must be a JSON integer")

    def test_empty_bytes(self):
        rec = source(NID1, 1, 1, D1)
        rec["bytes"] = ""
        self.assertMalformed([rec, end()], "empty bytes")

    def test_oversize_line_rejected(self):
        huge = "x" * (2 * 1024 * 1024 + 1)
        with self.assertRaisesRegex(MalformedTranscript, "2 MiB"):
            check([huge, json.dumps(end())])

    def test_duplicate_source_identity(self):
        self.assertMalformed(
            [source(NID1, 1, 1, D1), source(NID1, 1, 1, D2), end()], "duplicate source identity")

    def test_committed_through_forbidden_on_non_ack(self):
        rec = resp(NID1, 1, 1, "gap")
        rec["committed_through"] = 1
        self.assertMalformed([rec, end()], "field mismatch")

    def test_retained_sequences_with_duplicate(self):
        rec = node_state(NID1, 1, 0, [1, 1])
        self.assertMalformed([rec, end()], "must not repeat")


if __name__ == "__main__":
    unittest.main()
