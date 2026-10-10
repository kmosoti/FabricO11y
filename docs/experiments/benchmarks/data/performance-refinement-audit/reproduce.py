#!/usr/bin/env python3
"""Reproduce the recorded rate defects; exit 1 means disagreement with the oracle.

This diagnostic is not a verification gate. Run at the recorded source revision,
with Cargo on PATH and dependencies available offline. Builds use the caller's
CARGO_TARGET_DIR when set; temporary source files are removed automatically.
"""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def main():
    here = Path(__file__).resolve().parent
    repo = here.parents[4]
    extraction = json.loads((here / "extraction.json").read_text())
    query = (repo / "crates/fabric-server/src/query.rs").read_text()
    function = query[query.index("fn rates(mut points:"):].strip() + "\n"
    if hashlib.sha256(function.encode()).hexdigest() != extraction["function_sha256"]:
        raise ValueError("rate source changed; use recorded revision for this diagnostic")
    rust = (here / "rate-probe.rs").read_bytes()
    if hashlib.sha256(rust).hexdigest() != extraction["source_sha256"]:
        raise ValueError("recorded Rust probe changed")
    sys.path.insert(0, str(repo / "tools/qualification"))
    import query_oracle

    with tempfile.TemporaryDirectory(prefix="fabric-rate-audit-") as directory:
        work = Path(directory)
        (work / "src").mkdir()
        (work / "src/main.rs").write_bytes(rust)
        (work / "Cargo.lock").write_bytes((here / "Cargo.lock").read_bytes())
        core = json.dumps(str(repo / "crates/fabric-core"))
        (work / "Cargo.toml").write_text(
            '[package]\nname="fabric-rate-audit"\nversion="0.0.0"\nedition="2024"\n'
            '[dependencies]\nserde_json="=1.0.150"\n'
            f'fabric-core={{path={core}}}\n'
        )
        result = subprocess.run(
            ["cargo", "run", "--locked", "--offline", "--quiet",
             "--manifest-path", str(work / "Cargo.toml")],
            check=True, capture_output=True, text=True, env=os.environ.copy(),
        )
        actual = json.loads(result.stdout)
    fixture = json.loads((here / "comparison.json").read_text())
    outcomes = {}
    for name, case in fixture["cases"].items():
        materialized = query_oracle.MaterializedRecord(
            label="node", node_id="00" * 16, sequence=1,
            received_ns=2_000_000_000, metric_points=case["input_points"],
        )
        expected = query_oracle._expected_rate_rows([materialized], fixture["query"])
        outcomes[name] = {
            "expected": expected, "actual": actual[name],
            "equal": expected == actual[name],
        }
    print(json.dumps(outcomes, indent=2))
    return 0 if all(case["equal"] for case in outcomes.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
