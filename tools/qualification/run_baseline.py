"""Execute the preregistered fresh FOL2 baseline under the alpha watchdog."""

import argparse
from collections import Counter
import csv
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

from runner import owned_root, run

ROOT = Path(__file__).resolve().parents[2]
TARGET = ROOT / "target"
SEEDS = (2703204353, 2703204354, 2703204355)
COUNT = 200


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def percentile(values: list[int], numerator: int, denominator: int) -> int:
    ordered = sorted(values)
    return ordered[max(0, (len(ordered) * numerator + denominator - 1) // denominator - 1)]


def measure(probe: Path, prefix: str, label: str, seed: int) -> dict:
    path = TARGET / f"{prefix}-{label}"
    if path.exists():
        raise ValueError(f"trial output already exists: {path}")
    owned_root(path)
    write = run(path, [str(probe), "write", "trial.fol2", str(seed), str(COUNT)],
                60, 16 * 1024**2, 2 * 1024**2)
    if not write["passed"]:
        raise RuntimeError(f"write failed: {label}: {write}")
    shutil.copyfile(path / "command-output.bin", path / "write.csv")
    verify = run(path, [str(probe), "verify", "trial.fol2", str(seed), str(COUNT)],
                 60, 16 * 1024**2, 2 * 1024**2)
    if not verify["passed"]:
        raise RuntimeError(f"verify failed: {label}: {verify}")
    shutil.copyfile(path / "command-output.bin", path / "verify.csv")
    with (path / "write.csv").open(newline="") as source:
        rows = list(csv.DictReader(source))
    with (path / "verify.csv").open(newline="") as source:
        verified = list(csv.DictReader(source))
    appends = [int(row["elapsed_ns"]) for row in rows if row["phase"] == "append"]
    ingest = [row for row in rows if row["phase"] == "ingest"]
    opens = [row for row in verified if row["phase"] == "open"]
    replays = [row for row in verified if row["phase"] == "replay"]
    if (len(appends) != COUNT or len(ingest) != 1 or len(opens) != 1 or len(replays) != 1
            or int(ingest[0]["events"]) != COUNT
            or int(opens[0]["events"]) != COUNT
            or int(replays[0]["events"]) != COUNT):
        raise RuntimeError(f"unexpected probe schema or count: {label}")
    size = (path / "trial.fol2").stat().st_size
    if size != int(ingest[0]["file_bytes"]) or size != int(replays[0]["file_bytes"]):
        raise RuntimeError(f"file size mismatch: {label}")
    return {"label": label, "seed": seed, "records": COUNT,
            "pipeline_events_per_s": round(COUNT * 1e9 / int(ingest[0]["elapsed_ns"]), 2),
            "append_p50_us": round(percentile(appends, 50, 100) / 1000, 2),
            "append_p99_us": round(percentile(appends, 99, 100) / 1000, 2),
            "append_histogram_1ms": dict(sorted(Counter(str(value // 1_000_000)
                                                       for value in appends).items(),
                                                 key=lambda item: int(item[0]))),
            "file_bytes": size, "bytes_per_event": size / COUNT,
            "open_ms": round(int(opens[0]["elapsed_ns"]) / 1e6, 3),
            "replay_ms": round(int(replays[0]["elapsed_ns"]) / 1e6, 3),
            "peak_rss_kib_probe": int(ingest[0]["peak_rss_kib"]),
            "full_rejections": int(ingest[0]["full_rejections"]),
            "write_runner": write, "verify_runner": verify,
            "write_csv_sha256": digest(path / "write.csv"),
            "verify_csv_sha256": digest(path / "verify.csv"),
            "log_sha256": digest(path / "trial.fol2")}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--prefix", default="alpha-phase0-baseline-01")
    args = parser.parse_args()
    prefix = args.prefix
    if not prefix.startswith("alpha-") or "/" in prefix or prefix in (".", ".."):
        parser.error("prefix must be an alpha-* name without separators")
    probe = TARGET / "release" / "examples" / "local_log_probe"
    if not probe.is_file():
        parser.error("build the release local_log_probe first")
    record_dir = TARGET / f"{prefix}-record"
    if record_dir.exists():
        parser.error("record directory exists; choose a fresh prefix")
    owned_root(record_dir)
    rows = [measure(probe, prefix, "warmup", SEEDS[0])]
    for index, seed in enumerate(SEEDS, 1):
        rows.append(measure(probe, prefix, f"trial-{index}", seed))
    files = ["Cargo.toml", "Cargo.lock", "src/lib.rs", "src/generator.rs",
             "src/buffer.rs", "src/log.rs", "examples/local_log_probe.rs",
             "tools/qualification/runner.py", "tools/qualification/run_baseline.py"]
    result = {"kind": "current FOL2 baseline", "warmup": rows[0], "trials": rows[1:],
              "source_sha256": {name: digest(ROOT / name) for name in files},
              "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
              "uname": subprocess.check_output(["uname", "-a"], text=True).strip(),
              "git_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()}
    (record_dir / "summary.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"record": str(record_dir), "trial_count": len(rows) - 1,
                      "trials_passed": all(t["write_runner"]["passed"] and t["verify_runner"]["passed"] for t in rows)},
                     sort_keys=True))


if __name__ == "__main__":
    main()
