"""Persistent adapter integration; run after building fabric-adapter and Spindle.

python examples/agent_harness.py /run/user/UID/spindle/spindle.sock
Set FABRIC_ADAPTER to the installed adapter binary when it is not on PATH.
"""
import json
import os
import subprocess
import sys
import time


def command(process, request):
    process.stdin.write(json.dumps(request) + "\n")
    process.stdin.flush()
    response = json.loads(process.stdout.readline())
    if "error" in response:
        raise RuntimeError(response["error"])
    return response


def record(process, data):
    response = command(process, {"command": "record", "timestamp_ns": time.time_ns(), "data": data})
    if "accepted" not in response.get("admission", {}):
        raise RuntimeError(f"observation refused: {response}")


def generated_tool(process, run_id):
    started = time.monotonic_ns()
    result = 6 * 7
    record(process, {"kind": "tool_execution", "tool": "multiply", "run_id": run_id,
                     "duration_ns": time.monotonic_ns() - started, "status": "ok"})
    return result


if __name__ == "__main__":
    process = subprocess.Popen([os.environ.get("FABRIC_ADAPTER", "fabric-adapter"), sys.argv[1],
                                "python-agent-harness"], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, text=True)
    try:
        started = time.monotonic_ns()
        result = generated_tool(process, "python-example")
        record(process, {"kind": "agent_run", "agent": "python-example", "run_id": "python-example",
                         "duration_ns": time.monotonic_ns() - started, "status": "ok"})
        print({"result": result, "custody": command(process, {"command": "flush", "timeout_ms": 2000})})
        process.stdin.close()
        print(json.loads(process.stdout.readline()))  # Explicit EOF flush reply.
        process.wait(timeout=3)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
