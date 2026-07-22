#!/usr/bin/env python3
"""Build, install, import, and parity-test the PyO3 wheel in a disposable environment."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def run(command: list[str], *, env: dict[str, str] | None = None) -> None:
    subprocess.run(command, cwd=ROOT, env=env, check=True)


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="fabrico11y-wheel-") as temporary:
        temporary_path = Path(temporary)
        wheel_directory = temporary_path / "wheel"
        environment_directory = temporary_path / "environment"
        wheel_directory.mkdir()

        run(
            [
                "uvx",
                "--from",
                "maturin==1.14.1",
                "maturin",
                "build",
                "--release",
                "--out",
                str(wheel_directory),
                "--interpreter",
                sys.executable,
            ]
        )
        wheels = sorted(wheel_directory.glob("fabrico11y-*.whl"))
        if len(wheels) != 1:
            raise RuntimeError(f"expected one wheel, found {len(wheels)}")

        run(["uv", "venv", "--python", sys.executable, str(environment_directory)])
        python = environment_directory / "bin" / "python"
        run(["uv", "pip", "install", "--python", str(python), str(wheels[0])])
        run(
            [
                str(python),
                "-I",
                "-c",
                "import fabrico11y; assert fabrico11y.__version__ == '0.1.0'",
            ]
        )
        run(["cargo", "build", "-p", "fabric-sdk", "--bin", "fabricctl"])
        env = os.environ.copy()
        env["FABRICO11Y_REPO_ROOT"] = str(ROOT)
        env["FABRICCTL"] = str(ROOT / "target/debug/fabricctl")
        run(
            [
                str(python),
                "-I",
                "-m",
                "unittest",
                "discover",
                "-s",
                "tests/python",
                "-v",
            ],
            env=env,
        )

        print(
            json.dumps(
                {
                    "wheel": wheels[0].name,
                    "python": sys.version.split()[0],
                    "clean_import": True,
                    "rust_python_parity": True,
                },
                sort_keys=True,
            )
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
