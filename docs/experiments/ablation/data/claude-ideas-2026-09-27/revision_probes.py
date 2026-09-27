"""Conceptual probes for I7/I8/I9 revised contracts."""
import hashlib
import itertools
import json

def digest(data):
    return hashlib.sha256(data).hexdigest()

def emit(name, **evidence):
    print(json.dumps({"probe": name, **evidence}, sort_keys=True))

# I7: a complete receipt is indistinguishable from a correctly computed one
# to a verifier that trusts the builder and does not inspect row bytes.
row = b"rare"
summary = b"common"
commitment = digest(b"0|" + digest(row).encode() + b"|" + digest(summary).encode() + b"|v1")
receipt_authenticates = bool(commitment) and b"rare" not in summary
builder_should_reject = b"rare" not in summary
assert receipt_authenticates and builder_should_reject
emit("I7_semantic_bug_boundary", row=row.decode(), summary=summary.decode(),
     trusted_builder_would_reject=True,
     verifier_without_rows_cannot_detect_sealed_bug=True)

# I8: two sectors, one frame and one complete seal. A seal-only crash image is
# among the advertised arbitrary subsets and produces a full seal mismatch.
outcomes = {}
for frame, seal in itertools.product((False, True), repeat=2):
    if frame and seal:
        outcome = "accept"
    elif seal:
        outcome = "error_preserve"
    else:
        outcome = "truncate"
    outcomes[f"frame={int(frame)},seal={int(seal)}"] = outcome
assert outcomes["frame=0,seal=1"] == "error_preserve"
emit("I8_subset_contradiction", outcomes=outcomes,
     every_partial_subset_truncates=False)

# I8: a failed sync is process knowledge. Identical bytes are possible after
# success and error, so a file-only reopen cannot distinguish the histories.
visible_bytes = b"complete valid frame and seal"
histories = {"sync_ok": visible_bytes, "sync_eio": visible_bytes}
assert histories["sync_ok"] == histories["sync_eio"]
emit("I8_failed_sync_not_in_file", identical_reopened_bytes=True,
     external_error_witness_required=True)

# I9: a retry can return conflicting row values at the same coordinate.
coordinate = ("headS", 3, 2)
first = (coordinate, b"A")
retry = (coordinate, b"B")
assert first[0] == retry[0] and first[1] != retry[1]
emit("I9_same_coordinate_conflict", coordinate=coordinate,
     first_value="A", retry_value="B", set_union_ambiguous=True)
