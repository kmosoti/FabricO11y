#!/usr/bin/env python3
"""Run the frozen S1 experiment in a fresh directory and preserve provenance."""

import datetime
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from summarize_storage_s1 import analyze


ROOT = Path(__file__).resolve().parents[2]
PROTOCOL = Path("docs/experiments/ablation/storage-query-s1-protocol.md")
SOURCE_PATHS = (
    Path("Cargo.toml"), Path("Cargo.lock"),
    Path("tools/storage-probe/Cargo.toml"), Path("tools/storage-probe/Cargo.lock"),
    PROTOCOL,
    Path("tools/bench/run_storage_s1.py"), Path("tools/bench/summarize_storage_s1.py"),
    Path("tools/bench/test_storage_s1.py"),
)
SOURCE_DIRS = (Path("src"), Path("tools/storage-probe/src"), Path("tools/storage-probe/tests"))


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_hashes():
    paths = list(SOURCE_PATHS)
    for directory in SOURCE_DIRS:
        paths.extend(path.relative_to(ROOT) for path in (ROOT / directory).rglob("*.rs"))
    return {str(path): digest(ROOT / path) for path in sorted(set(paths))}


def command(args, cwd=ROOT):
    result = subprocess.run(args, cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False)
    print(f"$ {' '.join(map(str, args))} -> exit {result.returncode}", flush=True)
    print(result.stdout, end="", flush=True)
    if result.returncode:
        raise RuntimeError(f"command failed: {' '.join(map(str, args))} (exit {result.returncode})")
    return result.stdout


def benchmark(binary, output):
    # wait4 returns usage for this child alone, excluding the preceding Cargo commands.
    with tempfile.TemporaryFile(mode="w+b") as stdout, tempfile.TemporaryFile(mode="w+b") as stderr:
        process = subprocess.Popen([str(binary), "run", str(output)], cwd=ROOT, stdout=stdout, stderr=stderr)
        _, status, usage = os.wait4(process.pid, 0)
        code = os.waitstatus_to_exitcode(status)
        process.returncode = code
        stdout.seek(0)
        stderr.seek(0)
        out = stdout.read()
        err = stderr.read()
    print(f"$ {binary} run {output} -> exit {code}", flush=True)
    if err:
        print(err.decode("utf-8", errors="replace"), end="", file=sys.stderr, flush=True)
    if output.is_dir():
        (output / "benchmark.stdout.txt").write_bytes(out)
        (output / "benchmark.stderr.txt").write_bytes(err)
    if code:
        raise RuntimeError(f"benchmark failed (exit {code})")
    return {"command": [str(binary), "run", str(output)], "exit_code": code,
            "user_seconds": usage.ru_utime, "system_seconds": usage.ru_stime,
            "max_rss_kib": usage.ru_maxrss}


def main():
    if len(sys.argv) != 2:
        raise ValueError("usage: python3 -B tools/bench/run_storage_s1.py FRESH_OUTPUT_DIR")
    output = Path(os.path.abspath(sys.argv[1]))
    if os.path.lexists(output):
        raise ValueError(f"output already exists: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    if os.path.lexists(output):
        raise ValueError(f"output already exists: {output}")
    environment = {
        "utc_start": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "git_head": command(["git", "rev-parse", "HEAD"]).strip(),
        "git_status_porcelain": command(["git", "status", "--porcelain=v1"]),
        "rustc": command(["rustc", "--version"]).strip(),
        "cargo": command(["cargo", "--version"]).strip(),
        "python": sys.version,
        "uname": platform.uname()._asdict(),
        "cpu_model": next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), None),
        "filesystem": command(["findmnt", "--target", str(output.parent), "--output", "SOURCE,FSTYPE,TARGET", "--noheadings"]).strip(),
        "RUSTFLAGS": os.environ.get("RUSTFLAGS", ""),
        "source_sha256": source_hashes(),
    }
    commands = [
        ([sys.executable, "-B", "tools/bench/test_storage_s1.py"], ROOT),
        (["cargo", "test", "--offline", "--locked"], ROOT),
        (["cargo", "test", "--offline", "--locked", "--manifest-path", "tools/storage-probe/Cargo.toml"], ROOT),
        (["cargo", "build", "--offline", "--locked", "--release", "--manifest-path", "tools/storage-probe/Cargo.toml", "--target-dir", str(ROOT / "tools/storage-probe/target")], ROOT),
    ]
    checks = []
    for args, cwd in commands:
        command(args, cwd)
        checks.append({"command": args, "exit_code": 0})
    binary = ROOT / "tools/storage-probe/target/release/storage-probe"
    resources = benchmark(binary, output)
    summary = analyze(output)
    shutil.copyfile(ROOT / PROTOCOL, output / "protocol.md")
    environment["utc_end"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    environment["checks"] = checks
    (output / "environment.json").write_text(json.dumps(environment, indent=2, sort_keys=True) + "\n")
    (output / "resources.json").write_text(json.dumps(resources, indent=2, sort_keys=True) + "\n")
    (output / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    preserved = ("builds.csv", "queries.csv", "benchmark.stdout.txt", "benchmark.stderr.txt", "environment.json", "resources.json", "protocol.md", "summary.json")
    manifest = {name: digest(output / name) for name in preserved}
    (output / "SHA256SUMS.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"S1 result: {summary['status']}; preserved {output}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        print(error, file=sys.stderr)
        sys.exit(1)
