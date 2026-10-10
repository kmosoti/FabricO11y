#!/usr/bin/env python3
"""Reduce a retained remote simulator case to compact diagnostic metrics."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import tarfile
import tempfile

import service


def percentile(values):
    ordered = sorted(values)
    def rank(p):
        return ordered[max(0, (len(ordered) * p + 99) // 100 - 1)] if ordered else None
    return {"samples": len(ordered), "p50": rank(50), "p99": rank(99),
            "max": ordered[-1] if ordered else None}


def kv(text):
    return {key: int(value) for key, value in
            (line.split() for line in text.splitlines())}


def validate_times(created, event_time, attempts):
    for key, start, end, _kind in attempts:
        if key not in created or not event_time[key] <= created[key] <= start <= end:
            raise ValueError("invalid simulator event ordering")


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def remote_inputs(work):
    names = ["remote/resources.json", "remote/sim/events.jsonl",
             "remote/sim/transcript.jsonl", "remote/sim/sim-summary.json",
             "remote/worker-result.json"]
    paths = {name: work / name for name in names}
    if all(path.is_file() for path in paths.values()):
        return paths, None, {}
    archive, metadata = work / "remote.tar.gz", work / "remote-archive.json"
    if not archive.is_file() or not metadata.is_file():
        raise FileNotFoundError("remote inputs or authenticated remote archive are missing")
    manifest = json.loads(metadata.read_text())
    if archive.stat().st_size != int(manifest["bytes"]) or digest(archive) != manifest["sha256"]:
        raise ValueError("remote archive size/SHA-256 does not match remote-archive.json")
    scratch = Path(os.environ["FABRIC_SCRATCH_ROOT"]).resolve(strict=True)
    extracted = Path(tempfile.mkdtemp(prefix="hammer-remote-reduce-", dir=scratch))
    wanted = {name.removeprefix("remote/"): name for name in names}
    total = 0
    found = set()
    with tarfile.open(archive, "r:gz") as tar:
        for member in tar:
            parts = Path(member.name).parts
            if Path(member.name).is_absolute() or ".." in parts:
                raise ValueError("unsafe member path in authenticated archive")
            if member.isdir():
                continue
            if not member.isfile():
                raise ValueError("non-regular archive member rejected")
            normalized = "/".join(part for part in parts if part not in (".", ""))
            if normalized not in wanted:
                continue
            if normalized in found:
                raise ValueError("duplicate required member in remote archive")
            total += member.size
            if total > 64 * 1024 * 1024:
                raise ValueError("required remote archive members exceed 64 MiB")
            source = tar.extractfile(member)
            if source is None:
                raise ValueError("required archive member has no regular file payload")
            destination = extracted / normalized
            destination.parent.mkdir(parents=True, exist_ok=True)
            with source, destination.open("xb") as output:
                shutil.copyfileobj(source, output, 1024 * 1024)
            found.add(normalized)
    if found != set(wanted):
        raise ValueError("remote archive is missing required diagnostic files")
    resolved = {name: extracted / name.removeprefix("remote/") for name in names}
    provenance = {"remote.tar.gz": digest(archive), "remote-archive.json": digest(metadata)}
    return resolved, extracted, provenance


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    service.resource_group.require_limits()
    work = args.work.resolve(strict=True)
    required = ["grading.json", "delivery-verdict.json", "recovered-hashes.jsonl.gz"]
    paths = {name: work / name for name in required}
    missing = [name for name, path in paths.items() if not path.is_file()]
    if missing:
        parser.error("missing retained case inputs: " + ", ".join(missing))
    remote_paths, extraction, archive_hashes = remote_inputs(work)
    paths.update(remote_paths)
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)

    sim = json.loads(paths["remote/sim/sim-summary.json"].read_text())
    worker = json.loads(paths["remote/worker-result.json"].read_text())
    verdict = json.loads(paths["delivery-verdict.json"].read_text())
    began = int(sim["began_unix_ns"])
    created, event_time, attempts, ack_ends = {}, {}, [], {}
    lateness, buckets = [], {}
    with paths["remote/sim/events.jsonl"].open() as stream:
        for line in stream:
            row = json.loads(line)
            if row.get("e") == "created":
                key = (int(row["id"]), int(row["seq"]))
                if key in created:
                    raise ValueError(f"duplicate created Batch {key}")
                created[key] = int(row["generated_ns"])
                event_time[key] = int(row["t"])
                late = (created[key] - (began + (key[1] - 1) * 1_000_000_000)) / 1e6
                lateness.append(late)
                bucket = max(0, (created[key] - began) // 10_000_000_000)
                buckets.setdefault(int(bucket), {"created": 0, "acked": 0})["created"] += 1
            elif row.get("e") == "attempt":
                key = (int(row["id"]), int(row["seq"]))
                attempts.append((key, int(row["start"]), int(row["end"]), row["kind"]))
                if row["kind"] == "ack":
                    if key in ack_ends:
                        raise ValueError(f"duplicate ACKed Batch {key}")
                    ack_ends[key] = int(row["end"])
                    bucket = max(0, (int(row["end"]) - began) // 10_000_000_000)
                    buckets.setdefault(int(bucket), {"created": 0, "acked": 0})["acked"] += 1

    if not set(ack_ends) <= set(created) or not {a[0] for a in attempts} <= set(created):
        raise ValueError("attempt or ACK has no created Batch")
    validate_times(created, event_time, attempts)
    sources, transcript_acks, transcript_attempts = set(), set(), set()
    transcript_source_count = transcript_attempt_count = transcript_ack_count = 0
    with paths["remote/sim/transcript.jsonl"].open() as stream:
        for line in stream:
            row = json.loads(line)
            kind = row.get("type")
            if kind == "source":
                key = (row["node_id"], int(row["sequence"]))
                if key in sources:
                    raise ValueError("duplicate transcript source Batch")
                sources.add(key)
                transcript_source_count += 1
            elif kind == "attempt":
                transcript_attempts.add((row["node_id"], int(row["sequence"])))
                transcript_attempt_count += 1
            elif kind == "response" and row.get("kind") == "ack":
                transcript_acks.add((row["node_id"], int(row["sequence"])))
                transcript_ack_count += 1

    recovered, recovered_bytes = set(), 0
    with gzip.open(paths["recovered-hashes.jsonl.gz"], "rt") as stream:
        for line in stream:
            label, sequence, _record_digest, size, _received_ns = json.loads(line)
            key = (label, int(sequence))
            if key in recovered:
                raise ValueError("duplicate recovered Batch")
            recovered.add(key)
            recovered_bytes += int(size)

    pending = int(sim["undelivered"])
    acked = set(ack_ends)
    if (len(created) != transcript_source_count or len(created) != len(sources)
            or len(attempts) != transcript_attempt_count
            or len({a[0] for a in attempts}) != len(transcript_attempts)
            or len(acked) != transcript_ack_count
            or len(acked) != len(transcript_acks)
            or len(recovered) != len(acked)
            or len(created) != len(acked) + pending):
        raise ValueError("simulator/event/transcript/recovery counts do not reconcile")
    retained = 0
    with paths["remote/sim/transcript.jsonl"].open() as stream:
        for line in stream:
            row = json.loads(line)
            if row.get("type") == "node_state":
                retained += len(row["retained_sequences"])
    if retained != pending:
        raise ValueError("transcript retained sequences disagree with simulator pending count")

    rtt_ms = [(end - start) / 1e6 for _, start, end, _ in attempts]
    creation_ack_ms = [(ack_ends[key] - created[key]) / 1e6 for key in acked]
    event_to_ack_ms = [(ack_ends[key] - event_time[key]) / 1e6 for key in acked]
    ordered_created = sorted(created.values())
    span = ((ordered_created[-1] - ordered_created[0]) / 1e9) if ordered_created else 0.0

    samples = json.loads(paths["remote/resources.json"].read_text())
    sample_cpu = [sum(float(p.get("cpu_s", 0)) for p in row.get("processes", []))
                  for row in samples]
    sample_rss = [sum(int(p.get("rss_kib", 0)) for p in row.get("processes", []))
                  for row in samples]
    mem_peak = max((int(row["cgroup"]["memory.peak"]) for row in samples), default=0)
    high_events = max((kv(row["cgroup"]["memory.events"]).get("high", 0)
                       for row in samples), default=0)
    throttled = max((kv(row["cgroup"]["cpu.stat"]).get("throttled_usec", 0)
                     for row in samples), default=0)
    throttled_periods = max((kv(row["cgroup"]["cpu.stat"]).get("nr_throttled", 0)
                             for row in samples), default=0)
    cpu_usage = max((kv(row["cgroup"]["cpu.stat"]).get("usage_usec", 0)
                     for row in samples), default=0)
    final = worker.get("final_cgroup", {})
    mem_peak = max(mem_peak, int(final.get("memory.peak", 0)))
    high_events = max(high_events, kv(final.get("memory.events", "")).get("high", 0))
    report = {
        "input_sha256": {**{name: digest(path) for name, path in paths.items()}, **archive_hashes},
        "grading": json.loads(paths["grading.json"].read_text()),
        "simulation": {
            "identities": sim["identities"], "scheduled_seconds": sim["seconds"],
            "scheduled_batches_fixed_one_per_identity_second": sim["identities"] * sim["seconds"],
            "uncreated_scheduled_batches": sim["identities"] * sim["seconds"] - len(created),
            "worker_claimed_offered_logs": worker["offered_logs"],
            "inferred_created_logs_fixed_ten_per_batch": len(created) * 10,
            "inferred_log_count_boundary": "This registered simulator uses log_factor=5, two base logs per identity-second; pending payload bytes are not independently replayed.",
            "created_unique_batches": len(created), "attempted_unique_batches": len({a[0] for a in attempts}),
            "attempts": len(attempts), "acked_unique_batches": len(acked),
            "pending_batches": pending, "pending_reconciles": len(created) == len(acked) + pending == retained + len(acked),
            "creation_span_s": span,
            "creation_relative_to_start_s": [(ordered_created[0] - began) / 1e9,
                                               (ordered_created[-1] - began) / 1e9] if ordered_created else [],
            "creation_lateness_from_sequence_schedule_ms": percentile(lateness),
            "created_and_acked_per_10s": [dict(bucket_start_s=10 * index, **buckets[index])
                                          for index in sorted(buckets)],
            "attempt_rtt_ms": percentile(rtt_ms),
            "generated_to_ack_ms": percentile(creation_ack_ms),
            "event_t_to_ack_ms": percentile(event_to_ack_ms),
        },
        "recovery": {"unique_batches": len(recovered), "encoded_bytes": recovered_bytes,
                     "approx_encoded_bytes_per_batch": recovered_bytes / len(recovered) if recovered else None},
        "remote_samples": {
            "count": len(samples), "max_sampled_process_cpu_s": max(sample_cpu, default=0),
            "max_sampled_cgroup_cpu_usage_usec": cpu_usage,
            "max_sampled_process_rss_kib": max(sample_rss, default=0),
            "max_sampled_or_final_cgroup_memory_peak_bytes": mem_peak,
            "max_memory_high_events": high_events,
            "max_sampled_cpu_throttled_usec": throttled,
            "max_sampled_cpu_throttled_periods": throttled_periods,
            "peak_sampled_disk_bytes": max((int(row.get("disk_bytes", 0)) for row in samples), default=0),
        },
        "provenance": {"delivery_oracle_passed": bool(verdict.get("passed")),
                       "stream_reconciliation": "Counts and cardinalities only: simulator index-to-node mapping is not recorded. Latencies come from the event stream; exact custody is graded separately by the unchanged delivery oracle.",
                       "worker_offered_logs_field_is_untrusted_summary": True,
                       "queued_state_is_in_memory_not_a_disk_spool": True,
                       "cross_host_latency_inferred": False,
                       "worker_clock_boundary": worker.get("clock_boundary"),
                       "worker_elapsed_s": worker.get("elapsed_s")},
    }
    (out / "remote-reduction.json").write_text(json.dumps(report, indent=2) + "\n")
    if extraction is not None:
        shutil.rmtree(extraction)
    print(out / "remote-reduction.json")


if __name__ == "__main__":
    main()
