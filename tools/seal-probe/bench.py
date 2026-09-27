#!/usr/bin/env python3
"""Paired, disposable-file E2R sync-placement cost probe.

The writer uses candidate framing. Verification uses only the frozen oracle
fixture and an independent field walk. This measures a research format, not FOL2.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import statistics
import struct
import subprocess
import sys
import time
import traceback

import candidate
import oracle_support as oracle

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SOURCE_FILES = (HERE / "bench.py", HERE / "candidate.py", HERE / "oracle_support.py",
                HERE / "CONTRACT.md", ROOT / "docs/experiments/benchmarks/group-seal-cost-protocol.md")
FULL_COUNT = 1200
VARIANTS = ("one", "two")


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def groups_for(count: int, size: int) -> tuple[tuple[bytes, ...], ...]:
    if count < 1 or size not in (1, 2, 4, 6) or count % size:
        raise ValueError("body count must be positive and divisible by group size")
    frozen = oracle.bodies(6, 11)
    bodies = tuple(frozen[i % 6] for i in range(count))
    return tuple(bodies[i:i + size] for i in range(0, count, size))


def write_all(fd: int, data: bytes) -> None:
    view = memoryview(data)
    while view:
        n = os.write(fd, view)
        if n <= 0:
            raise OSError("write made no progress")
        view = view[n:]


def write_trial(path: Path, sector: int, size: int, variant: str, count: int) -> dict:
    if sector not in oracle.SECTORS or variant not in VARIANTS:
        raise ValueError("invalid configuration")
    groups = groups_for(count, size)  # source generation outside timing
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        # The newly created parent is synced by run() before this process starts.
        # Sync the new file's directory entry outside the ingest interval too.
        parent_fd = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(parent_fd)
        finally:
            os.close(parent_fd)
        commits = []
        encode_ns = 0
        syncs = 0
        cpu_start = time.process_time_ns()
        wall_start = time.perf_counter_ns()
        for index, group in enumerate(groups):
            commit_start = time.perf_counter_ns()
            encode_start = time.perf_counter_ns()
            encoded = candidate._encode_group(index, group, sector)
            encode_ns += time.perf_counter_ns() - encode_start
            if len(encoded) % sector or len(encoded) < 3 * sector:
                raise ValueError("candidate encoded an invalid group span")
            # Hold write boundaries constant so only sync placement/count varies.
            write_all(fd, encoded[:-sector])
            if variant == "two":
                os.fsync(fd)
                syncs += 1
            write_all(fd, encoded[-sector:])
            os.fsync(fd)
            syncs += 1
            commits.append(time.perf_counter_ns() - commit_start)
        wall_ns = time.perf_counter_ns() - wall_start
        cpu_ns = time.process_time_ns() - cpu_start
        file_bytes = os.fstat(fd).st_size
    finally:
        os.close(fd)
    ordered = sorted(commits)
    return {"sector": sector, "group_size": size, "variant": variant,
            "events": count, "groups": len(groups), "pipeline_wall_ns": wall_ns,
            "pipeline_cpu_ns": cpu_ns, "encoding_ns": encode_ns,
            "commit_elapsed_ns": commits,
            "commit_p50_ns": ordered[math.ceil(len(ordered) * .50) - 1],
            "commit_p99_ns": ordered[math.ceil(len(ordered) * .99) - 1],
            "successful_fsyncs": syncs, "file_bytes": file_bytes,
            "events_per_second": count * 1e9 / wall_ns,
            "pipeline_cpu_s_per_event": cpu_ns / 1e9 / count,
            "syncs_per_event": syncs / count, "bytes_per_event": file_bytes / count}


def verify_trial(path: Path, sector: int, size: int, count: int) -> dict:
    groups = groups_for(count, size)
    image = path.read_bytes()
    expected = oracle.fixture(groups, sector)
    if image != expected:
        first = next((i for i, (a, b) in enumerate(zip(image, expected)) if a != b),
                     min(len(image), len(expected)))
        raise ValueError(f"disk bytes differ from frozen fixture at offset {first}; "
                         f"actual={len(image)} expected={len(expected)}")
    recovered = []
    offset = 0
    for group_id, group in enumerate(groups):
        descriptor = image[offset:offset + sector]
        if descriptor[:4] != b"FGD1" or struct.unpack_from("<QI", descriptor, 4) != (group_id, len(group)):
            raise ValueError(f"descriptor mismatch at group {group_id}")
        lengths = struct.unpack_from("<6I", descriptor, 16)
        if lengths != tuple(len(b) for b in group) + (0,) * (6 - len(group)):
            raise ValueError(f"lengths mismatch at group {group_id}")
        offset += sector
        decoded = []
        for frame_id, body in enumerate(group):
            head = image[offset:offset + 12]
            if head[:4] != b"FGF1" or struct.unpack_from("<II", head, 4) != (frame_id, len(body)):
                raise ValueError(f"frame header mismatch at group {group_id}, frame {frame_id}")
            decoded.append(image[offset + 12:offset + 12 + len(body)])
            offset += ((12 + len(body) + sector - 1) // sector) * sector
        seal = image[offset:offset + sector]
        if seal[:4] != b"FGS1" or struct.unpack_from("<QI", seal, 4) != (group_id, len(group)):
            raise ValueError(f"seal mismatch at group {group_id}")
        offset += sector
        if tuple(decoded) != group:
            raise ValueError(f"body mismatch at group {group_id}")
        recovered.append(tuple(decoded))
    if offset != len(image) or tuple(recovered) != groups:
        raise ValueError("replay group boundaries or count mismatch")
    return {"verified_groups": len(recovered), "verified_events": count,
            "file_bytes": len(image), "sha256": hashlib.sha256(image).hexdigest(),
            "fixture_sha256": hashlib.sha256(expected).hexdigest()}


def run_child(command: list[str], out: Path, stem: str) -> dict:
    stdout = out / f"{stem}.stdout"
    stderr = out / f"{stem}.stderr"
    started = time.perf_counter_ns()
    with stdout.open("xb") as so, stderr.open("xb") as se:
        process = subprocess.Popen(command, stdout=so, stderr=se, cwd=ROOT)
        _, status, usage = os.wait4(process.pid, 0)
        process.returncode = os.waitstatus_to_exitcode(status)
    record = {"name": stem, "command": command, "exit_code": process.returncode,
              "child_wall_ns_including_setup": time.perf_counter_ns() - started,
              "child_user_cpu_s": usage.ru_utime, "child_system_cpu_s": usage.ru_stime,
              "child_peak_rss_kib": usage.ru_maxrss, "child_input_blocks": usage.ru_inblock,
              "child_output_blocks": usage.ru_oublock,
              "stdout": stdout.name, "stderr": stderr.name}
    with (out / "commands.jsonl").open("a", encoding="utf-8") as log:
        log.write(json.dumps(record, sort_keys=True) + "\n")
    if process.returncode:
        raise RuntimeError(f"{stem} exited {process.returncode}; see {stderr}")
    return record


def environment() -> dict:
    result = {"utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
              "python": sys.version, "platform": platform.platform(),
              "source_sha256": {str(p.relative_to(ROOT)): sha256(p) for p in SOURCE_FILES}}
    for key, cmd in (("git_head", ["git", "rev-parse", "HEAD"]),
                     ("git_status", ["git", "status", "--short"]),
                     ("mount", ["findmnt", "-T", str(ROOT), "-o", "SOURCE,FSTYPE,OPTIONS", "-n"]),
                     ("cpu", ["lscpu"])):
        p = subprocess.run(cmd, cwd=ROOT, text=True, capture_output=True)
        result[key] = {"command": cmd, "exit_code": p.returncode,
                       "stdout": p.stdout, "stderr": p.stderr}
    # Process names only; command lines can contain private arguments.
    p = subprocess.run(["ps", "-eo", "comm="], text=True, capture_output=True)
    result["process_names"] = {"command": ["ps", "-eo", "comm="],
                               "exit_code": p.returncode, "stdout": p.stdout,
                               "stderr": p.stderr}
    return result


def summarize(trials: list[dict], count: int) -> dict:
    configurations = []
    for sector in oracle.SECTORS:
        for size in (1, 2, 4, 6):
            pairs = [t for t in trials if t["sector"] == sector and t["group_size"] == size
                     and t["pair"] != "warmup"]
            gains = []
            cpu_ratios = []
            for pair_no in range(1, 6):
                variants = {t["variant"]: t for t in pairs if t["pair"] == pair_no}
                if set(variants) != set(VARIANTS):
                    raise ValueError("incomplete pair")
                a, b = variants["one"], variants["two"]
                if a["verify"]["sha256"] != b["verify"]["sha256"]:
                    raise ValueError("paired file hash mismatch")
                gains.append(a["write"]["events_per_second"] / b["write"]["events_per_second"] - 1)
                cpu_ratios.append(a["write"]["pipeline_cpu_ns"] / b["write"]["pipeline_cpu_ns"])
            item = {"sector": sector, "group_size": size, "paired_throughput_gains": gains,
                    "paired_pipeline_cpu_ratios": cpu_ratios,
                    "median_throughput_gain": statistics.median(gains),
                    "median_pipeline_cpu_ratio": statistics.median(cpu_ratios)}
            if size == 1:
                item["material_benefit_gate"] = (item["median_throughput_gain"] >= .10
                                                 and item["median_pipeline_cpu_ratio"] <= 1.10)
            configurations.append(item)
    return {"events_per_trial": count, "configurations": configurations,
            "interpretation": "Research group-seal sync-placement cost only; no direct FOL2/S0 comparison or physical-sector durability claim."}


def run(out: Path, count: int, smoke: bool = False) -> None:
    if not out.parent.is_dir():
        raise ValueError("output parent must already exist and be durable")
    if not smoke and not out.is_relative_to(ROOT):
        raise ValueError("full trials must use an output directory within the repository")
    out.mkdir(exist_ok=False)
    parent_fd = os.open(out.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(parent_fd)  # durable new output-directory entry, outside timing
    finally:
        os.close(parent_fd)
    (out / "environment.json").write_text(json.dumps(environment(), indent=2) + "\n")
    trials = []
    configs = ((512, 1), (4096, 6)) if smoke else tuple((s, g) for s in oracle.SECTORS for g in (1, 2, 4, 6))
    pair_ids = ("warmup",) if smoke else ("warmup", 1, 2, 3, 4, 5)
    for sector, size in configs:
        for pair in pair_ids:
            order = VARIANTS if pair == "warmup" or pair % 2 else VARIANTS[::-1]
            for variant in order:
                stem = f"s{sector}-g{size}-{pair}-{variant}"
                path = out / f"{stem}.seal"
                base = [sys.executable, str(HERE / "bench.py")]
                common = [str(path), str(sector), str(size), str(count)]
                wr = run_child(base + ["_write", *common, variant], out, stem + "-write")
                ve = run_child(base + ["_verify", *common], out, stem + "-verify")
                write = json.loads((out / wr["stdout"]).read_text())
                verify = json.loads((out / ve["stdout"]).read_text())
                expected_syncs = (count // size) * (1 if variant == "one" else 2)
                if write["successful_fsyncs"] != expected_syncs or write["file_bytes"] != verify["file_bytes"]:
                    raise ValueError(f"counts differ for {stem}")
                trials.append({"sector": sector, "group_size": size, "pair": pair,
                               "variant": variant, "write": write, "verify": verify,
                               "writer_peak_rss_kib": wr["child_peak_rss_kib"],
                               "writer": wr, "verifier": ve, "file": path.name})
                (out / "trials.json").write_text(json.dumps(trials, indent=2) + "\n")
    if smoke:
        path = out / "smoke-corrupt.seal"
        original = out / "s512-g1-warmup-one.seal"
        data = bytearray(original.read_bytes())
        data[512 + 12] ^= 1
        path.write_bytes(data)
        cmd = [sys.executable, str(HERE / "bench.py"), "_verify", str(path), "512", "1", str(count)]
        try:
            record = run_child(cmd, out, "smoke-corrupt-verify")
        except RuntimeError:
            error = (out / "smoke-corrupt-verify.stderr").read_text()
            if "disk bytes differ from frozen fixture" not in error:
                raise ValueError("corruption check failed for an unexpected reason")
        else:
            raise ValueError(f"corruption checker unexpectedly passed: {record}")
        (out / "smoke.json").write_text(json.dumps({"clean_variants_verified": 4,
                                                       "corruption_rejected": True}, indent=2) + "\n")
    else:
        (out / "summary.json").write_text(json.dumps(summarize(trials, count), indent=2) + "\n")
    (out / "artifact_sha256.json").write_text(json.dumps(
        {p.name: sha256(p) for p in sorted(out.iterdir()) if p.is_file() and p.name != "artifact_sha256.json"},
        indent=2) + "\n")
    print(json.dumps({"output": str(out), "trials": len(trials), "smoke": smoke}))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="mode", required=True)
    for mode in ("run", "smoke"):
        p = sub.add_parser(mode)
        p.add_argument("output_dir", type=Path)
    for mode in ("_write", "_verify"):
        p = sub.add_parser(mode)
        p.add_argument("path", type=Path)
        p.add_argument("sector", type=int)
        p.add_argument("group_size", type=int)
        p.add_argument("count", type=int)
        if mode == "_write":
            p.add_argument("variant", choices=VARIANTS)
    args = parser.parse_args()
    try:
        if args.mode in ("run", "smoke"):
            run(args.output_dir.resolve(), 12 if args.mode == "smoke" else FULL_COUNT,
                args.mode == "smoke")
        elif args.mode == "_write":
            print(json.dumps(write_trial(args.path, args.sector, args.group_size,
                                         args.variant, args.count)))
        else:
            print(json.dumps(verify_trial(args.path, args.sector, args.group_size, args.count)))
        return 0
    except Exception:
        traceback.print_exc(file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
