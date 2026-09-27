"""Small falsification probes for IDEAS.jsonl; no repository writes."""
import hashlib
import json


def h(value):
    return hashlib.sha256(value).hexdigest()


def emit(name, **evidence):
    print(json.dumps({"probe": name, **evidence}, sort_keys=True))


# I1: A perfectly fresh seal can authenticate a false-negative summary.
block = b"tenant=1 body=rare\n"
bad_summary = b"tokens={common}"
seal = h(b"genesis|block=0|rows=1|" + h(block).encode() + b"|" + h(bad_summary).encode())
assert b"rare" in block and b"rare" not in bad_summary and len(seal) == 64
emit("I1_sealed_false_negative", query="rare", true_matches=1,
     summary_rejects=True, fresh_seal_verifies=True)


# I2: A full, previously ACKed group with later bit rot must fail closed.
committed = b"event=ACKed"
stored = b"event=ACKeX"  # same length, changed byte after successful sync
seal_hash = h(committed)
assert len(committed) == len(stored) and h(stored) != seal_hash
proposed_action = "truncate whole group on hash mismatch"
emit("I2_corrupt_acked_group", acked=True, full_length_seal_present=True,
     hash_matches=False, proposed_action=proposed_action,
     acknowledged_event_survives=False)


# I3: Physical positions cease to be identities if a residual crosses snapshots.
first = {0: "A", 1: "B"}
after_compaction = {0: "B", 1: "C"}
initial_rows = [(0, first[0])]
residual_rows = [(1, after_compaction[1])]
merged = initial_rows + residual_rows
assert merged != sorted(first.items()) and merged != sorted(after_compaction.items())
emit("I3_cross_snapshot_residual", initial_snapshot=first,
     resumed_snapshot=after_compaction, merged=merged,
     equals_either_snapshot=False)


# I4: Reconciled ledgers can agree while the physical data are gone.
sender_ack_ledger = {(7, 42, "record-hash")}
owner_seal_ledger = sender_ack_ledger.copy()
actual_replayed_disk = set()
assert sender_ack_ledger == owner_seal_ledger and sender_ack_ledger != actual_replayed_disk
emit("I4_two_stale_ledgers", iblt_difference=[], actual_disk_missing=1,
     audit_reports_empty=True)


# I5: A single blocked sync caller cannot build a post-write summary in its wait.
events = ["write frames", "sync frames blocks caller", "sync returns",
          "build summary", "write commit with summary", "sync commit"]
assert events.index("build summary") > events.index("sync returns")
emit("I5_single_thread_sync_shadow", schedule=events,
     summary_overlaps_sync=False, sync_calls=2)


# I6: Exact per-piece dictionaries do not preserve whitespace token boundaries.
template_parts = ("time", "")
variable = "out"
body = template_parts[0] + variable + template_parts[1]
query = "timeout"
component_tokens = set(template_parts[0].split()) | set(variable.split())
assert query in body.split() and query not in component_tokens
emit("I6_cross_boundary_token", template="time{v}", variable=variable,
     query=query, reconstructed_match=True, dictionary_rejects=True)
