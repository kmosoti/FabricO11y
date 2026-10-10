"""Real contained verifier controls; fresh disk fixtures, no implementation mocks."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
from resource_group import require_limits, STORAGE


def main():
    group = require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if not args.out.is_absolute() or args.out.exists() or args.out.is_symlink():
        raise ValueError("fresh absolute evidence directory required")
    args.out.parent.resolve(strict=True).relative_to(STORAGE.resolve(strict=True))
    scratch = Path(os.environ["FABRIC_SCRATCH_ROOT"]).resolve(strict=True)
    scratch.relative_to((STORAGE / "scratch").resolve(strict=True))
    work = scratch / ("freeze-verifier-controls-" + str(os.getpid()))
    work.mkdir()
    args.out.mkdir()
    outcomes = []
    payload = b"frozen-member"
    diff = b"control-source-diff\n"
    outside = work / "outside.txt"
    outside.write_bytes(payload)
    for case in ("positive", "altered-bytes", "symlink", "traversal"):
        owned = work / case
        (owned / "frozen").mkdir(parents=True)
        (owned / "provenance").mkdir()
        (owned / ".fabric-soak-freeze-owned").write_text("fabric-soak-freeze-r2-v1\n")
        member = owned / "frozen/member.bin"
        member.write_bytes(payload)
        (owned / "provenance/source-diff.patch").write_bytes(diff)
        manifest = {"state": "frozen", "frozen_root": str(owned / "frozen"),
                    "source_diff_sha256": hashlib.sha256(diff).hexdigest(),
                    "members": {"frozen/member.bin": {
                        "bytes": len(payload), "sha256": hashlib.sha256(payload).hexdigest()}}}
        if case == "altered-bytes":
            member.write_bytes(b"changed-value")  # Same length; SHA must detect it.
        elif case == "symlink":
            member.unlink()
            member.symlink_to(outside)  # Same bytes; ownership must detect it.
        elif case == "traversal":
            manifest["members"] = {"../outside.txt": manifest["members"]["frozen/member.bin"]}
        manifest_path = owned / "provenance/manifest.json"
        manifest_path.write_text(json.dumps(manifest) + "\n")
        result_path = args.out / (case + ".verification.json")
        command = [sys.executable, "-B", str(HERE / "verify_soak_freeze.py"),
                   "--manifest", str(manifest_path), "--label", case, "--out", str(result_path)]
        began = time.monotonic()
        stop_reason = None
        try:
            child = subprocess.run(command, capture_output=True, text=True, timeout=30)
            code, stdout, stderr = child.returncode, child.stdout, child.stderr
        except subprocess.TimeoutExpired as error:
            code, stop_reason = None, "timeout"
            stdout = (error.stdout or b"").decode(errors="replace")
            stderr = (error.stderr or b"").decode(errors="replace")
        (args.out / (case + ".stdout")).write_text(stdout)
        (args.out / (case + ".stderr")).write_text(stderr)
        receipt = {"command": command, "exit": code, "stop_reason": stop_reason,
                   "elapsed_s": time.monotonic() - began, "cgroup": str(group),
                   "scratch": str(owned), "expected_exit": "zero" if case == "positive" else "nonzero"}
        (args.out / (case + ".command.json")).write_text(json.dumps(receipt, indent=2) + "\n")
        observed = json.loads(result_path.read_text()) if result_path.is_file() else {}
        expected = case == "positive"
        passed = stop_reason is None and (code == 0 if expected else code != 0) and observed.get("passed") is expected
        outcomes.append({"case": case, "control_passed": passed,
                         "verifier_exit": code, "verifier_passed": observed.get("passed")})
        (args.out / "controls.json").write_text(json.dumps(outcomes, indent=2) + "\n")
    if not all(row["control_passed"] for row in outcomes):
        raise RuntimeError("freeze verifier control failed; preserve receipts and owned scratch")
    shutil.rmtree(work)
    (args.out / "cleanup.json").write_text(json.dumps({"scratch": str(work), "removed": not work.exists()}) + "\n")
    print(json.dumps({"controls": outcomes, "cleanup_removed": not work.exists(), "evidence": str(args.out)}))


if __name__ == "__main__":
    main()
