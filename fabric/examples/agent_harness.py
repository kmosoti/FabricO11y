"""Carry one global context through a persistent Python JSONL adapter.

Usage: python3 fabric/examples/agent_harness.py SOCKET [PATH_TO_FABRIC_ADAPTER] [GLOBAL_RUN_UUID]
Pass the JSON context unchanged to child/generated processes with --run-context.
"""
import json
import subprocess
import sys
import time
import uuid

context = {"run_id": sys.argv[3] if len(sys.argv) > 3 else str(uuid.uuid4())}
with subprocess.Popen(
    [sys.argv[2] if len(sys.argv) > 2 else "fabric-adapter", sys.argv[1],
     "python-harness", "--run-context", json.dumps(context)],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
) as adapter:
    def exchange(command):
        adapter.stdin.write(json.dumps(command) + "\n")
        adapter.stdin.flush()
        line = adapter.stdout.readline()
        if not line:
            raise RuntimeError("adapter closed without a reply")
        reply = json.loads(line)
        if "error" in reply:
            raise RuntimeError(reply["error"])
        return reply

    command = {"command": "record", "timestamp_ns": time.time_ns(),
               "data": {"kind": "agent_run", "agent": "python-harness",
                        "run_id": context["run_id"], "duration_ns": 1, "status": "ok"}}
    deadline = time.monotonic() + 2
    while True:
        if time.monotonic() >= deadline:
            raise RuntimeError("observation admission deadline")
        reply = exchange(command)
        admission = reply.get("admission")
        if isinstance(admission, dict) and "accepted" in admission:
            break
        if admission not in ("contended", "full"):
            raise RuntimeError(f"admission refused: {reply}")
        time.sleep(min(0.001, max(0, deadline - time.monotonic())))
    print(json.dumps({"context": context,
                      "local": exchange({"command": "flush", "timeout_ms": 5000}),
                      "fabric": exchange({"command": "flush_to_fabric", "timeout_ms": 30000})}))
    adapter.stdin.close()
    for line in adapter.stdout:
        reply = json.loads(line)
        if "error" in reply:
            raise RuntimeError(reply["error"])
    if adapter.wait() != 0:
        raise RuntimeError("adapter failed")
