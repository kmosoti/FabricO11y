"""Validate and summarize the registered Stage 6 local-log CSV files.

Usage: python3 -B tools/bench/summarize_local_log_stage6.py target/stage6/RUN
"""

import csv
import json
import statistics
import sys
from pathlib import Path


EXPECTED_HEADER = [
    "phase",
    "event_index",
    "elapsed_ns",
    "events",
    "full_rejections",
    "file_bytes",
    "peak_rss_kib",
    "alloc_calls",
    "alloc_requested_bytes",
]


def rows(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as source:
        reader = csv.DictReader(source)
        if reader.fieldnames != EXPECTED_HEADER:
            raise ValueError(f"{path}: unexpected CSV header {reader.fieldnames}")
        result = list(reader)
    if not result:
        raise ValueError(f"{path}: empty CSV")
    return result


def one_phase(path: Path, phase: str) -> dict[str, str]:
    found = [row for row in rows(path) if row["phase"] == phase]
    if len(found) != 1:
        raise ValueError(f"{path}: expected exactly one {phase} row, got {len(found)}")
    return found[0]


def number(row: dict[str, str], key: str) -> int:
    value = row[key]
    if not value:
        raise ValueError(f"missing {key} in {row['phase']} row")
    return int(value)


def percentile_nearest_rank(values: list[int], numerator: int) -> int:
    if not values:
        raise ValueError("cannot compute percentile of zero samples")
    ordered = sorted(values)
    return ordered[(numerator * len(ordered) + 99) // 100 - 1]


def trial(directory: Path, name: str, count: int, allocation: bool = False) -> dict:
    write_path = directory / f"write-{name}.csv"
    write_rows = rows(write_path)
    appends = [row for row in write_rows if row["phase"] == "append"]
    ingests = [row for row in write_rows if row["phase"] == "ingest"]
    if len(appends) != count or len(ingests) != 1 or len(write_rows) != count + 1:
        raise ValueError(f"{write_path}: expected {count} append rows and one ingest row")
    if [number(row, "event_index") for row in appends] != list(range(1, count + 1)):
        raise ValueError(f"{write_path}: append indexes are not consecutive")
    append_ns = [number(row, "elapsed_ns") for row in appends]
    if any(value <= 0 for value in append_ns):
        raise ValueError(f"{write_path}: append timing is not positive")
    ingest = ingests[0]
    if number(ingest, "events") != count:
        raise ValueError(f"{write_path}: ingest count differs from registered count")
    pipeline_ns = number(ingest, "elapsed_ns")
    if count and pipeline_ns < sum(append_ns):
        raise ValueError(f"{write_path}: pipeline time is below summed append time")
    file_bytes = number(ingest, "file_bytes")
    actual_bytes = (directory / f"trial-{name}.fol2").stat().st_size if name in {"warmup", "1", "2", "3", "4", "5"} else (directory / f"{name}.fol2").stat().st_size
    if actual_bytes != file_bytes:
        raise ValueError(f"{write_path}: recorded file bytes differ from file size")
    verification = directory / f"verify-{name}.csv"
    verify_rows = rows(verification)
    if sorted(row["phase"] for row in verify_rows) != ["open", "replay"]:
        raise ValueError(f"{verification}: expected one open and one replay row")
    opened = one_phase(verification, "open")
    replayed = one_phase(verification, "replay")
    if any(number(row, "events") != count or number(row, "file_bytes") != file_bytes for row in (opened, replayed)):
        raise ValueError(f"{verification}: count or byte size differs from write")
    result = {
        "name": name,
        "events": count,
        "pipeline_ns": pipeline_ns,
        "events_per_second": count * 1_000_000_000 / pipeline_ns if count else None,
        "append_time_fraction": sum(append_ns) / pipeline_ns if pipeline_ns else None,
        "append_p50_ns": percentile_nearest_rank(append_ns, 50) if count else None,
        "append_p99_ns": percentile_nearest_rank(append_ns, 99) if count else None,
        "append_max_ns": max(append_ns) if count else None,
        "full_rejections": number(ingest, "full_rejections"),
        "file_bytes": file_bytes,
        "peak_rss_kib": int(ingest["peak_rss_kib"]) if ingest["peak_rss_kib"] else None,
        "open_ns": number(opened, "elapsed_ns"),
        "replay_verify_ns": number(replayed, "elapsed_ns"),
    }
    if allocation:
        result["alloc_calls"] = number(ingest, "alloc_calls")
        result["alloc_requested_bytes"] = number(ingest, "alloc_requested_bytes")
        if count and (result["alloc_calls"] <= 0 or result["alloc_requested_bytes"] <= 0):
            raise ValueError(f"{write_path}: allocation counters did not observe the pipeline")
    elif ingest["alloc_calls"] or ingest["alloc_requested_bytes"]:
        raise ValueError(f"{write_path}: ordinary run unexpectedly includes allocation counters")
    return result


def summarize(directory: Path) -> dict:
    measured = [trial(directory, str(i), 2000) for i in range(1, 6)]
    trial(directory, "warmup", 2000)
    recovery = [
        trial(directory, "recovery-0", 0),
        trial(directory, "recovery-500", 500),
    ]
    allocated = trial(directory, "alloc-2000", 2000, allocation=True)
    for item in measured:
        if item["full_rejections"] != 28:
            raise ValueError(f"trial {item['name']}: expected 28 full-buffer retries")
    rates = [item["events_per_second"] for item in measured]
    bytes_per_event = {item["file_bytes"] / item["events"] for item in measured}
    if len(bytes_per_event) != 1:
        raise ValueError("measured trials produced different bytes per event")
    return {
        "measured_trials": measured,
        "throughput_events_per_second": {
            "median": statistics.median(rates),
            "min": min(rates),
            "max": max(rates),
        },
        "bytes_per_event": bytes_per_event.pop(),
        "recovery_sizes": recovery,
        "allocation_run": allocated,
    }


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("usage: python3 -B tools/bench/summarize_local_log_stage6.py RUN_DIRECTORY", file=sys.stderr)
        raise SystemExit(2)
    try:
        print(json.dumps(summarize(Path(sys.argv[1])), indent=2))
    except (OSError, ValueError) as error:
        print(f"Stage 6 summary failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
