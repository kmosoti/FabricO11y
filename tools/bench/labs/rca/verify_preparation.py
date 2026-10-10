#!/usr/bin/env python3
"""Prepare and validate the bounded RCA fixture packet (not a Fabric run)."""

import argparse
import base64
import copy
import hashlib
import json
import os
from pathlib import Path
import resource
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / "tools"))
import resource_group

sys.path.insert(0, str(ROOT / "tools/qualification"))
import query_oracle

HERE = Path(__file__).resolve().parent
PROTOCOL = ROOT / "docs/experiments/benchmarks/use-case-preparation-protocol.md"
EVIDENCE_PARENT = ROOT / "docs/experiments/benchmarks/data/hammer-reference-01/query"
MAX_EVIDENCE = 8 * 1024 * 1024


def digest(data):
    return hashlib.sha256(data).hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read_packet(packet):
    require((packet / "controller/producer.jsonl").is_file(), "producer file missing")
    require((packet / "investigator/playbooks.json").is_file(), "playbooks missing")
    require((packet / "controller/truth.json").is_file(), "controller truth missing")
    producer = [json.loads(line) for line in
                (packet / "controller/producer.jsonl").read_text().splitlines() if line]
    playbooks = json.loads((packet / "investigator/playbooks.json").read_text())
    truth = json.loads((packet / "controller/truth.json").read_text())
    return producer, playbooks, truth


def decode_rows(producer):
    decoded = []
    for row in producer:
        raw = base64.b64decode(row["bytes"], validate=True)
        require(digest(raw) == row["sha256"], "payload digest mismatch")
        batch = query_oracle.decode_batch(raw)
        require(batch["version"] == 1, "unexpected Batch version")
        require(batch["node_id"].hex() == row["node_id"], "node identity mismatch")
        require(row["node_id"] == hashlib.sha256(
            ("rca-node:" + row["label"]).encode()).digest()[:16].hex(),
            "source identity does not match its fixture label")
        require(batch["generation"] == row["generation"], "generation mismatch")
        require(batch["sequence"] == row["sequence"], "sequence mismatch")
        require(len(raw) < 1024 * 1024, "Batch exceeds envelope cap")
        decoded.append((row, batch, query_oracle.decode_metrics_request(batch["metrics_bytes"]),
                       query_oracle.decode_logs_request(batch["logs_bytes"]),
                       query_oracle.decode_traces_request(batch["traces_bytes"])))
    return decoded


def validate(producer, playbooks, truth, investigator_files, scenario_cases):
    require(len(producer) == 15, "expected 15 producer Batches")
    require(len(playbooks) == 7 and len(truth) == 7, "expected seven cases/playbooks/truth rows")
    require(set(investigator_files) == {"playbooks.json"},
            "investigator output must contain playbooks.json only")
    require(len({p["case"] for p in playbooks}) == 7, "scenario identities are not distinct")
    require(not any(any(word in name.lower() for word in ("truth", "scenario", "controller"))
                    for name in investigator_files), "controller source in investigator output")
    decoded = decode_rows(producer)
    by_case = {}
    for row, batch, metrics, logs, spans in decoded:
        cid = row["case"]
        require(cid in {f"rca-{i:02}" for i in range(1, 8)}, "unexpected investigator identity")
        by_case.setdefault(cid, []).append((row, batch, metrics, logs, spans))

    require(set(by_case) == {f"rca-{i:02}" for i in range(1, 8)}, "scenario identity missing")
    require(sum(r[0]["stage"] == "initial" for values in by_case.values() for r in values) == 14,
            "expected fourteen initial Batches")
    require(sum(r[0]["stage"] == "late" for values in by_case.values() for r in values) == 1,
            "expected one late Batch")

    truth_by_case = {t["case"]: t for t in truth}
    playbook_by_case = {p["case"]: p for p in playbooks}
    scenario_by_case = {f"rca-{index:02}": case
                        for index, case in enumerate(scenario_cases, 1)}
    require(set(truth_by_case) == set(by_case) == set(playbook_by_case), "case sets differ")
    for cid, values in by_case.items():
        values.sort(key=lambda x: (x[0]["label"], x[0]["sequence"]))
        expected_late = cid == "rca-05"
        stages = [r[0]["stage"] for r in values]
        require(stages.count("initial") == 2 and stages.count("late") == int(expected_late),
                f"wrong stage population for {cid}")
        by_label = {}
        for row, batch, metrics, logs, spans in values:
            require(row["label"] in {cid + "-api", cid + "-dependency"}, "non-opaque source label")
            by_label.setdefault(row["label"], []).append((row, batch, metrics, logs, spans))
        require(set(by_label) == {cid + "-api", cid + "-dependency"}, "source label missing")
        for label, records in by_label.items():
            records.sort(key=lambda x: x[0]["sequence"])
            require([r[0]["sequence"] for r in records] == list(range(1, len(records) + 1)),
                    f"non-contiguous sequence for {label}")
        api = by_label[cid + "-api"][0]
        dep = by_label[cid + "-dependency"]
        require(api[0]["stage"] == "initial" and dep[0][0]["stage"] == "initial",
                f"initial source records missing for {cid}")
        require(api[1]["node_id"] != dep[0][1]["node_id"], "source identities must differ")

        metrics = {m["name"]: m for m in api[2]}
        require({"app.pool.in_use", "app.pool.capacity", "system.memory.available", "app.requests"}
                <= set(metrics), f"required metrics missing for {cid}")
        require(metrics["app.pool.in_use"]["unit"] == "1" and
                metrics["app.pool.capacity"]["unit"] == "1" and
                metrics["app.requests"]["unit"] == "1" and
                metrics["system.memory.available"]["unit"] == "By",
                f"metric units incorrect for {cid}")
        require(metrics["app.requests"]["kind"] == "sum" and metrics["app.requests"]["monotonic"],
                f"request counter is not monotonic Sum for {cid}")
        case_truth = truth_by_case[cid]
        scenario = scenario_by_case[cid]
        require(case_truth["scenario_id"] == scenario["id"],
                f"scenario/truth mapping differs for {cid}")
        for metric_name, expected_value in (
                ("app.pool.in_use", scenario["pool_in_use"]),
                ("app.pool.capacity", scenario["pool_capacity"]),
                ("system.memory.available", scenario["available_memory_bytes"])):
            require(metrics[metric_name]["points"][0]["value"] == expected_value,
                    f"metric fixture value incorrect for {cid}:{metric_name}")

        api_spans = {s["name"]: s for s in api[4]}
        require(set(api_spans) == {"request", "pool.acquire"}, f"API spans incorrect for {cid}")
        request = api_spans["request"]
        acquire = api_spans["pool.acquire"]
        require(request["parent_span_id"] == "" and acquire["parent_span_id"] == request["span_id"],
                f"API parent-child identity incorrect for {cid}")
        require(request["trace_id"] == acquire["trace_id"] == playbook_by_case[cid]["trace_id"],
                f"API trace identity incorrect for {cid}")
        require(request["end_ns"] - request["start_ns"] == 1_000_000_000,
                f"request duration incorrect for {cid}")
        expected_acquire = case_truth["acquire_ms"] * 1_000_000
        require(acquire["end_ns"] - acquire["start_ns"] == expected_acquire,
                f"pool acquisition duration incorrect for {cid}")

        child_records = [s for rec in dep for s in rec[4] if s["name"] == "downstream.call"]
        duration_ms = case_truth["downstream_ms"]
        require(len(child_records) == (0 if duration_ms is None else 1),
                f"downstream span population incorrect for {cid}")
        if child_records:
            child = child_records[0]
            require(child["parent_span_id"] == request["span_id"] and
                    child["trace_id"] == request["trace_id"],
                    f"downstream parent/trace identity incorrect for {cid}")
            require(child["end_ns"] - child["start_ns"] == duration_ms * 1_000_000,
                    f"downstream duration incorrect for {cid}")
            offset_ns = (0 if cid != "rca-06" else -2_000) * 1_000_000
            expected_start = (request["start_ns"] + expected_acquire + 10_000_000 + offset_ns)
            require(child["start_ns"] == expected_start, f"declared clock offset incorrect for {cid}")

        queries = playbook_by_case[cid]["queries"]
        require(len(queries) == 8, f"expected eight queries for {cid}")
        require({q["key"] for q in queries} ==
                {"logs", "coverage", "pool_use", "pool_capacity", "memory", "counter", "rate", "trace"},
                f"query keys incorrect for {cid}")

    # The registered reset fixture is checked from independently decoded points.
    reset_case = by_case["rca-03"]
    counter = next(m for m in reset_case[0][2] if m["name"] == "app.requests")
    points = counter["points"]
    require(len(points) == 3 and points[0]["value"] == 100 and points[1]["value"] == 160 and
            points[2]["value"] == 8, "counter fixture values changed")
    require((points[1]["value"] - points[0]["value"]) /
            ((points[1]["time_ns"] - points[0]["time_ns"]) / 1_000_000_000) == 6,
            "expected 6/s counter interval missing")
    require(points[2]["start_ns"] != points[1]["start_ns"], "counter reset start time missing")

    # rca-05 has two spans before the delayed Batch and three after inclusion.
    late = by_case["rca-05"]
    initial_count = sum(len(r[4]) for r in late if r[0]["stage"] == "initial")
    total_count = sum(len(r[4]) for r in late)
    require(initial_count == 2 and total_count == 3, "late trace population must be 2 then 3")
    return {"batches": len(producer), "cases": 7, "initial_batches": 14,
            "late_batches": 1, "late_case_initial_spans": initial_count,
            "late_case_including_late_spans": total_count}


def rejection_controls(producer, playbooks, truth, investigator_files, scenario_cases):
    controls = []

    def rejected(name, expected_reason, mutate):
        p, b, t, files = copy.deepcopy((producer, playbooks, truth, investigator_files))
        mutate(p, b, t, files)
        try:
            validate(p, b, t, files, scenario_cases)
        except (ValueError, KeyError, TypeError) as exc:
            reason = str(exc)
            if expected_reason not in reason:
                raise ValueError(f"{name} rejected for wrong reason: {reason}") from exc
            controls.append({"name": name, "rejected": True, "reason": reason})
        else:
            raise ValueError(f"rejection control was accepted: {name}")

    rejected("payload_digest_mutation", "payload digest mismatch",
             lambda p, *_: p[0].__setitem__("bytes", "AA=="))
    rejected("missing_initial_batch", "expected 15 producer Batches", lambda p, *_: p.pop(0))
    rejected("late_relabelled_initial", "expected fourteen initial Batches", lambda p, *_: next(
        row for row in p if row["stage"] == "late").__setitem__("stage", "initial"))
    rejected("truth_file_in_investigator_output", "investigator output must contain",
             lambda _p, _b, _t, files:
             files.__setitem__("controller-truth.json", "ground truth"))
    require(len(controls) == 4 and all(c["rejected"] for c in controls),
            "not all registered rejection controls rejected")
    return controls


def hash_tree(path):
    return {str(p.relative_to(path)): digest(p.read_bytes())
            for p in sorted(path.rglob("*")) if p.is_file()}


def cgroup_value(name):
    entry = next((line[3:] for line in Path("/proc/self/cgroup").read_text().splitlines()
                  if line.startswith("0::")), None)
    if entry is None:
        return None
    target = Path("/sys/fs/cgroup") / entry.lstrip("/") / name
    return target.read_text().strip() if target.is_file() else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    resource_group.require_limits()
    out = args.out.resolve()
    out.relative_to(EVIDENCE_PARENT.resolve())
    require(out != EVIDENCE_PARENT.resolve() and not out.exists(),
            "--out must be a fresh child of the registered query evidence directory")
    out.mkdir(mode=0o700, parents=True)
    packet = out / "packet"
    command = [sys.executable, "-B", str(HERE / "prepare.py"), "--evidence", str(packet)]
    started = time.monotonic()
    completed = None
    failure = None
    try:
        completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True,
                                   timeout=20, check=False)
        require(completed.returncode == 0,
                f"prepare.py exited {completed.returncode}: {completed.stderr[-2000:]}")
        producer, playbooks, truth = read_packet(packet)
        scenarios = json.loads((HERE / "scenarios.json").read_text())["cases"]
        manifest_path = packet / "controller/manifest.json"
        manifest = json.loads(manifest_path.read_text())
        for relative, expected_hash in manifest["files"].items():
            require(digest((packet / relative).read_bytes()) == expected_hash,
                    f"generator manifest file hash mismatch: {relative}")
        source_map = {
            "tools/bench/labs/rca/prepare.py": HERE / "prepare.py",
            "tools/bench/labs/rca/scenarios.json": HERE / "scenarios.json",
            "tools/qualification/test_query_oracle.py": ROOT / "tools/qualification/test_query_oracle.py",
            "tools/qualification/query_oracle.py": ROOT / "tools/qualification/query_oracle.py",
        }
        require(set(manifest["sources"]) == set(source_map), "generator manifest source set differs")
        for relative, source in source_map.items():
            require(digest(source.read_bytes()) == manifest["sources"][relative],
                    f"generator manifest source hash mismatch: {relative}")
        packet_investigator = packet / "investigator"
        investigator_files = {
            str(p.relative_to(packet_investigator)): p.read_bytes()
            for p in packet_investigator.rglob("*") if p.is_file()
        }
        summary = validate(producer, playbooks, truth, investigator_files, scenarios)
        controls = rejection_controls(producer, playbooks, truth, investigator_files, scenarios)
        for playbook in playbooks:
            for query_recipe in playbook["queries"]:
                query_oracle.validate_query(query_recipe["query"])
        summary["supported_queries_validated"] = 56

        # These deterministic receive times exist only in memory for oracle input;
        # they are never serialized or described as server observations.
        oracle_records = []
        record_by_case_stage = {}
        for index, row in enumerate(producer, 1):
            oracle_records.append({"label": row["label"], "received_ns": index,
                                  "bytes": row["bytes"]})
            record_by_case_stage.setdefault((row["case"], row["stage"]), []).append(
                {"label": row["label"], "received_ns": index, "bytes": row["bytes"]})
        rca05 = next(p for p in playbooks if p["case"] == "rca-05")
        trace_query = next(q["query"] for q in rca05["queries"] if q["key"] == "trace")
        initial_late_records = record_by_case_stage[("rca-05", "initial")]
        late_records = record_by_case_stage[("rca-05", "late")]
        before_late = query_oracle.expected(initial_late_records, trace_query)["rows"]
        after_late = query_oracle.expected(initial_late_records + late_records, trace_query)["rows"]
        require(len(before_late) == 2 and len(after_late) == 3,
                "query oracle late-span population must change from two to three")
        rate_query = next(q["query"] for q in next(p for p in playbooks if p["case"] == "rca-03")["queries"]
                          if q["key"] == "rate")
        rate_rows = query_oracle.expected(oracle_records, rate_query)["rows"]
        rate_rows = [r for r in rate_rows if r["node"] == "rca-03-api"]
        require(len(rate_rows) == 2 and rate_rows[0]["rate"] == 6.0 and
                rate_rows[0]["reset"] is False and rate_rows[1]["reset"] is True and
                rate_rows[1]["rate"] is None,
                "query oracle must return 6/s followed by reset")
        summary["oracle_rate_rows"] = rate_rows
        summary["late_trace_oracle_rows"] = {"before": len(before_late), "after": len(after_late)}

        # Preserve the exact verifier and optional coordinator wrapper used for this job.
        controller_dir = packet / "controller"
        verifier_copy = controller_dir / "verify_preparation.py"
        verifier_copy.write_bytes(Path(__file__).read_bytes())
        wrapper = HERE / "job.py"
        copied_sources = {"verify_preparation.py": digest(verifier_copy.read_bytes())}
        if wrapper.is_file():
            wrapper_copy = controller_dir / "job.py"
            wrapper_copy.write_bytes(wrapper.read_bytes())
            copied_sources["job.py"] = digest(wrapper_copy.read_bytes())
        (controller_dir / "validation_inputs.json").write_text(
            json.dumps({"sha256": copied_sources}, indent=2) + "\n")
        bundle = hash_tree(packet)
        evidence_bytes = sum(p.stat().st_size for p in out.rglob("*") if p.is_file())
        require(evidence_bytes <= MAX_EVIDENCE, "evidence exceeds 8 MiB reservation")
        status = "prepared_fixture_shape_validated_not_ingested"
    except (Exception, subprocess.TimeoutExpired) as exc:
        failure = f"{type(exc).__name__}: {exc}"
        summary, controls, bundle, evidence_bytes = None, [], hash_tree(out), None
        status = "failed"

    source_paths = [Path(__file__), HERE / "prepare.py", HERE / "scenarios.json",
                    ROOT / "tools/qualification/test_query_oracle.py",
                    ROOT / "tools/qualification/query_oracle.py", PROTOCOL]
    if (HERE / "job.py").is_file():
        source_paths.append(HERE / "job.py")
    usage = resource.getrusage(resource.RUSAGE_SELF)
    child_usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    report = {
        "status": status,
        "failure": failure,
        "command": command,
        "exit": 0 if status != "failed" else 1,
        "generator_exit": None if completed is None else completed.returncode,
        "elapsed_seconds": time.monotonic() - started,
        "stdout": "" if completed is None else completed.stdout[-2000:],
        "stderr": "" if completed is None else completed.stderr[-2000:],
        "summary": summary,
        "rejection_controls": controls,
        "source_sha256": {str(p.relative_to(ROOT)): digest(p.read_bytes()) for p in source_paths},
        "bundle_sha256": bundle,
        "storage_path": str(out),
        "scratch_path": os.environ.get("TMPDIR"),
        "evidence_bytes": evidence_bytes,
        "resource_observations": {
            "self_maxrss_platform_units": usage.ru_maxrss,
            "child_maxrss_platform_units": child_usage.ru_maxrss,
            "memory_max_bytes": cgroup_value("memory.max"),
            "memory_swap_max_bytes": cgroup_value("memory.swap.max"),
        },
        "synthetic_receive_times": "in-memory ordinal nanoseconds for oracle fixture checks only; no server observations",
        "server_started": False,
        "native_queries_executed": 0,
        "cleanup": "rejection mutations were in-memory; launcher owns removal of generator scratch",
    }
    (out / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(out / "result.json")
    return 0 if status != "failed" else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(f"preparation validator: {type(error).__name__}: {error}", file=sys.stderr)
        sys.exit(2)
