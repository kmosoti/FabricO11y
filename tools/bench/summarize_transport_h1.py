#!/usr/bin/env python3
"""Validate and summarize the preregistered first H1 packet-slot comparison."""

import csv
import json
import math
import random
import sys
from pathlib import Path

SEEDS = range(10)
VARIANTS = ("M0", "M1")
PRODUCERS = 32
BURSTS = 1000
BYTES = 65_536
PACKETS = 44
INJECTION = 1_760_000
DEADLINE = 1_770_000
MASK = (1 << 64) - 1


def fail(message):
    raise ValueError(message)


def rows(path, required):
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        if reader.fieldnames is None or len(reader.fieldnames) != len(set(reader.fieldnames)):
            fail(f"{path}: missing or duplicate CSV header")
        missing = set(required) - set(reader.fieldnames)
        if missing:
            fail(f"{path}: missing columns {sorted(missing)}")
        for line, row in enumerate(reader, 2):
            if None in row or any(value is None for value in row.values()):
                fail(f"{path}:{line}: malformed CSV width")
            yield line, row


def integer(row, key, where):
    try:
        return int(row[key])
    except ValueError as error:
        raise ValueError(f"{where}: invalid {key}={row[key]!r}") from error


def p99(values):
    if not values:
        fail("p99 requested for an empty sample")
    ordered = sorted(values)
    return ordered[math.ceil(0.99 * len(ordered)) - 1]


def release_ticks(seed):
    state = seed
    output = [0]
    for burst in range(1, BURSTS):
        state = (state + 0x9E3779B97F4A7C15) & MASK
        value = state
        value = ((value ^ (value >> 30)) * 0xBF58476D1CE4E5B9) & MASK
        value = ((value ^ (value >> 27)) * 0x94D049BB133111EB) & MASK
        value ^= value >> 31
        output.append(burst * 1760 + (value % 801) - 400)
    return output


def read_summary(path):
    required = (
        "seed variant producers bursts message_bytes packet_size data_delay_ticks "
        "control_delay_ticks bdp_packets sender_queue_cap_bytes "
        "switch_queue_cap_bytes receiver_queue_cap_bytes injection_interval_ticks "
        "drain_deadline_ticks nominal_burst_spacing_ticks jitter_magnitude_ticks "
        "control_message_bytes switch_egress_packets_per_tick sender_packets_per_tick "
        "offered_messages offered_bytes "
        "completed_injection_messages completed_injection_bytes "
        "completed_deadline_messages ack_count rejection_count loss_count "
        "control_messages control_bytes data_packets_sent data_packets_delivered "
        "modeled_commits sender_peak_bytes max_sender_queue_per_producer_bytes "
        "sender_retained_peak_bytes max_sender_retained_per_producer_bytes "
        "sender_byte_ticks switch_peak_bytes switch_byte_ticks receiver_peak_bytes "
        "receiver_byte_ticks total_queue_peak_bytes total_queue_byte_ticks "
        "p99_burst_peak_hotspot_bytes max_granted_outstanding_packets "
        "max_sender_window_outstanding_packets cap_overflow_count unacked_at_deadline"
    ).split()
    result = {}
    fixed = {
        "producers": PRODUCERS,
        "bursts": BURSTS,
        "message_bytes": BYTES,
        "packet_size": 1500,
        "data_delay_ticks": 4,
        "control_delay_ticks": 4,
        "bdp_packets": 9,
        "sender_queue_cap_bytes": 131_072,
        "switch_queue_cap_bytes": 768_000,
        "receiver_queue_cap_bytes": 0,
        "injection_interval_ticks": INJECTION,
        "drain_deadline_ticks": DEADLINE,
        "nominal_burst_spacing_ticks": 1760,
        "jitter_magnitude_ticks": 400,
        "control_message_bytes": 32,
        "switch_egress_packets_per_tick": 1,
        "sender_packets_per_tick": 1,
        "offered_messages": PRODUCERS * BURSTS,
        "offered_bytes": PRODUCERS * BURSTS * BYTES,
    }
    for line, row in rows(path, required):
        where = f"{path}:{line}"
        seed = integer(row, "seed", where)
        variant = row["variant"]
        key = (seed, variant)
        if seed not in SEEDS or variant not in VARIANTS or key in result:
            fail(f"{where}: unexpected or duplicate run {key}")
        for field, expected in fixed.items():
            if integer(row, field, where) != expected:
                fail(f"{where}: {field} differs from registration")
        item = {field: integer(row, field, where) for field in required if field not in {"seed", "variant"}}
        item["seed"] = seed
        item["variant"] = variant
        if item["control_bytes"] != 32 * item["control_messages"]:
            fail(f"{where}: control byte accounting")
        expected_controls = item["data_packets_delivered"] + (
            item["offered_messages"] if variant == "M0" else 2 * item["offered_messages"]
        )
        if item["control_messages"] != expected_controls:
            fail(f"{where}: control count differs from fixed protocol")
        if item["total_queue_byte_ticks"] != item["sender_byte_ticks"] + item["switch_byte_ticks"] + item["receiver_byte_ticks"]:
            fail(f"{where}: queued byte-time accounting")
        if item["receiver_peak_bytes"] != 0 or item["receiver_byte_ticks"] != 0:
            fail(f"{where}: network-limited receiver has a nonzero queue")
        if item["data_packets_sent"] != PRODUCERS * BURSTS * PACKETS:
            fail(f"{where}: data packet count")
        if item["data_packets_delivered"] != item["data_packets_sent"]:
            fail(f"{where}: sent/delivered conservation")
        if item["max_sender_queue_per_producer_bytes"] > 131_072:
            fail(f"{where}: sender unsent cap breached")
        if item["max_sender_retained_per_producer_bytes"] > 131_072:
            fail(f"{where}: sender retained cap breached")
        if item["switch_peak_bytes"] > 768_000:
            fail(f"{where}: switch cap breached")
        if variant == "M1" and item["max_granted_outstanding_packets"] > 9:
            fail(f"{where}: global credit budget breached")
        if variant == "M0" and item["max_sender_window_outstanding_packets"] > 9:
            fail(f"{where}: sender window breached")
        result[key] = item
    if len(result) != len(SEEDS) * len(VARIANTS):
        fail(f"{path}: expected 20 seed/variant rows, got {len(result)}")
    return result


def read_messages(path, summary):
    required = (
        "seed variant producer sequence burst bytes release_tick first_send_tick "
        "complete_tick modeled_commit_tick ack_tick"
    ).split()
    release = {seed: release_ticks(seed) for seed in SEEDS}
    current_key = None
    seen_keys = set()
    state = None
    results = {}

    def finish():
        if current_key is None:
            return
        if state["count"] != PRODUCERS * BURSTS or not all(state["seen"]):
            fail(f"{path}: incomplete message IDs for {current_key}")
        row = summary[current_key]
        checks = {
            "completed_injection_messages": state["injection_messages"],
            "completed_injection_bytes": state["injection_bytes"],
            "completed_deadline_messages": state["deadline_messages"],
            "ack_count": state["acked"],
            "modeled_commits": state["committed"],
        }
        for field, expected in checks.items():
            if row[field] != expected:
                fail(f"{path}: {current_key} {field} disagrees with message records")
        results[current_key] = {
            "producer_p99_ticks": [p99(x) for x in state["latencies"]],
            "producer_max_ticks": [max(x) for x in state["latencies"]],
            "producer_injection_bytes": state["producer_injection_bytes"],
            "burst_last_complete": state["burst_last_complete"],
        }

    for line, row in rows(path, required):
        where = f"{path}:{line}"
        key = (integer(row, "seed", where), row["variant"])
        if key not in summary:
            fail(f"{where}: unknown seed/variant {key}")
        if key != current_key:
            finish()
            if key in seen_keys:
                fail(f"{where}: run {key} appears in multiple blocks")
            seen_keys.add(key)
            current_key = key
            state = {
                "count": 0,
                "seen": bytearray(PRODUCERS * BURSTS),
                "injection_messages": 0,
                "injection_bytes": 0,
                "deadline_messages": 0,
                "acked": 0,
                "committed": 0,
                "latencies": [[] for _ in range(PRODUCERS)],
                "producer_injection_bytes": [0] * PRODUCERS,
                "burst_last_complete": [0] * BURSTS,
            }
        producer = integer(row, "producer", where)
        sequence = integer(row, "sequence", where)
        burst = integer(row, "burst", where)
        size = integer(row, "bytes", where)
        start = integer(row, "release_tick", where)
        first = integer(row, "first_send_tick", where)
        complete = integer(row, "complete_tick", where)
        commit = integer(row, "modeled_commit_tick", where)
        ack = integer(row, "ack_tick", where)
        if not (0 <= producer < PRODUCERS and 0 <= burst < BURSTS):
            fail(f"{where}: invalid producer/burst")
        if sequence != burst + 1 or size != BYTES or start != release[key[0]][burst]:
            fail(f"{where}: trace differs from registration")
        index = producer * BURSTS + burst
        if state["seen"][index]:
            fail(f"{where}: duplicate identity")
        state["seen"][index] = 1
        state["count"] += 1
        if first < start or complete < first + 4 + PACKETS or commit != complete or ack != commit + 4:
            fail(f"{where}: impossible timing or early durable ACK")
        if key[1] == "M1" and first < start + 8:
            fail(f"{where}: scheduled data sent before request and credit RTT")
        if complete > DEADLINE or ack > DEADLINE:
            fail(f"{where}: completion or ACK after common deadline")
        if complete < INJECTION:
            state["injection_messages"] += 1
            state["injection_bytes"] += BYTES
            state["producer_injection_bytes"][producer] += BYTES
        state["deadline_messages"] += 1
        state["acked"] += 1
        state["committed"] += 1
        state["latencies"][producer].append(complete - start)
        state["burst_last_complete"][burst] = max(state["burst_last_complete"][burst], complete)
    finish()
    if len(results) != len(summary):
        fail(f"{path}: expected {len(summary)} run blocks, got {len(results)}")
    return results, release


def read_bursts(path, summary, messages, release):
    required = "seed variant burst release_tick last_complete_tick peak_hotspot_bytes".split()
    peaks = {key: [None] * BURSTS for key in summary}
    for line, row in rows(path, required):
        where = f"{path}:{line}"
        key = (integer(row, "seed", where), row["variant"])
        if key not in peaks:
            fail(f"{where}: unknown run {key}")
        burst = integer(row, "burst", where)
        if not 0 <= burst < BURSTS or peaks[key][burst] is not None:
            fail(f"{where}: invalid or duplicate burst")
        start = integer(row, "release_tick", where)
        end = integer(row, "last_complete_tick", where)
        peak = integer(row, "peak_hotspot_bytes", where)
        if start != release[key[0]][burst] or end != messages[key]["burst_last_complete"][burst]:
            fail(f"{where}: burst interval differs from message records")
        if peak < 0 or peak > summary[key]["switch_peak_bytes"]:
            fail(f"{where}: invalid hot-spot peak")
        peaks[key][burst] = peak
    for key, values in peaks.items():
        if any(value is None for value in values):
            fail(f"{path}: incomplete burst rows for {key}")
        if p99(values) != summary[key]["p99_burst_peak_hotspot_bytes"]:
            fail(f"{path}: p99 burst peak mismatch for {key}")
    return peaks


def analyze(summary, messages):
    paired = []
    gate_failures = []
    for seed in SEEDS:
        m0, m1 = summary[(seed, "M0")], summary[(seed, "M1")]
        zero_fields = ("rejection_count", "loss_count", "cap_overflow_count", "unacked_at_deadline")
        for variant, row in (("M0", m0), ("M1", m1)):
            for field in zero_fields:
                if row[field] != 0:
                    gate_failures.append(f"seed {seed} {variant}: {field}={row[field]}")
            if row["completed_deadline_messages"] != PRODUCERS * BURSTS:
                gate_failures.append(f"seed {seed} {variant}: incomplete by deadline")
            if row["completed_injection_bytes"] < math.ceil(0.95 * row["offered_bytes"]):
                gate_failures.append(f"seed {seed} {variant}: injection completion below 95%")
        achieved0 = m0["completed_injection_bytes"]
        achieved1 = m1["completed_injection_bytes"]
        if achieved0 == 0 or abs(achieved1 - achieved0) / achieved0 > 0.05:
            gate_failures.append(f"seed {seed}: achieved goodput mismatch")
        p0 = m0["p99_burst_peak_hotspot_bytes"]
        p1 = m1["p99_burst_peak_hotspot_bytes"]
        reduction = None if p0 == 0 else (p0 - p1) / p0
        if reduction is None:
            gate_failures.append(f"seed {seed}: zero control hot-spot occupancy")
        for producer in range(PRODUCERS):
            a = messages[(seed, "M0")]
            b = messages[(seed, "M1")]
            if b["producer_p99_ticks"][producer] > 1.05 * a["producer_p99_ticks"][producer]:
                gate_failures.append(f"seed {seed} producer {producer}: p99 latency guardrail")
            if b["producer_max_ticks"][producer] > 2 * a["producer_max_ticks"][producer]:
                gate_failures.append(f"seed {seed} producer {producer}: max latency guardrail")
        paired.append({
            "seed": seed,
            "m0_p99_hotspot_bytes": p0,
            "m1_p99_hotspot_bytes": p1,
            "relative_reduction": reduction,
            "m0_completed_injection_bytes": achieved0,
            "m1_completed_injection_bytes": achieved1,
            "m0_producer_injection_bytes": messages[(seed, "M0")]["producer_injection_bytes"],
            "m1_producer_injection_bytes": messages[(seed, "M1")]["producer_injection_bytes"],
            "m0_producer_goodput_bytes_per_tick": [value / INJECTION for value in messages[(seed, "M0")]["producer_injection_bytes"]],
            "m1_producer_goodput_bytes_per_tick": [value / INJECTION for value in messages[(seed, "M1")]["producer_injection_bytes"]],
            "m0_producer_p99_network_ticks": messages[(seed, "M0")]["producer_p99_ticks"],
            "m1_producer_p99_network_ticks": messages[(seed, "M1")]["producer_p99_ticks"],
            "m0_producer_max_network_ticks": messages[(seed, "M0")]["producer_max_ticks"],
            "m1_producer_max_network_ticks": messages[(seed, "M1")]["producer_max_ticks"],
            "m0_sender_peak_bytes": m0["sender_peak_bytes"],
            "m1_sender_peak_bytes": m1["sender_peak_bytes"],
            "m0_sender_retained_peak_bytes": m0["sender_retained_peak_bytes"],
            "m1_sender_retained_peak_bytes": m1["sender_retained_peak_bytes"],
            "m0_switch_peak_bytes": m0["switch_peak_bytes"],
            "m1_switch_peak_bytes": m1["switch_peak_bytes"],
            "m0_total_queue_byte_ticks": m0["total_queue_byte_ticks"],
            "m1_total_queue_byte_ticks": m1["total_queue_byte_ticks"],
            "m0_control_bytes": m0["control_bytes"],
            "m1_control_bytes": m1["control_bytes"],
        })
    reductions = [item["relative_reduction"] for item in paired]
    bootstrap = None
    mean = None
    if all(value is not None for value in reductions):
        mean = sum(reductions) / len(reductions)
        rng = random.Random(42)
        samples = sorted(sum(reductions[rng.randrange(len(reductions))] for _ in reductions) / len(reductions) for _ in range(10_000))
        bootstrap = [samples[249], samples[9749]]
    advances = not gate_failures and mean is not None and mean >= 0.10 and bootstrap[0] > 0
    return {
        "interpretation": "network-limited packet-slot model; no real durable transport result",
        "paired_seeds": paired,
        "mean_relative_reduction": mean,
        "paired_bootstrap_95_percent": bootstrap,
        "gate_failures": gate_failures,
        "preliminary_h1_advances": advances,
    }


def main():
    if len(sys.argv) != 2:
        fail("usage: python3 -B tools/bench/summarize_transport_h1.py OUTPUT_DIR")
    root = Path(sys.argv[1])
    summary = read_summary(root / "summary.csv")
    messages, release = read_messages(root / "messages.csv", summary)
    read_bursts(root / "bursts.csv", summary, messages, release)
    print(json.dumps(analyze(summary, messages), indent=2, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        sys.exit(1)
