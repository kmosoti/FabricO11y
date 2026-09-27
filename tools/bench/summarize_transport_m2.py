#!/usr/bin/env python3
"""Independently validate and summarize the registered M1/M2 CSV comparison."""

import csv
import gzip
import json
import math
import random
import sys
from pathlib import Path


SEEDS = range(10)
VARIANTS = ("M1", "M2")
MASK = (1 << 64) - 1
PACKET_SIZE = 1_500
DATA_DELAY = 4
CONTROL_DELAY = 4
BDP_PACKETS = 9
CONTROL_BYTES = 32
SWITCH_CAP = 768_000
H1_MESSAGES = (
    Path(__file__).resolve().parents[2]
    / "docs/experiments/ablation/data/receiver-credit-h1-run-01/messages.csv.gz"
)

PROFILES = {
    "small": {
        "producers": 8,
        "bursts": 1_000,
        "message_bytes": 148,
        "packets": 1,
        "spacing": 10,
        "jitter": 3,
        "injection": 10_000,
        "deadline": 11_000,
        "sender_cap": 592,
    },
    "incast": {
        "producers": 32,
        "bursts": 1_000,
        "message_bytes": 65_536,
        "packets": 44,
        "spacing": 1_760,
        "jitter": 400,
        "injection": 1_760_000,
        "deadline": 1_770_000,
        "sender_cap": 131_072,
    },
}

SUMMARY_FIELDS = (
    "profile seed variant producers bursts message_bytes packet_size data_delay_ticks "
    "control_delay_ticks bdp_packets sender_queue_cap_bytes switch_queue_cap_bytes "
    "receiver_queue_cap_bytes injection_interval_ticks drain_deadline_ticks "
    "nominal_burst_spacing_ticks jitter_magnitude_ticks control_message_bytes "
    "switch_egress_packets_per_tick sender_packets_per_tick offered_messages offered_bytes "
    "completed_injection_messages completed_injection_bytes completed_deadline_messages "
    "ack_count rejection_count loss_count control_messages control_bytes data_packets_sent "
    "data_packets_delivered modeled_commits sender_peak_bytes "
    "max_sender_queue_per_producer_bytes sender_retained_peak_bytes "
    "max_sender_retained_per_producer_bytes sender_byte_ticks switch_peak_bytes "
    "switch_byte_ticks receiver_peak_bytes receiver_byte_ticks total_queue_peak_bytes "
    "total_queue_byte_ticks p99_burst_peak_hotspot_bytes max_granted_outstanding_packets "
    "max_sender_window_outstanding_packets cap_overflow_count unacked_at_deadline "
    "unscheduled_prefix_packets scheduled_packets_sent unscheduled_packets_sent "
    "embedded_metadata_bytes data_wire_bytes_sent"
).split()
MESSAGE_FIELDS = (
    "profile seed variant producer sequence burst bytes release_tick first_send_tick "
    "complete_tick modeled_commit_tick ack_tick"
).split()
BURST_FIELDS = (
    "profile seed variant burst release_tick last_complete_tick peak_hotspot_bytes"
).split()
H1_MESSAGE_FIELDS = MESSAGE_FIELDS[1:]


def fail(message):
    raise ValueError(message)


def rows(path, required, opener=None):
    if opener is None:
        opener = lambda name: name.open(newline="", encoding="utf-8")
    with opener(path) as handle:
        reader = csv.DictReader(handle)
        if reader.fieldnames is None or reader.fieldnames[0] != required[0]:
            fail(f"{path}: missing header or first column is not {required[0]}")
        if len(reader.fieldnames) != len(set(reader.fieldnames)):
            fail(f"{path}: duplicate CSV header")
        missing = set(required) - set(reader.fieldnames)
        extra = set(reader.fieldnames) - set(required)
        if missing or extra:
            fail(f"{path}: schema mismatch, missing={sorted(missing)}, extra={sorted(extra)}")
        for row in reader:
            if None in row or any(value is None for value in row.values()):
                fail(f"{path}:{reader.line_num}: malformed CSV width")
            yield f"{path}:{reader.line_num}", row


def integer(row, field, where):
    try:
        value = int(row[field])
    except ValueError as error:
        raise ValueError(f"{where}: invalid integer {field}={row[field]!r}") from error
    if value < 0:
        fail(f"{where}: negative {field}={value}")
    return value


def nearest_rank(values, fraction):
    if not values:
        fail("nearest rank requested for an empty sample")
    ordered = sorted(values)
    return ordered[math.ceil(fraction * len(ordered)) - 1]


def release_ticks(seed, profile):
    """Reimplement the registered SplitMix64 trace, without calling the simulator."""
    state = seed
    releases = [0]
    for burst in range(1, profile["bursts"]):
        state = (state + 0x9E3779B97F4A7C15) & MASK
        value = state
        value = ((value ^ (value >> 30)) * 0xBF58476D1CE4E5B9) & MASK
        value = ((value ^ (value >> 27)) * 0x94D049BB133111EB) & MASK
        value ^= value >> 31
        releases.append(
            burst * profile["spacing"]
            + value % (2 * profile["jitter"] + 1)
            - profile["jitter"]
        )
    return releases


def read_summary(path):
    result = {}
    for where, raw in rows(path, SUMMARY_FIELDS):
        profile_name = raw["profile"]
        variant = raw["variant"]
        seed = integer(raw, "seed", where)
        key = (profile_name, seed, variant)
        if profile_name not in PROFILES or seed not in SEEDS or variant not in VARIANTS or key in result:
            fail(f"{where}: unknown or duplicate run {key}")
        profile = PROFILES[profile_name]
        offered_messages = profile["producers"] * profile["bursts"]
        offered_bytes = offered_messages * profile["message_bytes"]
        expected_packets = offered_messages * profile["packets"]
        fixed = {
            "producers": profile["producers"],
            "bursts": profile["bursts"],
            "message_bytes": profile["message_bytes"],
            "packet_size": PACKET_SIZE,
            "data_delay_ticks": DATA_DELAY,
            "control_delay_ticks": CONTROL_DELAY,
            "bdp_packets": BDP_PACKETS,
            "sender_queue_cap_bytes": profile["sender_cap"],
            "switch_queue_cap_bytes": SWITCH_CAP,
            "receiver_queue_cap_bytes": 0,
            "injection_interval_ticks": profile["injection"],
            "drain_deadline_ticks": profile["deadline"],
            "nominal_burst_spacing_ticks": profile["spacing"],
            "jitter_magnitude_ticks": profile["jitter"],
            "control_message_bytes": CONTROL_BYTES,
            "switch_egress_packets_per_tick": 1,
            "sender_packets_per_tick": 1,
            "offered_messages": offered_messages,
            "offered_bytes": offered_bytes,
            "unscheduled_prefix_packets": int(variant == "M2"),
        }
        item = {field: integer(raw, field, where) for field in SUMMARY_FIELDS if field not in ("profile", "variant")}
        item["profile"] = profile_name
        item["variant"] = variant
        for field, expected in fixed.items():
            if item[field] != expected:
                fail(f"{where}: {field}={item[field]} differs from registered {expected}")
        unscheduled = offered_messages if variant == "M2" else 0
        scheduled = expected_packets - unscheduled
        expected_control_messages = scheduled + (offered_messages if variant == "M2" else 2 * offered_messages)
        expected_embedded = CONTROL_BYTES * unscheduled
        exact = {
            "data_packets_sent": expected_packets,
            "data_packets_delivered": expected_packets,
            "scheduled_packets_sent": scheduled,
            "unscheduled_packets_sent": unscheduled,
            "control_messages": expected_control_messages,
            "control_bytes": CONTROL_BYTES * expected_control_messages,
            "embedded_metadata_bytes": expected_embedded,
            "data_wire_bytes_sent": offered_bytes + expected_embedded,
        }
        for field, expected in exact.items():
            if item[field] != expected:
                fail(f"{where}: {field}={item[field]} disagrees with protocol accounting {expected}")
        if item["total_queue_byte_ticks"] != sum(item[field] for field in ("sender_byte_ticks", "switch_byte_ticks", "receiver_byte_ticks")):
            fail(f"{where}: queued byte-time accounting mismatch")
        if item["receiver_peak_bytes"] or item["receiver_byte_ticks"]:
            fail(f"{where}: network-limited receiver has a queue")
        if item["total_queue_peak_bytes"] < max(item["sender_peak_bytes"], item["switch_peak_bytes"]):
            fail(f"{where}: total queue peak below a component peak")
        if item["total_queue_peak_bytes"] > item["sender_peak_bytes"] + item["switch_peak_bytes"]:
            fail(f"{where}: total queue peak exceeds component peaks")
        if item["max_sender_queue_per_producer_bytes"] > profile["sender_cap"]:
            fail(f"{where}: sender unsent cap breached")
        if item["max_sender_retained_per_producer_bytes"] > profile["sender_cap"]:
            fail(f"{where}: sender retained-copy cap breached")
        if item["sender_peak_bytes"] > profile["producers"] * profile["sender_cap"]:
            fail(f"{where}: aggregate sender unsent peak exceeds per-producer caps")
        if item["sender_retained_peak_bytes"] > profile["producers"] * profile["sender_cap"]:
            fail(f"{where}: aggregate retained-source peak exceeds per-producer caps")
        if item["switch_peak_bytes"] > SWITCH_CAP:
            fail(f"{where}: switch queue cap breached")
        if item["max_granted_outstanding_packets"] > BDP_PACKETS:
            fail(f"{where}: scheduled global credit budget breached")
        if item["max_sender_window_outstanding_packets"] != 0:
            fail(f"{where}: sender-window counter is nonzero in M1/M2")
        result[key] = item
    expected_runs = len(PROFILES) * len(SEEDS) * len(VARIANTS)
    if len(result) != expected_runs:
        fail(f"{path}: expected {expected_runs} profile/seed/variant runs, got {len(result)}")
    return result


def read_h1_m1_timings(path):
    timings = {}
    opener = lambda name: gzip.open(name, "rt", newline="", encoding="utf-8")
    for where, raw in rows(path, H1_MESSAGE_FIELDS, opener):
        if raw["variant"] != "M1":
            continue
        seed = integer(raw, "seed", where)
        producer = integer(raw, "producer", where)
        burst = integer(raw, "burst", where)
        sequence = integer(raw, "sequence", where)
        if seed not in SEEDS or not (0 <= producer < 32 and 0 <= burst < 1_000) or sequence != burst + 1:
            fail(f"{where}: invalid preserved H1 identity")
        identity = (seed, producer, sequence)
        if identity in timings:
            fail(f"{where}: duplicate preserved H1 identity {identity}")
        timings[identity] = tuple(integer(raw, field, where) for field in H1_MESSAGE_FIELDS[5:])
    if len(timings) != len(SEEDS) * 32 * 1_000:
        fail(f"{path}: incomplete preserved H1 M1 message rows")
    return timings


def read_messages(path, summary, h1_timings):
    releases = {(profile_name, seed): release_ticks(seed, profile) for profile_name, profile in PROFILES.items() for seed in SEEDS}
    state = {}
    for key in summary:
        profile = PROFILES[key[0]]
        state[key] = {
            "seen": bytearray(profile["producers"] * profile["bursts"]),
            "count": 0,
            "injection_messages": 0,
            "deadline_messages": 0,
            "deadline_acks": 0,
            "producer_injection_bytes": [0] * profile["producers"],
            "network": [[] for _ in range(profile["producers"])],
            "durable": [[] for _ in range(profile["producers"])],
            "burst_last_complete": [0] * profile["bursts"],
            "post_deadline": 0,
        }
    for where, raw in rows(path, MESSAGE_FIELDS):
        profile_name = raw["profile"]
        seed = integer(raw, "seed", where)
        key = (profile_name, seed, raw["variant"])
        if key not in state:
            fail(f"{where}: unknown run {key}")
        profile = PROFILES[profile_name]
        producer = integer(raw, "producer", where)
        sequence = integer(raw, "sequence", where)
        burst = integer(raw, "burst", where)
        size = integer(raw, "bytes", where)
        start = integer(raw, "release_tick", where)
        first = integer(raw, "first_send_tick", where)
        complete = integer(raw, "complete_tick", where)
        commit = integer(raw, "modeled_commit_tick", where)
        ack = integer(raw, "ack_tick", where)
        if not (0 <= producer < profile["producers"] and 0 <= burst < profile["bursts"]):
            fail(f"{where}: producer or burst outside registered profile")
        if sequence != burst + 1 or size != profile["message_bytes"] or start != releases[(profile_name, seed)][burst]:
            fail(f"{where}: identity, size, or SplitMix64 release differs from registration")
        index = producer * profile["bursts"] + burst
        item = state[key]
        if item["seen"][index]:
            fail(f"{where}: duplicate message identity")
        item["seen"][index] = 1
        item["count"] += 1
        if first < start or complete < first + DATA_DELAY + profile["packets"]:
            fail(f"{where}: impossible first-send or completion timing")
        if raw["variant"] == "M1" and first < start + 2 * CONTROL_DELAY:
            fail(f"{where}: M1 scheduled DATA sent before request/credit round trip")
        if commit != complete or ack != commit + CONTROL_DELAY:
            fail(f"{where}: modeled commit or durable ACK timing disagrees with protocol")
        if profile_name == "incast" and raw["variant"] == "M1":
            identity = (seed, producer, sequence)
            expected = h1_timings.pop(identity, None)
            actual = (size, start, first, complete, commit, ack)
            if actual != expected:
                fail(f"{where}: M1 timing differs from preserved H1 for {identity}: {actual} != {expected}")
        if complete < profile["injection"]:
            item["injection_messages"] += 1
            item["producer_injection_bytes"][producer] += size
        if complete <= profile["deadline"]:
            item["deadline_messages"] += 1
        else:
            item["post_deadline"] += 1
        if ack <= profile["deadline"]:
            item["deadline_acks"] += 1
        else:
            item["post_deadline"] += 1
        item["network"][producer].append(complete - start)
        item["durable"][producer].append(ack - start)
        item["burst_last_complete"][burst] = max(item["burst_last_complete"][burst], complete)
    if h1_timings:
        fail(f"{path}: {len(h1_timings)} preserved H1 M1 identities missing from incast CSV")
    output = {}
    for key, item in state.items():
        profile = PROFILES[key[0]]
        expected = profile["producers"] * profile["bursts"]
        if item["count"] != expected or not all(item["seen"]):
            fail(f"{path}: incomplete message identities for {key}: {item['count']} / {expected}")
        run = summary[key]
        checks = {
            "completed_injection_messages": item["injection_messages"],
            "completed_injection_bytes": item["injection_messages"] * profile["message_bytes"],
            "completed_deadline_messages": item["deadline_messages"],
            "ack_count": item["deadline_acks"],
            "modeled_commits": item["deadline_messages"],
            "unacked_at_deadline": expected - item["deadline_acks"],
        }
        for field, value in checks.items():
            if run[field] != value:
                fail(f"{path}: {key} {field}={run[field]} disagrees with message rows ({value})")
        network = [duration for producer in item["network"] for duration in producer]
        durable = [duration for producer in item["durable"] for duration in producer]
        output[key] = {
            "p50_network_ticks": nearest_rank(network, 0.50),
            "p99_network_ticks": nearest_rank(network, 0.99),
            "p50_durable_ack_ticks": nearest_rank(durable, 0.50),
            "p99_durable_ack_ticks": nearest_rank(durable, 0.99),
            "producer_p99_network_ticks": [nearest_rank(x, 0.99) for x in item["network"]],
            "producer_max_network_ticks": [max(x) for x in item["network"]],
            "producer_injection_bytes": item["producer_injection_bytes"],
            "burst_last_complete": item["burst_last_complete"],
            "post_deadline_events": item["post_deadline"],
        }
    return output, releases


def read_bursts(path, summary, messages, releases):
    peaks = {key: [None] * PROFILES[key[0]]["bursts"] for key in summary}
    for where, raw in rows(path, BURST_FIELDS):
        profile_name = raw["profile"]
        seed = integer(raw, "seed", where)
        key = (profile_name, seed, raw["variant"])
        if key not in peaks:
            fail(f"{where}: unknown run {key}")
        burst = integer(raw, "burst", where)
        if not 0 <= burst < PROFILES[profile_name]["bursts"] or peaks[key][burst] is not None:
            fail(f"{where}: invalid or duplicate burst")
        start = integer(raw, "release_tick", where)
        last = integer(raw, "last_complete_tick", where)
        peak = integer(raw, "peak_hotspot_bytes", where)
        if start != releases[(profile_name, seed)][burst] or last != messages[key]["burst_last_complete"][burst]:
            fail(f"{where}: burst interval disagrees with message rows")
        if peak > summary[key]["switch_peak_bytes"]:
            fail(f"{where}: burst hot-spot exceeds run switch peak")
        peaks[key][burst] = peak
    for key, values in peaks.items():
        if any(value is None for value in values):
            fail(f"{path}: missing burst rows for {key}")
        if nearest_rank(values, 0.99) != summary[key]["p99_burst_peak_hotspot_bytes"]:
            fail(f"{path}: p99 burst hot-spot disagrees with summary for {key}")
        if key[0] == "incast" and key[2] == "M1" and nearest_rank(values, 0.99) != 13_500:
            fail(f"{path}: incast M1 p99 burst peak differs from preserved H1 13,500 B")
    return peaks


def cost_metrics(run):
    fields = (
        "sender_byte_ticks", "switch_byte_ticks", "total_queue_byte_ticks",
        "sender_retained_peak_bytes", "sender_peak_bytes", "switch_peak_bytes",
        "control_bytes", "embedded_metadata_bytes", "data_wire_bytes_sent",
        "scheduled_packets_sent", "unscheduled_packets_sent",
    )
    result = {field: run[field] for field in fields}
    result["modeled_overhead_bytes"] = run["control_bytes"] + run["embedded_metadata_bytes"]
    return result


def paired_bootstrap(values):
    rng = random.Random(42)
    samples = sorted(
        sum(values[rng.randrange(len(values))] for _ in values) / len(values)
        for _ in range(10_000)
    )
    return [samples[math.ceil(0.025 * len(samples)) - 1], samples[math.ceil(0.975 * len(samples)) - 1]]


def analyze(summary, messages):
    profiles = {}
    for profile_name, profile in PROFILES.items():
        paired = []
        gate_failures = []
        reductions = []
        for seed in SEEDS:
            m1 = summary[(profile_name, seed, "M1")]
            m2 = summary[(profile_name, seed, "M2")]
            a = messages[(profile_name, seed, "M1")]
            b = messages[(profile_name, seed, "M2")]
            for variant, run, detail in (("M1", m1, a), ("M2", m2, b)):
                for field in ("rejection_count", "loss_count", "cap_overflow_count", "unacked_at_deadline"):
                    if run[field]:
                        gate_failures.append(f"seed {seed} {variant}: {field}={run[field]}")
                if run["completed_deadline_messages"] != run["offered_messages"]:
                    gate_failures.append(f"seed {seed} {variant}: messages incomplete by deadline")
                if run["completed_injection_bytes"] * 100 < 95 * run["offered_bytes"]:
                    gate_failures.append(f"seed {seed} {variant}: less than 95% of offered payload completed before injection end")
                if detail["post_deadline_events"]:
                    gate_failures.append(f"seed {seed} {variant}: completion or ACK after deadline")
            achieved1 = m1["completed_injection_bytes"]
            achieved2 = m2["completed_injection_bytes"]
            if achieved1 == 0 or abs(achieved2 - achieved1) * 100 > 5 * achieved1:
                gate_failures.append(f"seed {seed}: paired injection goodput differs by more than 5%")
            p1 = a["p99_network_ticks"]
            p2 = b["p99_network_ticks"]
            reduction = None if p1 == 0 else (p1 - p2) / p1
            if profile_name == "small":
                if reduction is None:
                    gate_failures.append(f"seed {seed}: zero M1 p99 makes relative reduction undefined")
                else:
                    reductions.append(reduction)
            if profile_name == "incast":
                if m2["p99_burst_peak_hotspot_bytes"] > 65_536:
                    gate_failures.append(f"seed {seed}: M2 p99 burst peak exceeds 65,536 wire bytes")
                for producer, (control, candidate) in enumerate(zip(a["producer_p99_network_ticks"], b["producer_p99_network_ticks"])):
                    if candidate * 100 > control * 105:
                        gate_failures.append(f"seed {seed} producer {producer}: M2 p99 network latency exceeds 1.05x M1")
                for producer, (control, candidate) in enumerate(zip(a["producer_max_network_ticks"], b["producer_max_network_ticks"])):
                    if candidate > control * 2:
                        gate_failures.append(f"seed {seed} producer {producer}: M2 max network latency exceeds 2x M1")
            costs1 = cost_metrics(m1)
            costs2 = cost_metrics(m2)
            paired.append({
                "seed": seed,
                "m1": {
                    "p50_network_ticks": a["p50_network_ticks"],
                    "p99_network_ticks": p1,
                    "p50_durable_ack_ticks": a["p50_durable_ack_ticks"],
                    "p99_durable_ack_ticks": a["p99_durable_ack_ticks"],
                    "p99_burst_peak_hotspot_bytes": m1["p99_burst_peak_hotspot_bytes"],
                    "completed_injection_bytes": achieved1,
                    "goodput_payload_bytes_per_tick": achieved1 / profile["injection"],
                    "producer_injection_bytes": a["producer_injection_bytes"],
                    "producer_p99_network_ticks": a["producer_p99_network_ticks"],
                    "producer_max_network_ticks": a["producer_max_network_ticks"],
                    "costs": costs1,
                },
                "m2": {
                    "p50_network_ticks": b["p50_network_ticks"],
                    "p99_network_ticks": p2,
                    "p50_durable_ack_ticks": b["p50_durable_ack_ticks"],
                    "p99_durable_ack_ticks": b["p99_durable_ack_ticks"],
                    "p99_burst_peak_hotspot_bytes": m2["p99_burst_peak_hotspot_bytes"],
                    "completed_injection_bytes": achieved2,
                    "goodput_payload_bytes_per_tick": achieved2 / profile["injection"],
                    "producer_injection_bytes": b["producer_injection_bytes"],
                    "producer_p99_network_ticks": b["producer_p99_network_ticks"],
                    "producer_max_network_ticks": b["producer_max_network_ticks"],
                    "costs": costs2,
                },
                "relative_p99_network_reduction": reduction,
                "goodput_relative_change": None if achieved1 == 0 else (achieved2 - achieved1) / achieved1,
                "m2_minus_m1_costs": {field: costs2[field] - costs1[field] for field in costs1},
            })
        mean = sum(reductions) / len(reductions) if len(reductions) == len(SEEDS) else None
        interval = paired_bootstrap(reductions) if mean is not None else None
        if profile_name == "small" and mean is not None:
            if mean < 0.10:
                gate_failures.append(f"mean paired p99 reduction {mean:.6f} is below 10%")
            if interval[0] <= 0:
                gate_failures.append(f"paired-bootstrap lower 95% endpoint {interval[0]:.6f} is not above zero")
        aggregate = {
            "mean_m1_p99_network_ticks": sum(item["m1"]["p99_network_ticks"] for item in paired) / len(paired),
            "mean_m2_p99_network_ticks": sum(item["m2"]["p99_network_ticks"] for item in paired) / len(paired),
            "mean_m1_p99_burst_peak_hotspot_bytes": sum(item["m1"]["p99_burst_peak_hotspot_bytes"] for item in paired) / len(paired),
            "mean_m2_p99_burst_peak_hotspot_bytes": sum(item["m2"]["p99_burst_peak_hotspot_bytes"] for item in paired) / len(paired),
            "mean_m2_minus_m1_costs": {
                field: sum(item["m2_minus_m1_costs"][field] for item in paired) / len(paired)
                for field in paired[0]["m2_minus_m1_costs"]
            },
        }
        if profile_name == "small":
            aggregate["mean_relative_p99_network_reduction"] = mean
            aggregate["paired_bootstrap_95_percent"] = interval
        profiles[profile_name] = {
            "status": "inconclusive" if gate_failures else "advances",
            "gate_failures": gate_failures,
            "paired_seeds": paired,
            "aggregate": aggregate,
        }
    return {
        "status": "advances" if all(result["status"] == "advances" for result in profiles.values()) else "inconclusive",
        "profiles": profiles,
        "limitations": [
            "CSV totals and message timings cannot independently prove that each scheduled DATA packet consumed a credit or reconstruct every per-tick queue peak; the simulator tests cover those paths.",
            "This network-limited packet-slot model has no data/control loss, control-link contention, priority queues, disk service, or real durable transport.",
        ],
    }


def main():
    if len(sys.argv) != 2:
        fail("usage: python3 -B tools/bench/summarize_transport_m2.py OUTPUT_DIR")
    root = Path(sys.argv[1])
    summary = read_summary(root / "summary.csv")
    h1_timings = read_h1_m1_timings(H1_MESSAGES)
    messages, releases = read_messages(root / "messages.csv", summary, h1_timings)
    read_bursts(root / "bursts.csv", summary, messages, releases)
    print(json.dumps(analyze(summary, messages), indent=2, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, UnicodeError, ValueError, csv.Error) as error:
        print(error, file=sys.stderr)
        sys.exit(1)
