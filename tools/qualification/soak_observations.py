#!/usr/bin/env python3
"""Supplemental retry-aware observations for a completed native soak.

This report is descriptive only: it does not call or modify the registered
soak oracle or gates. Event input is read once, line by line.
"""

import argparse
import hashlib
import json
import math
from collections import Counter, defaultdict
from pathlib import Path

WARMUP_S = 60
WINDOW_S = 600
WINDOWS = 9
IDENTITIES = 100
TRIAL_SECONDS = WARMUP_S + WINDOW_S * WINDOWS


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def percentile(values, fraction):
    """Nearest-rank percentile (ceil(p*n), one-based), or None if empty."""
    if not values:
        return None
    ordered = sorted(values)
    return ordered[max(0, math.ceil(fraction * len(ordered)) - 1)]


def _window(timestamp, began_ns):
    offset_s = (timestamp - began_ns) / 1_000_000_000 - WARMUP_S
    if offset_s < 0 or offset_s >= WINDOW_S * WINDOWS:
        return None
    return int(offset_s // WINDOW_S)


def _rate(numerator, denominator):
    return numerator / denominator if denominator else None


def analyze(events_path, sim_summary_path, soak_summary_path):
    sim = json.loads(Path(sim_summary_path).read_text())
    soak = json.loads(Path(soak_summary_path).read_text())
    if (soak.get("smoke") is not False or soak.get("protocol_revision") != "R2"
            or soak.get("seconds") != TRIAL_SECONDS or soak.get("identities") != IDENTITIES):
        raise ValueError("soak summary is not a full R2 trial with the registered 60s+9x600s profile")
    if sim.get("seconds") != TRIAL_SECONDS or sim.get("identities") != IDENTITIES:
        raise ValueError("simulator summary does not match the registered full-trial profile")
    if sim.get("seed") != soak.get("seed"):
        raise ValueError("simulator and soak summaries have different seeds")
    began_ns = sim["began_unix_ns"]
    created = {}
    last_created = {}
    acked = {}
    last_ack_seq = {}
    attempts = [Counter() for _ in range(WINDOWS)]
    attempt_latency_ms = [[] for _ in range(WINDOWS)]
    observed_attempts = 0

    with open(events_path, encoding="utf-8") as source:
        for line_no, line in enumerate(source, 1):
            try:
                event = json.loads(line)
                kind = event["e"]
                if kind == "created":
                    identity = event["id"]
                    seq = event["seq"]
                    timestamp = event["t"]
                    key = (identity, seq)
                    if key in created:
                        raise ValueError(f"duplicate created identity {key}")
                    previous = last_created.get(identity)
                    if previous is not None and (seq <= previous[0] or timestamp < previous[1]):
                        raise ValueError(f"nonmonotonic creation for identity {identity}")
                    last_created[identity] = (seq, timestamp)
                    created[key] = timestamp
                elif kind == "attempt":
                    observed_attempts += 1
                    key = (event["id"], event["seq"])
                    if key not in created:
                        raise ValueError(f"attempt without created identity {key}")
                    if event["created"] != created[key]:
                        raise ValueError(f"created timestamp mismatch for {key}")
                    if event["start"] < created[key]:
                        raise ValueError(f"attempt starts before creation for {key}")
                    if event["start"] > event["end"]:
                        raise ValueError(f"attempt end precedes start for {key}")
                    category = event["kind"]
                    if category not in {"ack", "conflict", "gap", "too_large", "bad_request",
                                        "unauthorized", "unavailable", "no_response"}:
                        raise ValueError(f"unknown attempt kind {category!r}")
                    index = _window(event["start"], began_ns)
                    if index is not None:
                        attempts[index][category] += 1
                        attempt_latency_ms[index].append((event["end"] - event["start"]) / 1e6)
                    if category == "ack":
                        if key in acked:
                            raise ValueError(f"duplicate ACK identity {key}")
                        if event["end"] < created[key]:
                            raise ValueError(f"ACK precedes creation for {key}")
                        previous_ack = last_ack_seq.get(key[0])
                        if previous_ack is not None and key[1] <= previous_ack:
                            raise ValueError(f"nonmonotonic ACK sequence for identity {key[0]}")
                        last_ack_seq[key[0]] = key[1]
                        acked[key] = event["end"]
                elif kind == "applied":
                    if any(not isinstance(event.get(field), int) for field in ("id", "rev", "t")):
                        raise ValueError("malformed applied event")
                else:
                    raise ValueError(f"unknown event type {kind!r}")
            except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
                raise ValueError(f"{events_path}:{line_no}: {error}") from error

    if len(created) != soak.get("batches_created"):
        raise ValueError("event created count disagrees with registered soak summary")
    if len(acked) != soak.get("batches_acked"):
        raise ValueError("event ACK-kind count disagrees with registered soak summary")

    created_by_window = Counter()
    acked_by_window = Counter()
    first_ack_delay_ms = defaultdict(list)
    for key, timestamp in created.items():
        index = _window(timestamp, began_ns)
        if index is not None:
            created_by_window[index] += 1
            if key in acked:
                acked_by_window[index] += 1
                first_ack_delay_ms[index].append((acked[key] - timestamp) / 1e6)

    windows = []
    for i in range(WINDOWS):
        counted = attempts[i]
        total = sum(counted.values())
        latencies = attempt_latency_ms[i]
        delays = first_ack_delay_ms[i]
        windows.append({
            "from_s_after_trial_start": WARMUP_S + i * WINDOW_S,
            "created_batches": created_by_window[i],
            "created_batches_with_ack_kind_by_event_log_end": acked_by_window[i],
            "batch_completion_rate_by_creation_cohort": _rate(acked_by_window[i], created_by_window[i]),
            "attempts_by_outcome": dict(sorted(counted.items())),
            "attempt_count": total,
            "ack_kind_attempt_rate": _rate(counted["ack"], total),
            "http_503_unavailable_kind_attempt_rate": _rate(counted["unavailable"], total),
            "attempt_latency_ms_end_minus_start_p50": percentile(latencies, 0.50),
            "attempt_latency_ms_end_minus_start_p99": percentile(latencies, 0.99),
            "created_to_first_ack_ms_created_t_to_first_ack_end_p50": percentile(delays, 0.50),
            "created_to_first_ack_ms_created_t_to_first_ack_end_p99": percentile(delays, 0.99),
            "first_ack_batch_count": len(delays),
        })

    raw_companion = soak.get("companion", {}).get("resources")
    return {
        "report_kind": "supplemental_observations_not_a_gate",
        "registered_oracle_or_gates_modified": False,
        "interpretation": {
            "attempt_rates": "per-send-attempt outcome fractions; retries count as additional attempts; ack is the simulator's ACK-kind event (committed_through is absent from events.jsonl)",
            "batch_completion_rate": "distinct created identities with an ACK-kind event by end of event log, grouped by creation window; this supplemental event file does not carry committed_through",
            "attempt_latency": "one send attempt end minus start; excludes retry waiting",
            "created_to_first_ack": "first ACK attempt end minus original created timestamp; includes retry waiting",
            "window_bounds": "attempts use attempt start; created cohorts use created t; 60 s warmup then nine 600 s windows",
        },
        "input_sha256": {
            "events_jsonl": sha256(events_path),
            "sim_summary_json": sha256(sim_summary_path),
            "soak_summary_json": sha256(soak_summary_path),
        },
        "observed_totals": {
            "distinct_created_batches": len(created),
            "distinct_batches_with_ack_event": len(acked),
            "all_event_attempts": observed_attempts,
            "registered_summary_counts_match_event_stream": True,
        },
        "retry_aware_windows": windows,
        "registered_soak_summary_metrics_verbatim": {
            "server_cpu_seconds_by_window": [w.get("server_cpu_s") for w in soak.get("windows", [])],
            "server_rss_kib_p50_by_window": [w.get("rss_kib_p50") for w in soak.get("windows", [])],
            "server_rss_kib_max_by_window": [w.get("rss_kib_max") for w in soak.get("windows", [])],
            "server_vmhwm_kib": soak.get("server_vmhwm_kib"),
            "query_latency_seconds_p50_p99": [soak.get("query_p50_s"), soak.get("query_p99_s")],
            "registered_ack_attempt_latency_p99_ms_by_window": [w.get("ack_ms_p99") for w in soak.get("windows", [])],
            "registered_oracle_passed": soak.get("gates", {}).get("oracle"),
            "registered_gates_verbatim": soak.get("gates"),
            "server_plus_companion_cgroup_cpu_stat_raw": raw_companion.get("cpu.stat") if raw_companion else None,
            "server_plus_companion_cgroup_memory_events_raw": raw_companion.get("memory.events") if raw_companion else None,
            "server_plus_companion_cgroup_memory_peak_bytes": raw_companion.get("memory.peak") if raw_companion else None,
            "server_plus_companion_cgroup_swap_current_bytes": raw_companion.get("memory.swap.current") if raw_companion else None,
            "server_plus_companion_cgroup_events_raw": raw_companion.get("cgroup.events") if raw_companion else None,
            "server_plus_companion_cgroup_io_stat_raw": raw_companion.get("io.stat") if raw_companion else None,
            "server_plus_companion_cgroup_pids_peak": raw_companion.get("pids.peak") if raw_companion else None,
        },
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--events", required=True, type=Path)
    parser.add_argument("--sim-summary", required=True, type=Path)
    parser.add_argument("--soak-summary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    output = args.output.resolve(strict=False)
    inputs = {path.resolve(strict=True) for path in
              (args.events, args.sim_summary, args.soak_summary)}
    if output in inputs:
        raise ValueError("report destination must be separate from every input")
    report = analyze(args.events, args.sim_summary, args.soak_summary)
    payload = json.dumps(report, sort_keys=True, indent=2) + "\n"
    with output.open("x", encoding="utf-8") as destination:
        destination.write(payload)
    print(json.dumps({"output": str(output), "sha256": sha256(output)}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
