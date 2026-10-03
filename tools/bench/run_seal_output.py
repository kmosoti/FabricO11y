#!/usr/bin/env python3
"""Bounded paired R0 pilot; binary paths and an absent owned output root required."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import shutil
import signal
import subprocess
import time


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as f:
        for data in iter(lambda: f.read(1024 * 1024), b""):
            h.update(data)
    return h.hexdigest()


def compare_files(left, right):
    a = {str(p.relative_to(left)): digest(p) for p in left.rglob("*") if p.is_file()}
    b = {str(p.relative_to(right)): digest(p) for p in right.rglob("*") if p.is_file()}
    if not a or a != b:
        raise AssertionError("Segment output bytes differ")
    return a


def run(binary, state, shape, mib, seed, cpus):
    def limits():
        os.setsid()
        os.sched_setaffinity(0, cpus)
        resource.setrlimit(resource.RLIMIT_AS, (2 * 1024**3, 2 * 1024**3))
        resource.setrlimit(resource.RLIMIT_CPU, (180, 180))
    timing = state.with_suffix(".time.json")
    cmd = ["/usr/bin/time", "-f", '{"user_seconds":%U,"system_seconds":%S,"peak_rss_kib":%M}', "-o", str(timing), str(binary), str(state), shape, str(mib), str(seed)]
    start = time.monotonic()
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, preexec_fn=limits)
    try:
        out, err = p.communicate(timeout=180)
    except subprocess.TimeoutExpired:
        os.killpg(p.pid, signal.SIGKILL)
        out, err = p.communicate()
        state.with_suffix(".stderr.txt").write_bytes(err)
        raise RuntimeError("180-second trial watchdog")
    state.with_suffix(".stderr.txt").write_bytes(err)
    state.with_suffix(".stdout.json").write_bytes(out)
    if p.returncode:
        raise RuntimeError(f"trial exit {p.returncode}: {err.decode(errors='replace')}")
    result = json.loads(out)
    result.update(json.loads(timing.read_text()))
    result.update(command=cmd, exit_code=p.returncode, process_wall_seconds=time.monotonic()-start)
    return result


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("baseline", type=Path)
    ap.add_argument("candidate", type=Path)
    ap.add_argument("output", type=Path)
    args = ap.parse_args()
    if args.output.exists():
        ap.error("output must not exist")
    for binary in (args.baseline, args.candidate):
        if not binary.is_file():
            ap.error(f"missing binary: {binary}")
    args.output.mkdir(parents=True)
    (args.output / "owned-by-seal-output-pilot").write_text("v1\n")
    cpus = sorted(os.sched_getaffinity(0))[:2]
    metadata = {"baseline_binary_sha256": digest(args.baseline), "candidate_binary_sha256": digest(args.candidate), "cpu_affinity": cpus, "address_space_limit_bytes": 2*1024**3}
    (args.output / "metadata.json").write_text(json.dumps(metadata, indent=2)+"\n")
    # Verify this output-comparison checker can reject a one-byte mutation.
    for name in ("control-a", "control-b"):
        d = args.output / name
        d.mkdir()
        (d / "payload").write_bytes(b"exact")
    compare_files(args.output / "control-a", args.output / "control-b")
    (args.output / "control-b" / "payload").write_bytes(b"exacX")
    try:
        compare_files(args.output / "control-a", args.output / "control-b")
    except AssertionError:
        (args.output / "negative-control.json").write_text('{"equal_accepted":true,"one_byte_change_rejected":true}\n')
    else:
        raise AssertionError("output checker accepted corrupt bytes")
    campaign = time.monotonic()
    pairs = []
    for shape in ("repeat", "entropy"):
        for mib in (16, 64):
            for seed in (2703163393, 2703163394, 2703163395):
                if time.monotonic()-campaign > 1800:
                    raise RuntimeError("30-minute campaign watchdog")
                if shutil.disk_usage(args.output).free < 4*1024**3:
                    raise RuntimeError("4 GiB free-disk floor")
                order = ("baseline", "candidate") if len(pairs)%2 == 0 else ("candidate", "baseline")
                pair = {"shape": shape, "mib": mib, "seed": seed, "order": order}
                states = {v: args.output/f"{shape}-{mib}-{seed}-{v}" for v in order}
                for variant in order:
                    pair[variant] = run(getattr(args, variant).resolve(), states[variant].resolve(), shape, mib, seed, cpus)
                if pair["baseline"]["input_sha256"] != pair["candidate"]["input_sha256"]:
                    raise AssertionError("input fixture mismatch")
                pair["output_sha256"] = compare_files(states["baseline"], states["candidate"])
                pair["ratios"] = {key: pair["candidate"][key]/pair["baseline"][key] for key in ("incremental_peak_heap_bytes", "peak_rss_kib", "build_seconds_instrumented")}
                pairs.append(pair)
                (args.output / "pairs.json").write_text(json.dumps(pairs, indent=2)+"\n")
                print(json.dumps({"shape":shape,"mib":mib,"seed":seed,"ratios":pair["ratios"],"exact_output":True}), flush=True)
                for state in states.values():
                    shutil.rmtree(state)
    (args.output / "complete.json").write_text(json.dumps({"pairs":len(pairs),"exit_code":0})+"\n")


if __name__ == "__main__":
    main()
