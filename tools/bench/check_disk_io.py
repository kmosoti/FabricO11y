#!/usr/bin/env python3
"""Linux-only read-error probes with real failure and positive-control commands."""
from pathlib import Path
import hashlib
import json
import os
import platform
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]

def main():
    if len(sys.argv) not in (2, 3) or platform.system() != "Linux":
        raise SystemExit("usage (Linux): check_disk_io.py FRESH_OUTPUT [REPO_ROOT]")
    output = Path(sys.argv[1]).resolve()
    output.mkdir(parents=True, exist_ok=False)
    repo = Path(sys.argv[2]).resolve() if len(sys.argv) == 3 else ROOT
    support = repo / "tools/storage-probe/tests/io_fault_support"
    records = []
    for name, source, variable in [
        ("raw", "partial_read.c", "INJECT_PARTIAL_RAW"),
        ("metadata", "partial_read_metadata.c", "INJECT_PARTIAL_METADATA"),
    ]:
        library = output / f"{name}.so"
        commands = [
            ["cc", "-shared", "-fPIC", "-o", str(library), str(support / source), "-ldl"],
            ["cargo", "test", "--offline", "--locked", "--manifest-path",
             "tools/storage-probe/Cargo.toml", "--test", "disk_io_fault", f"{name}::",
             "--", "--ignored", "--nocapture", "--test-threads=1"],
        ]
        for step, command in enumerate(commands):
            env = os.environ.copy()
            if step:
                env["LD_PRELOAD"] = str(library)
                if name == "raw":
                    env[variable] = "1"  # Metadata fixture activates after retaining fallback bytes.
            result = subprocess.run(command, cwd=repo, env=env, text=True, capture_output=True)
            prefix = f"{name}-{step}"
            (output / f"{prefix}.stdout").write_text(result.stdout)
            (output / f"{prefix}.stderr").write_text(result.stderr)
            records.append({"command": command, "cwd": str(repo), "exit_code": result.returncode,
                            "injected": bool(step), "variable": variable if step else None,
                            "activation": ("fixture" if name == "metadata" else "runner") if step else None,
                            "source_sha256": hashlib.sha256((support / source).read_bytes()).hexdigest()})
            (output / "commands.json").write_text(json.dumps(records, indent=2) + "\n")
            if result.returncode:
                print(f"{name} step {step}: exit {result.returncode}")
                raise SystemExit(result.returncode)
            if step and ("1 passed" not in result.stdout or "EIO" not in result.stderr):
                raise SystemExit("probe did not execute or injection was not observed")
    print("raw and metadata read-error probes passed with observed EIO")

if __name__ == "__main__":
    main()
