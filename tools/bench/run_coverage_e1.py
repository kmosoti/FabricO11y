#!/usr/bin/env python3
"""Preserve the E1R correctness run; this is not a performance benchmark."""
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
from datetime import datetime, timezone

ROOT = Path(__file__).resolve().parents[2]


def capture(args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: run_coverage_e1.py FRESH_OUTPUT_DIRECTORY")
    output = Path(sys.argv[1]).resolve()
    output.mkdir(parents=True, exist_ok=False)
    command = [
        "cargo", "test", "--offline", "--locked", "--manifest-path",
        "tools/storage-probe/Cargo.toml", "--test", "coverage_e1", "--",
        "--nocapture", "--test-threads=1",
    ]
    paths = [ROOT / "Cargo.toml", ROOT / "Cargo.lock", Path(__file__).resolve()]
    paths.extend((ROOT / "src").rglob("*.rs"))
    package = ROOT / "tools/storage-probe"
    paths.extend(package / name for name in ["Cargo.toml", "Cargo.lock", "COVERAGE_API.md"])
    paths.extend((package / "src").rglob("*.rs"))
    paths.extend((package / "tests").rglob("*.rs"))
    paths.append(ROOT / "docs/experiments/ablation/coverage-e1-protocol.md")
    metadata = {
        "started_utc": datetime.now(timezone.utc).isoformat(),
        "command": command,
        "working_directory": str(ROOT),
        "git_head": capture(["git", "rev-parse", "HEAD"]),
        "git_status": capture(["git", "status", "--short"]),
        "rustc": capture(["rustc", "--version", "--verbose"]),
        "cargo": capture(["cargo", "--version"]),
        "platform": platform.platform(),
        "python": platform.python_version(),
        "build_profile": "test (unoptimized)",
        "cargo_target_dir": os.environ.get("CARGO_TARGET_DIR"),
        "source_sha256": {
            str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(set(paths))
        },
        "scope": "finite in-memory correctness; no latency/durability claim",
    }
    (output / "tracked.diff").write_text(capture(["git", "diff", "HEAD", "--"]) + "\n")
    (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    with (output / "stdout.txt").open("w") as stdout, (output / "stderr.txt").open("w") as stderr:
        result = subprocess.run(command, cwd=ROOT, stdout=stdout, stderr=stderr, check=False)
    metadata["exit_code"] = result.returncode
    metadata["finished_utc"] = datetime.now(timezone.utc).isoformat()
    (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(f"E1R cargo exit {result.returncode}; evidence: {output}")
    raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()
