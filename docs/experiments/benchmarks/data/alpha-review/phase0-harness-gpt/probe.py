#!/usr/bin/env python3
"""Independent phase-0 harness regression probes. Only target/alpha-* is written."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import sys
import time
import uuid

REPO = Path(__file__).resolve().parents[2]
TARGET = REPO / "target"
sys.path.insert(0, str(REPO / "tools" / "alpha"))
from runner import live_bytes, owned_root, run  # noqa: E402

SOURCES = [
    "tools/alpha/runner.py", "tools/alpha/workload.py",
    "tools/alpha/rate_oracle.py", "tools/alpha/test_runner.py",
    "tools/alpha/test_workload.py", "docs/ALPHA.md",
]


def hashes():
    return {name: hashlib.sha256((REPO / name).read_bytes()).hexdigest()
            for name in SOURCES}


def new_root(label):
    return owned_root(TARGET / f"alpha-gpt-review-{uuid.uuid4().hex[:12]}-{label}")


def clean(root):
    owned_root(root)
    live_bytes(root)
    shutil.rmtree(root)


def short(result):
    return {key: result.get(key) for key in
            ("exit_code", "passed", "stop_reason", "rate_contract_error",
             "retained_live_bytes", "source_seed", "process_group_cleanup_ok")}


def main():
    before = hashes()
    cases = []

    for mutation in ("valid", "skip", "extra", "wrong_node", "wrong_kind",
                     "wrong_entropy", "wrong_metric", "extra_field"):
        label = "rate-positive" if mutation == "valid" else f"rate-{mutation}"
        root = new_root(label)
        try:
            command = [sys.executable, str(Path(__file__).with_name("rate_child.py")),
                       mutation]
            result = run(root, command,
                         3, 500_000, 500_000, (10, 1))
            expected = mutation == "valid"
            specific_rejection = (expected or (result["rate_contract_error"] is not None
                                  and "missing, linked or oversized rate file"
                                  not in result["rate_contract_error"]))
            cases.append({"case": label, "input": {"mutation": mutation,
                          "csv": "0,20,320,340,0,340,0"},
                          "result": short(result),
                          "expected_passed": expected,
                          "ok": result["passed"] is expected and specific_rejection})
        finally:
            clean(root)

    for label, code, disk, evidence, expected in (
        ("runner-positive", "print('ok')", 100_000, 20_000, True),
        ("fast-disk-negative", "open('large','wb').write(b'x'*100000)",
         1_000, 1_000, False),
    ):
        root = new_root(label)
        try:
            result = run(root, [sys.executable, "-c", code],
                         3, disk, evidence)
            cases.append({"case": label, "input": {"code": code,
                          "disk_bytes": disk, "evidence_bytes": evidence},
                          "result": short(result), "expected_passed": expected,
                          "ok": result["passed"] is expected})
        finally:
            clean(root)

    root = new_root("evidence-disposable-data")
    try:
        command = [sys.executable, str(REPO / "tools/alpha/workload.py"),
                   "--tier", "10", "--seconds", "1", "--seed", "0xA11FA001",
                   "--out", "offers.jsonl", "--byte-cap", "400000"]
        result = run(root, command, 5, 500_000, 20_000, (10, 1))
        cases.append({"case": "evidence-disposable-data",
                      "input": {"tier": 10, "seconds": 1,
                                "disk_bytes": 500_000,
                                "evidence_bytes": 20_000,
                                "source_bytes": (root / "offers.jsonl").stat().st_size},
                      "result": short(result), "expected_passed": True,
                      "ok": result["passed"] is True})
    finally:
        clean(root)

    root = new_root("stale-fixtures")
    try:
        first = run(root, [sys.executable,
                           str(Path(__file__).with_name("rate_child.py")), "valid"],
                    3, 500_000, 500_000, (10, 1))
        second = run(root, [sys.executable, "-c", "pass"],
                     3, 500_000, 500_000, (10, 1))
        retained_report = json.loads((root / "result.json").read_text())
        stale_files = [(root / name).exists() for name in ("offered.csv", "offers.jsonl")]
        cases.append({"case": "stale-fixtures-and-result",
                      "input": "valid rate run followed by no-op command in same owned root",
                      "first": short(first), "second": short(second),
                      "stale_files": stale_files,
                      "ok": first["passed"] and not second["passed"]
                            and first["invocation_id"] != second["invocation_id"]
                            and retained_report["invocation_id"] == second["invocation_id"]
                            and not any(stale_files)})
    finally:
        clean(root)

    root = new_root("temp-churn")
    try:
        churn = ("import os,tempfile; from pathlib import Path; "
                 "[(lambda p: (p.write_bytes(b'x'*64),p.unlink()))"
                 "(Path(tempfile.gettempdir())/('churn-'+str(i))) for i in range(1000)]")
        result = run(root, [sys.executable, "-c", churn], 3, 100_000, 20_000)
        cases.append({"case": "temp-file-churn",
                      "input": "1000 create/write/unlink cycles in owned TMPDIR",
                      "result": short(result), "expected_passed": True,
                      "ok": result["passed"] is True})
    finally:
        clean(root)

    root = new_root("detached-descendant")
    try:
        late = "import time; from pathlib import Path; time.sleep(0.2); Path('late.txt').write_text('late')"
        code = ("import subprocess,sys; from pathlib import Path; "
                "p=subprocess.Popen([sys.executable,'-c'," + repr(late) +
                "],start_new_session=True); Path('detached.pid').write_text(str(p.pid))")
        result = run(root, [sys.executable, "-c", code], 3, 100_000, 100_000)
        pid = int((root / "detached.pid").read_text())
        time.sleep(0.3)
        late_exists = (root / "late.txt").exists()
        try:
            waited, _ = os.waitpid(pid, os.WNOHANG)
            if waited == 0:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
        except (ChildProcessError, ProcessLookupError):
            pass
        cases.append({"case": "detached-descendant",
                      "input": {"child": "new session; sleep 0.2s; write late.txt"},
                      "result": short(result), "late_file_after_result": late_exists,
                      "expected_passed": False,
                      "ok": result["passed"] is False})
    finally:
        clean(root)

    root = new_root("detached-ignores-term")
    try:
        late = ("import signal,time; from pathlib import Path; "
                "signal.signal(signal.SIGTERM,signal.SIG_IGN); "
                "Path('ready').write_text('ready'); time.sleep(0.8); "
                "Path('late.txt').write_text('late')")
        code = ("import subprocess,sys,time; from pathlib import Path; "
                "p=subprocess.Popen([sys.executable,'-c'," + repr(late) +
                "],start_new_session=True); Path('detached.pid').write_text(str(p.pid)); "
                "\nwhile not Path('ready').exists(): time.sleep(0.001)")
        result = run(root, [sys.executable, "-c", code], 3, 100_000, 100_000)
        pid = int((root / "detached.pid").read_text())
        time.sleep(0.3)
        late_exists = (root / "late.txt").exists()
        alive = Path(f"/proc/{pid}").exists()
        try:
            waited, _ = os.waitpid(pid, os.WNOHANG)
            if waited == 0:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
        except (ChildProcessError, ProcessLookupError):
            pass
        cases.append({"case": "detached-ignores-term",
                      "input": "setsid child ignores SIGTERM, then sleeps and writes late.txt",
                      "result": short(result), "late_file_after_result": late_exists,
                      "child_alive_after_result": alive,
                      "ok": result["passed"] is False and not late_exists and not alive
                            and result["process_group_cleanup_ok"] is True})
    finally:
        clean(root)

    root = TARGET / f"alpha-gpt-review-{uuid.uuid4().hex[:12]}-nan"
    try:
        run(root, [sys.executable, "-c", "pass"], float("nan"), 1_000, 1_000)
        rejected = False
    except ValueError:
        rejected = True
    if root.exists():
        clean(root)
    cases.append({"case": "nan-duration-negative", "input": "duration_s=NaN",
                  "rejected": rejected, "ok": rejected})

    after = hashes()
    verdict = {"command": "python3 target/alpha-harness-review-gpt/probe.py",
               "source_hashes_before": before, "source_hashes_after": after,
               "sources_stable": before == after, "cases": cases,
               "all_passed": before == after and all(case["ok"] for case in cases)}
    verdict["probe_exit_code"] = 0 if verdict["all_passed"] else 1
    output = Path(__file__).with_name("results.json")
    output.write_text(json.dumps(verdict, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"all_passed": verdict["all_passed"],
                      "sources_stable": verdict["sources_stable"],
                      "cases": [{"case": case["case"], "ok": case["ok"],
                                 "result": case.get("result")}
                                for case in cases]}, sort_keys=True))
    return verdict["probe_exit_code"]


if __name__ == "__main__":
    sys.exit(main())
