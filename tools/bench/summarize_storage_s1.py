#!/usr/bin/env python3
"""Validate the fixed S1 CSV grid and summarize its registered measurements."""

import csv
import json
import math
import statistics
import sys
from collections import defaultdict
from pathlib import Path


WORKLOADS = ("gauge", "clustered_logs", "shuffled_logs", "mixed")
SEEDS = (42, 43, 44)
MODES = ("scan", "pruned")
EVENTS = 2048
BLOCKS = 32
TRIALS = 5
QUERIES = 128
BUILD_FIELDS = ("workload", "seed", "events", "write_ns", "replay_ns", "build_ns", "raw_bytes", "summary_bytes", "peak_rss_kib")
QUERY_FIELDS = ("workload", "seed", "trial", "query", "case", "mode", "elapsed_ns", "scanned_events", "skipped_blocks", "matches", "expected_matches", "equal")


def fail(message):
    raise ValueError(message)


def csv_rows(path, fields):
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        if reader.fieldnames != list(fields):
            fail(f"{path}: CSV header differs from fixed schema")
        for row in reader:
            if None in row or any(value is None for value in row.values()):
                fail(f"{path}:{reader.line_num}: malformed CSV width")
            yield f"{path}:{reader.line_num}", row


def integer(raw, field, where):
    value = raw[field]
    if not value.isascii() or not value.isdecimal():
        fail(f"{where}: invalid nonnegative integer {field}={value!r}")
    return int(value)


def nearest_rank(values, fraction):
    ordered = sorted(values)
    if not ordered:
        fail("empty latency sample")
    return ordered[math.ceil(fraction * len(ordered)) - 1]


def analyze(directory):
    directory = Path(directory)
    builds = {}
    for where, raw in csv_rows(directory / "builds.csv", BUILD_FIELDS):
        workload = raw["workload"]
        seed = integer(raw, "seed", where)
        key = (workload, seed)
        if workload not in WORKLOADS or seed not in SEEDS or key in builds:
            fail(f"{where}: unknown or duplicate build {key}")
        item = {field: integer(raw, field, where) for field in BUILD_FIELDS[1:]}
        if item["events"] != EVENTS or item["summary_bytes"] != BLOCKS * 528:
            fail(f"{where}: event or logical summary byte count differs from protocol")
        if item["raw_bytes"] == 0 or item["peak_rss_kib"] == 0:
            fail(f"{where}: zero raw bytes or peak RSS")
        builds[key] = item
    if len(builds) != len(WORKLOADS) * len(SEEDS):
        fail(f"builds.csv: incomplete grid ({len(builds)} builds)")

    cells = {}
    expected_by_query = {}
    logical_by_query = {}
    latencies = defaultdict(list)
    case_rows = defaultdict(lambda: [0, 0])
    for where, raw in csv_rows(directory / "queries.csv", QUERY_FIELDS):
        workload = raw["workload"]
        seed = integer(raw, "seed", where)
        trial = integer(raw, "trial", where)
        query = integer(raw, "query", where)
        case = integer(raw, "case", where)
        mode = raw["mode"]
        key = (workload, seed, trial, query, mode)
        if (workload, seed) not in builds or trial >= TRIALS or query >= QUERIES or case != query % 8 or mode not in MODES or key in cells:
            fail(f"{where}: unknown, duplicate, or misclassified query cell {key}")
        item = {field: integer(raw, field, where) for field in QUERY_FIELDS[6:]}
        if item["equal"] != 1 or item["matches"] != item["expected_matches"]:
            fail(f"{where}: result equality failed")
        scanned = item["scanned_events"]
        skipped = item["skipped_blocks"]
        if item["matches"] > scanned or skipped > BLOCKS:
            fail(f"{where}: impossible result or skipped-block count")
        if mode == "scan":
            if scanned != EVENTS or skipped != 0:
                fail(f"{where}: full-scan accounting mismatch")
        elif scanned + skipped * 64 != EVENTS:
            fail(f"{where}: pruned block accounting mismatch")
        identity = (workload, seed, query)
        previous = expected_by_query.setdefault(identity, item["expected_matches"])
        if previous != item["expected_matches"]:
            fail(f"{where}: expected matches vary across modes or trials")
        logical = (scanned, skipped, item["matches"])
        previous_logical = logical_by_query.setdefault((identity, mode), logical)
        if previous_logical != logical:
            fail(f"{where}: logical work varies across trials")
        cells[key] = item
        latencies[(workload, seed, trial, mode)].append(item["elapsed_ns"])
        case_rows[(workload, seed, case, mode)][0] += scanned
        case_rows[(workload, seed, case, mode)][1] += 1
    required = len(WORKLOADS) * len(SEEDS) * TRIALS * QUERIES * len(MODES)
    if len(cells) != required:
        fail(f"queries.csv: incomplete grid ({len(cells)} / {required} rows)")

    workloads = {}
    gate_failures = []
    for workload in WORKLOADS:
        seeds = {}
        for seed in SEEDS:
            latency = {}
            for mode in MODES:
                trial_ranks = []
                for trial in range(TRIALS):
                    samples = latencies[(workload, seed, trial, mode)]
                    if len(samples) != QUERIES:
                        fail(f"{workload}/{seed}/{trial}/{mode}: incomplete latency sample")
                    trial_ranks.append({"trial": trial, "p50_ns": nearest_rank(samples, .50), "p99_ns": nearest_rank(samples, .99)})
                latency[mode] = {
                    "trials": trial_ranks,
                    "median_trial_p50_ns": statistics.median(rank["p50_ns"] for rank in trial_ranks),
                    "median_trial_p99_ns": statistics.median(rank["p99_ns"] for rank in trial_ranks),
                }
            cases = {}
            for case in range(8):
                scan_total, scan_count = case_rows[(workload, seed, case, "scan")]
                pruned_total, pruned_count = case_rows[(workload, seed, case, "pruned")]
                if scan_count != 16 * TRIALS or pruned_count != 16 * TRIALS:
                    fail(f"{workload}/{seed}/case {case}: incomplete case samples")
                ratio = pruned_total / scan_total
                cases[str(case)] = {"scan_rows": scan_total, "pruned_rows": pruned_total, "pruned_to_scan_ratio": ratio, "row_reduction": 1 - ratio}
            if workload == "clustered_logs":
                for case in (1, 4):
                    if cases[str(case)]["pruned_to_scan_ratio"] > .5:
                        gate_failures.append(f"clustered_logs seed {seed} case {case}: ratio {cases[str(case)]['pruned_to_scan_ratio']:.6f} exceeds 0.5")
            seeds[str(seed)] = {"build": builds[(workload, seed)], "latency": latency, "cases": cases}
        workloads[workload] = seeds
    return {
        "status": "inconclusive" if gate_failures else "advances",
        "correctness": "valid CSV; probe and independent contract tests must also pass",
        "gate_failures": gate_failures,
        "workloads": workloads,
        "limitations": ["Candidate rows are logical work, not physical bytes read.", "Warm-cache mixed-query latency is descriptive and selects no latency winner.", "Logical summary bytes exclude container and allocator overhead."],
    }


def main():
    if len(sys.argv) != 2:
        fail("usage: python3 -B tools/bench/summarize_storage_s1.py OUTPUT_DIR")
    print(json.dumps(analyze(sys.argv[1]), indent=2, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, UnicodeError, ValueError, csv.Error) as error:
        print(error, file=sys.stderr)
        sys.exit(1)
