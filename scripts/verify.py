#!/usr/bin/env python3
"""Run the maintained FabricO11y repository-wide verification gate."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def run(command: list[str]) -> None:
    print(json.dumps({"run": command}), flush=True)
    subprocess.run(command, cwd=ROOT, check=True)


def verify_generated_segment_fixtures() -> None:
    with tempfile.TemporaryDirectory(prefix="fabrico11y-segments-") as temporary:
        generated = Path(temporary)
        run(
            [
                "cargo",
                "run",
                "-p",
                "fabric-segment",
                "--example",
                "generate-fixtures",
                "--quiet",
                "--",
                str(generated),
            ]
        )
        tracked = ROOT / "fixtures/segment-format/binary"
        for name in (
            "valid.hex",
            "corrupt-payload.hex",
            "truncated.hex",
            "missing-dictionary.hex",
        ):
            if (generated / name).read_bytes() != (tracked / name).read_bytes():
                raise RuntimeError(f"sealed segment fixture drift: {name}")


def main() -> int:
    run(["uv", "run", "--no-project", "scripts/validate_contracts.py"])
    run(["cargo", "fmt", "--all", "--", "--check"])
    run(["cargo", "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"])
    run(["cargo", "test", "--workspace", "--all-targets"])
    verify_generated_segment_fixtures()
    run(["cargo", "run", "-p", "fabric-segment", "--example", "compression-smoke", "--quiet"])
    run([sys.executable, "scripts/verify_python_sdk.py"])
    run(["git", "diff", "--check"])
    print(json.dumps({"status": "ok", "gate": "FabricO11y MVP"}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
