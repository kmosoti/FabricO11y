"""One bounded native-node trial under runner.py's owned target/alpha-* root."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

from workload import SEEDS, entropy_body

WARMUP = 15
MEASURED = 120
TOTAL = WARMUP + MEASURED
LOGS_PER_SECOND = 2
MAX_SOURCE_BYTES = 1024 * 1024


def proc_cpu_ticks(pid: int) -> int:
    fields = Path(f"/proc/{pid}/stat").read_text().rpartition(") ")[2].split()
    return int(fields[11]) + int(fields[12])


def proc_memory_kib(pid: int, key: str) -> int:
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        if line.startswith(key + ":"):
            return int(line.split()[1])
    raise RuntimeError(f"missing {key} for native node")


def inspect(ctl: Path, config: Path) -> dict:
    result = subprocess.run([str(ctl), "inspect", str(config)], capture_output=True,
                            text=True, timeout=5)
    if result.returncode:
        raise RuntimeError(f"fabricctl inspect exit {result.returncode}: {result.stderr.strip()}")
    return dict(part.split("=", 1) for part in result.stdout.split())


def run(seed: int, node_bin: Path, ctl_bin: Path, dump_bin: Path) -> dict:
    if seed not in SEEDS:
        raise ValueError("unregistered native trial seed")
    root = Path.cwd().resolve()
    target = Path(__file__).resolve().parents[2] / "target"
    if root.parent != target.resolve() or not root.name.startswith("alpha-"):
        raise ValueError("native trial requires direct owned target/alpha-* root")
    if (root / ".fabric-alpha-owned").read_text() != "fabric-alpha-runner-v1\n":
        raise ValueError("native trial lacks runner ownership marker")
    for output in ("source.log", "node.conf", "node-output.log", "native-summary.json"):
        if (root / output).exists():
            raise ValueError(f"native trial output already exists: {output}")
    node_bin = node_bin.resolve(strict=True)
    ctl_bin = ctl_bin.resolve(strict=True)
    dump_bin = dump_bin.resolve(strict=True)
    spool = root / "spool"
    source = root / "source.log"
    config = root / "node.conf"
    config.write_text(f"spool_dir={spool}\nlog={source}\nmetric_interval_s=15\nspool_bytes=268435456\n")
    source.touch()
    source_bytes = 0
    source_hash = hashlib.sha256()
    max_lag_ms = 0.0
    peak_rss_kib = 0
    cpu_start = cpu_end = None
    cpu_start_elapsed = None
    node_output = (root / "node-output.log").open("xb")
    child = subprocess.Popen([str(node_bin), "run", str(config)], stdout=node_output,
                             stderr=subprocess.STDOUT)
    began = time.monotonic()
    try:
        with source.open("ab") as sink:
            for tick in range(TOTAL * LOGS_PER_SECOND):
                due = began + tick / LOGS_PER_SECOND
                time.sleep(max(0.0, due - time.monotonic()))
                lag = max(0.0, (time.monotonic() - due) * 1000)
                max_lag_ms = max(max_lag_ms, lag)
                if child.poll() is not None:
                    raise RuntimeError(f"native node exited early: {child.returncode}")
                body = "R" * 512 if tick % 2 == 0 else entropy_body(seed, 0, tick)
                line = (body + "\n").encode()
                if source_bytes + len(line) > MAX_SOURCE_BYTES:
                    raise RuntimeError("cooperative native source byte cap")
                sink.write(line)
                sink.flush()
                source_hash.update(line)
                source_bytes += len(line)
                peak_rss_kib = max(peak_rss_kib, proc_memory_kib(child.pid, "VmRSS"))
                if tick == WARMUP * LOGS_PER_SECOND:
                    cpu_start = proc_cpu_ticks(child.pid)
                    cpu_start_elapsed = time.monotonic() - began
            sink.flush()
            os.fsync(sink.fileno())
        deadline = began + TOTAL + 25
        status = None
        while time.monotonic() < deadline:
            if child.poll() is not None:
                raise RuntimeError(f"native node exited before source drain: {child.returncode}")
            peak_rss_kib = max(peak_rss_kib, proc_memory_kib(child.pid, "VmRSS"))
            try:
                status = inspect(ctl_bin, config)
                if int(status["log_records"]) >= TOTAL * LOGS_PER_SECOND:
                    break
            except RuntimeError:
                pass  # An inspect racing an append may see an incomplete suffix.
            time.sleep(0.2)
        else:
            raise RuntimeError("native node did not drain all 270 offered logs")
        cpu_end = proc_cpu_ticks(child.pid)
        cpu_end_elapsed = time.monotonic() - began
        hwm_kib = proc_memory_kib(child.pid, "VmHWM")
        if int(status["log_records"]) != TOTAL * LOGS_PER_SECOND or int(status["gaps"]) != 0:
            raise RuntimeError("native log count/gap mismatch")
        if status["coverage_unknown"] != "false" or status["recovery_required"] != "false":
            raise RuntimeError("native spool status is incomplete")
        if hwm_kib > 64 * 1024:
            raise RuntimeError(f"native VmHWM {hwm_kib} KiB exceeds 64 MiB")
        measured_cpu_s = (cpu_end - cpu_start) / os.sysconf("SC_CLK_TCK")
        result = {"seed": seed, "warmup_s": WARMUP, "measured_s": MEASURED,
                  "offered_logs": TOTAL * LOGS_PER_SECOND,
                  "committed_logs": int(status["log_records"]),
                  "actual_metric_points": int(status["metric_points"]),
                  "source_bytes": source_bytes, "source_sha256": source_hash.hexdigest(),
                  "max_source_lag_ms": round(max_lag_ms, 3),
                  "native_measured_cpu_s": round(measured_cpu_s, 6),
                  "cpu_window_start_elapsed_s": round(cpu_start_elapsed, 3),
                  "cpu_window_end_elapsed_s": round(cpu_end_elapsed, 3),
                  "native_peak_rss_kib_sampled": peak_rss_kib,
                  "native_vmhwm_kib": hwm_kib,
                  "spool_committed_bytes": int(status["committed_bytes"]),
                  "otlp_payload_bytes": int(status["otlp_payload_bytes"]),
                  "drain_elapsed_s": round(time.monotonic() - began, 3),
                  "node_pid": child.pid}
    finally:
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        node_output.close()
    restart_at = time.monotonic()
    restart = subprocess.run([str(node_bin), "collect", str(config)], capture_output=True,
                             text=True, timeout=10)
    if restart.returncode:
        raise RuntimeError(f"native restart exit {restart.returncode}: {restart.stderr.strip()}")
    after = inspect(ctl_bin, config)
    if int(after["log_records"]) != TOTAL * LOGS_PER_SECOND:
        raise RuntimeError("native restart duplicated or lost a source line")
    if after["coverage_unknown"] != "false" or after["recovery_required"] != "false":
        raise RuntimeError("native restart has incomplete spool status")
    replay_hash = hashlib.sha256()
    replay = subprocess.run([str(dump_bin), str(config)], capture_output=True, timeout=10)
    if replay.returncode != 0 or len(replay.stdout) > MAX_SOURCE_BYTES:
        raise RuntimeError(f"native exact replay failed: {replay.stderr[:400]!r}")
    replay_hash.update(replay.stdout)
    if replay_hash.hexdigest() != source_hash.hexdigest():
        raise RuntimeError("native committed log-body SHA256 differs from source")
    result["restart_ms"] = round((time.monotonic() - restart_at) * 1000, 3)
    result["restart_exit"] = restart.returncode
    result["replay_sha256"] = replay_hash.hexdigest()
    result["post_restart_otlp_payload_bytes"] = int(after["otlp_payload_bytes"])
    result["post_restart_committed_bytes"] = int(after["committed_bytes"])
    result["spool_live_bytes"] = sum(p.stat().st_size for p in spool.iterdir() if p.is_file())
    result["spool_amplification"] = round(
        result["spool_live_bytes"] / result["post_restart_otlp_payload_bytes"], 6)
    result["all_live_bytes"] = sum(
        p.stat().st_size for p in root.rglob("*") if p.is_file())
    (root / "native-summary.json").write_text(json.dumps(result, sort_keys=True) + "\n")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--seed", type=lambda value: int(value, 0), required=True)
    parser.add_argument("--node-bin", type=Path, required=True)
    parser.add_argument("--ctl-bin", type=Path, required=True)
    parser.add_argument("--dump-bin", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(run(args.seed, args.node_bin, args.ctl_bin, args.dump_bin), sort_keys=True))
