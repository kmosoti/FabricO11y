"""Strict opt-in companion custody adapter; never derives sources from recovery.

After both production processes stop, spool_dump supplies committed sources and
the durable ACK cursor. Complete source history must still be present (including
ACKed frames); reclamation of any prefix fails closed. Append projected_sources
to the simulator ledger before ALL projected server recovery records and run the
unchanged delivery oracle. Do not append the companion node_state: its cursor is
not an observed wire response. validate_recovery separately checks that cursor's
custody claim; companion wire retry behavior remains unobserved.
"""

import argparse
import base64
import hashlib
import json
from pathlib import Path
import subprocess
import time

import delivery_oracle

MAX_DUMP_BYTES = 128 * 1024 * 1024


def project(record):
    result = dict(record)
    raw = base64.b64decode(result["bytes"], validate=True)
    result["bytes"] = base64.b64encode(hashlib.sha256(raw).digest()).decode()
    return result


def inspect_records(records):
    """Validate independent spool_dump output and return an attested source set."""
    if not records or records[-1].get("type") != "node_state":
        raise ValueError("missing final companion node_state")
    if any(r.get("type") != "source" for r in records[:-1]):
        raise ValueError("unexpected companion spool record")
    # Reuse the frozen oracle's strict schema validation, without changing it.
    delivery_oracle._parse_lines([json.dumps(r) + "\n" for r in records] + ['{"type":"end"}\n'])
    state = records[-1]
    stream = (state["node_id"], state["generation"])
    sources = records[:-1]
    if not sources or state["ack_cursor"] < 1:
        raise ValueError("companion has no acknowledged source history")
    if any((r["node_id"], r["generation"]) != stream for r in sources):
        raise ValueError("companion source identity differs from spool identity")
    sequences = [r["sequence"] for r in sources]
    if sequences != list(range(1, len(sources) + 1)):
        raise ValueError("companion source prefix missing, reordered or noncontiguous")
    if state["retained_sequences"] != sequences:
        raise ValueError("companion retained set differs from independent sources")
    if state["ack_cursor"] > len(sources):
        raise ValueError("companion ACK cursor exceeds complete source history")
    return {"stream": stream, "ack_cursor": state["ack_cursor"],
            "source_count": len(sources),
            "unacked_sequences": sequences[state["ack_cursor"]:],
            "projected_sources": [project(r) for r in sources]}


def validate_recovery(observation, recovered):
    """Check projected recovered bytes against sources and the real Spool ACK."""
    stream = tuple(observation["stream"])
    sources = {r["sequence"]: r["bytes"] for r in observation["projected_sources"]}
    seen = set()
    for record in recovered:
        if (record["node_id"], record["generation"]) != stream:
            continue
        sequence = record["sequence"]
        if sequence in seen:
            raise ValueError("duplicate companion recovered identity")
        seen.add(sequence)
        if sequence not in sources or record["bytes"] != sources[sequence]:
            raise ValueError("recovered companion batch lacks exact independent source")
    missing = set(range(1, observation["ack_cursor"] + 1)) - seen
    if missing:
        raise ValueError("acknowledged companion source absent from recovery")
    return {"passed": True, "acked_sources": observation["ack_cursor"],
            "recovered_sources": len(seen),
            "retained_unacked": len(observation["unacked_sequences"]),
            "wire_attempts_observed": False}


def dump_stopped_spool(binary, config, output, *, processes_stopped, timeout_s=60):
    """Bound a diagnostic child and preserve its real exit/stderr on failure."""
    if not processes_stopped:
        raise ValueError("stop server and dedicated Spindle before inspection")
    output = Path(output)
    output.mkdir(parents=True, exist_ok=False)
    stdout, stderr = output / "spool.jsonl", output / "spool.stderr"
    command = [str(binary), str(config)]
    began = time.monotonic()
    reason = None
    with stdout.open("wb") as out, stderr.open("wb") as err:
        child = subprocess.Popen(command, stdout=out, stderr=err)
        try:
            while child.poll() is None:
                if stdout.stat().st_size + stderr.stat().st_size > MAX_DUMP_BYTES:
                    reason = "output_limit"
                    break
                if time.monotonic() - began > timeout_s:
                    reason = "timeout"
                    break
                time.sleep(0.05)
        finally:
            if child.poll() is None:
                child.kill()
            code = child.wait(timeout=10)
    receipt = {"command": command, "exit": code, "stop_reason": reason,
               "elapsed_s": time.monotonic() - began,
               "stdout_bytes": stdout.stat().st_size, "stderr_bytes": stderr.stat().st_size}
    (output / "command.json").write_text(json.dumps(receipt, indent=2) + "\n")
    if code or reason or receipt["stdout_bytes"] + receipt["stderr_bytes"] > MAX_DUMP_BYTES:
        raise ValueError("companion spool inspection failed; preserve command receipt")
    # spool_dump currently reports recovery flags on stderr while exiting zero.
    if stderr.read_bytes():
        raise ValueError("companion spool inspector reported uncertainty on stderr")
    with stdout.open() as source:
        return inspect_records([json.loads(line) for line in source])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--spool-dump", type=Path, required=True)
    parser.add_argument("--node-config", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--processes-stopped", action="store_true", required=True)
    args = parser.parse_args()
    observation = dump_stopped_spool(args.spool_dump, args.node_config, args.out,
                                     processes_stopped=args.processes_stopped)
    (args.out / "sources-projected.jsonl").write_text(
        "".join(json.dumps(r) + "\n" for r in observation["projected_sources"]))
    (args.out / "observation.json").write_text(json.dumps(observation, indent=2) + "\n")


if __name__ == "__main__":
    main()
