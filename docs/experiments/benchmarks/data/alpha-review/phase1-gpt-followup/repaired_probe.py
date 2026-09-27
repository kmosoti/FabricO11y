#!/usr/bin/env python3
"""Frozen-binary, small phase-1 counterexamples; writes only under this scratch root."""

import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parents[1]
NODE = REPO / "target/release/fabric-node"
CTL = REPO / "target/release/fabricctl"
DUMP = REPO / "target/release/examples/alpha_native_dump"


def config(case: str, logs: list[Path], interval: int = 15) -> tuple[Path, Path]:
    home = ROOT / case
    home.mkdir(exist_ok=True)
    spool = home / "spool"
    path = home / "node.conf"
    path.write_text(
        f"spool_dir={spool}\nmetric_interval_s={interval}\nspool_bytes=1048576\n"
        + "".join(f"log={log}\n" for log in logs)
    )
    return path, spool


def call(*parts: str) -> subprocess.CompletedProcess:
    return subprocess.run([str(part) for part in parts], capture_output=True,
                          text=True, timeout=5)


def inspect(config_path: Path) -> tuple[int, dict[str, str], str]:
    done = call(CTL, "inspect", config_path)
    fields = dict(part.split("=", 1) for part in done.stdout.split() if "=" in part)
    return done.returncode, fields, done.stderr.strip()


def reuse() -> dict:
    home = ROOT / "reuse-repaired"
    home.mkdir(exist_ok=True)
    source = home / "app.log"
    source.write_text("".join(f"old-{n:02d}\n" for n in range(10)))
    first_inode = source.stat().st_ino
    cfg, _ = config("reuse-repaired", [source])
    first = call(NODE, "collect", cfg)
    if first.returncode:
        raise RuntimeError(first.stderr)
    same_inode = False
    attempts = 0
    for attempts in range(1, 1001):
        source.unlink()
        source.write_text("".join(f"new-{n:02d}\n" for n in range(20)))
        if source.stat().st_ino == first_inode:
            same_inode = True
            break
    if not same_inode:
        return {"available": False, "first_inode": first_inode,
                "last_inode": source.stat().st_ino, "attempts": attempts}
    second = call(NODE, "collect", cfg)
    status_exit, status, status_error = inspect(cfg)
    replay = subprocess.run([str(DUMP), str(cfg)], capture_output=True, timeout=5)
    bodies = replay.stdout.decode().splitlines() if replay.returncode == 0 else []
    missing = [f"new-{n:02d}" for n in range(20) if f"new-{n:02d}" not in bodies]
    failed = second.returncode == 0 and status_exit == 0 and replay.returncode == 0 \
        and bool(missing) and status.get("gaps") == "0"
    return {"available": True, "attempts": attempts, "inode": first_inode,
            "first_exit": first.returncode, "first_output": first.stdout.strip(),
            "second_exit": second.returncode, "second_output": second.stdout.strip(),
            "inspect_exit": status_exit, "inspect": status,
            "inspect_error": status_error, "dump_exit": replay.returncode,
            "replayed_bodies": bodies, "missing_new": missing,
            "silent_loss": failed}


def gap_files(label: str, count: int) -> dict:
    home = ROOT / label
    home.mkdir(exist_ok=True)
    logs = []
    for i in range(count):
        source = home / f"source{i}.log"
        source.write_bytes(b"\xff\n" * 8 + b"good\n")
        logs.append(source)
    cfg, spool = config(label, logs)
    first = call(NODE, "collect", cfg)
    first_inspect = inspect(cfg)
    second = call(NODE, "collect", cfg)
    marker = spool / "coverage-unknown"
    return {"files": count, "first_exit": first.returncode,
            "first_output": first.stdout.strip(), "first_error": first.stderr.strip(),
            "first_inspect": first_inspect, "second_exit": second.returncode,
            "second_output": second.stdout.strip(), "second_error": second.stderr.strip(),
            "coverage_unknown_marker": marker.exists()}


def gaps() -> dict:
    control = gap_files("gap-control-2-repaired", 2)
    failure = gap_files("gap-3-repaired", 3)
    failed = (control["first_exit"] == 0 and control["second_exit"] == 0
              and failure["first_exit"] != 0 and failure["second_exit"] != 0
              and failure["coverage_unknown_marker"])
    return {"control": control, "failure": failure, "permanent_halt": failed}


def concurrent_inspect() -> dict:
    label = "inspect-repaired-" + uuid.uuid4().hex[:8]
    home = ROOT / label
    cfg, spool = config(label, [], 1)
    output = (home / "node-output.log").open("wb")
    child = subprocess.Popen([str(NODE), "run", str(cfg)], stdout=output,
                             stderr=subprocess.STDOUT)
    calls = 0
    false_status = []
    errors = 0
    clean_after_false = None
    try:
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline and child.poll() is None:
            status_exit, status, error = inspect(cfg)
            calls += 1
            if status_exit == 0 and status.get("recovery_required") == "true":
                false_status.append(status)
            if (false_status and status_exit == 0
                    and status.get("recovery_required") == "false"
                    and int(status.get("batches", "0")) >= 1):
                clean_after_false = status
                break
            if status_exit != 0:
                errors += 1
    finally:
        if child.poll() is None:
            child.send_signal(signal.SIGTERM)
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
        output.close()
    settled_exit, settled, settled_error = inspect(cfg)
    return {"calls": calls, "false_status_count": len(false_status),
            "first_false_status": false_status[:1], "retryable_errors": errors,
            "clean_after_false_while_writer_alive": clean_after_false,
            "node_exit": child.returncode, "marker_after_stop":
            (spool / "recovery-required").exists(),
            "settled_inspect_exit": settled_exit, "settled_inspect": settled,
            "settled_error": settled_error,
            "healthy_false_alarm": bool(false_status) and clean_after_false is not None}


def main() -> int:
    case = sys.argv[1]
    hashes = {str(path.relative_to(REPO)): hashlib.sha256(path.read_bytes()).hexdigest()
              for path in (NODE, CTL, DUMP, REPO / "src/alpha/node.rs",
                           REPO / "src/alpha/journal.rs", REPO / "src/alpha/log_source.rs")}
    result = {"reuse": reuse, "gaps": gaps,
              "inspect": concurrent_inspect}[case]()
    reproduced = {"reuse": "silent_loss", "gaps": "permanent_halt",
                  "inspect": "healthy_false_alarm"}[case]
    if case == "reuse":
        repair_ok = (result.get("available") and result["first_exit"] == 0
                     and result["second_exit"] == 0 and result["dump_exit"] == 0
                     and not result["missing_new"]
                     and int(result["inspect"]["gaps"]) >= 1)
    elif case == "gaps":
        control, repaired = result["control"], result["failure"]
        repair_ok = (control["first_exit"] == control["second_exit"] == 0
                     and repaired["first_exit"] == repaired["second_exit"] == 0
                     and not repaired["coverage_unknown_marker"]
                     and "logs=3" in repaired["second_output"])
    else:
        repair_ok = (result["calls"] >= 100
                     and result["false_status_count"] == 0
                     and result["settled_inspect_exit"] == 0
                     and result["settled_inspect"].get("recovery_required") == "false"
                     and int(result["settled_inspect"].get("batches", "0")) >= 3
                     and not result["marker_after_stop"])
    output = {"case": case, "repaired_hashes": hashes, "result": result,
              "repair_ok": repair_ok}
    (ROOT / f"repaired-{case}-result.json").write_text(json.dumps(output, indent=2,
                                                                sort_keys=True) + "\n")
    print(json.dumps(output, sort_keys=True))
    if case == "reuse" and not result.get("available"):
        return 2
    return 0 if repair_ok and not result[reproduced] else 1


if __name__ == "__main__":
    sys.exit(main())
