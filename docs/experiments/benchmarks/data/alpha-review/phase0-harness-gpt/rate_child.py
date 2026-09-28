#!/usr/bin/env python3
"""Write one tiny rate fixture during the supervised invocation."""

import csv
import json
from pathlib import Path
import sys

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tools" / "alpha"))
from workload import SEEDS, records_at  # noqa: E402

mutation = sys.argv[1]
offers = [dict(item) for item in records_at(10, 0, SEEDS[0])]
if mutation == "skip":
    offers.pop(0)
elif mutation == "extra":
    offers.append(dict(offers[0]))
elif mutation == "wrong_node":
    offers[0]["node"] = 999
elif mutation == "wrong_kind":
    offers[0]["kind"] = "metric"
elif mutation == "wrong_entropy":
    item = next(item for item in offers if item["kind"] == "log"
                and item["scheduled_ms"] == 500)
    item["body"] = "S" * 512
elif mutation == "wrong_metric":
    item = next(item for item in offers if item["kind"] == "metric")
    item["value"] = (item["value"] + 1) % (2**64)
elif mutation == "extra_field":
    offers[0]["unregistered"] = "value"
elif mutation != "valid":
    raise ValueError(f"unknown mutation: {mutation}")

Path("offers.jsonl").write_text(
    "".join(json.dumps(item, sort_keys=True) + "\n" for item in offers))
with Path("offered.csv").open("w", newline="") as sink:
    writer = csv.writer(sink)
    writer.writerow(("second", "offered_logs", "offered_metrics",
                     "admitted", "committed", "backlog", "gaps"))
    writer.writerow((0, 20, 320, 340, 0, 340, 0))
