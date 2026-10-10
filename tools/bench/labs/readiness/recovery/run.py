#!/usr/bin/env python3
"""Root-dispatched, sequential exact existing tests; no production fault hooks."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys

GROUPS = {
    "spill": [("fabric-server", "lib", "segment::bounded::tests::" + name) for name in (
        "failed_build_removes_scratch_and_keeps_journal",
        "spill_preserves_all_float_bits_and_rejects_truncation")],
    "custody": [
        ("fabric_o11y", "lib", "spindle::spool::tests::reported_sync_errors_quarantine_and_record_known_failure"),
        ("fabric_o11y", "lib", "spindle::spool::tests::process_death_at_each_append_stage_reopens_to_a_verified_prefix"),
        ("fabric_o11y", "spindle", "source_failure_and_full_spool_leave_visible_gap_or_unknown_coverage"),
        ("fabric-server", "lib", "sealer::tests::real_checkpoint_failure_preserves_journal_and_retry_preserves_exact_bytes")],
    "history": [
        ("fabric-server", "history", "sealed_history_answers_exactly_and_pages_are_stable"),
        ("fabric-server", "history", "crash_states_of_sealing_never_serve_a_record_twice"),
        ("fabric-server", "history", "journal_and_segment_representations_answer_identically"),
        ("fabric-server", "delivery", "node_delivers_exact_bytes_and_both_sides_survive_restart")],
}


def selected_test_passed(output):
    return bool(re.search(r"test result: ok\. 1 passed; 0 failed; 0 ignored;", output))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--group", choices=GROUPS, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    # Selection negative controls: zero tests and ignored tests must be rejected.
    assert not selected_test_passed("test result: ok. 0 passed; 0 failed; 0 ignored;")
    assert not selected_test_passed("test result: ok. 0 passed; 0 failed; 1 ignored;")
    assert selected_test_passed("test result: ok. 1 passed; 0 failed; 0 ignored;")
    scratch = Path(os.environ["FABRIC_LAB_SCRATCH"]).resolve(strict=True)
    temporary = Path(os.environ["TMPDIR"]).resolve(strict=True)
    storage = Path("/run/media/kmosoti/data/FabricO11y").resolve(strict=True)
    if not scratch.is_relative_to(storage) or not temporary.is_relative_to(storage):
        raise SystemExit("scratch and TMPDIR must remain on the mounted data drive")
    args.out.mkdir(parents=True, exist_ok=True)
    results = []
    for index, (package, target, name) in enumerate(GROUPS[args.group]):
        command = ["cargo", "test", "--offline", "--locked", "-p", package]
        command += ["--lib"] if target == "lib" else ["--test", target]
        command += [name, "--", "--exact", "--test-threads=1"]
        output = args.out / f"{index:02d}.txt"
        with output.open("w") as stream:
            child = subprocess.run(command, stdout=stream, stderr=subprocess.STDOUT)
        text = output.read_text()
        selected = selected_test_passed(text)
        results.append({"command": command, "exit": child.returncode,
                        "one_selected_test_passed": selected, "output": str(output)})
        (args.out / "result.json").write_text(json.dumps({"group": args.group,
            "scratch": str(scratch), "tmpdir": str(temporary),
            "selection_negative_controls": "passed", "tests": results}, indent=2) + "\n")
        print(json.dumps(results[-1]), flush=True)
        if child.returncode or not selected:
            return child.returncode or 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
