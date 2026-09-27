#!/usr/bin/env python3
"""Read-only Codex hooks. All output on stdout is one hook response object."""

import json
from pathlib import Path
import shutil
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[2]


def session_start():
    try:
        state = (ROOT / "docs/CURRENT.md").read_text(encoding="utf-8")
    except OSError as error:
        return {"systemMessage": f"Cannot load Fabric O11y current state: {error}"}
    context = (
        "Fabric O11y: follow AGENTS.md and inspect the source for the current task. "
        "The blueprint describes proposals. Current repository notes follow:\n\n"
        + state[:8000]
    )
    if len(state) > 8000:
        context += "\n[Truncated: read docs/CURRENT.md for the full state.]"
    return {
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": context,
        }
    }


def stop(event):
    runtime = shutil.which("bun")
    checker = ROOT / "tools/docs/check.mjs"
    if runtime is None or not (ROOT / "tools/docs/node_modules").is_dir():
        return {
            "systemMessage": (
                "Fabric O11y documentation check skipped: Bun or documentation "
                "dependencies are unavailable. See docs/CONTRIBUTING.md; install "
                "with bun install --cwd tools/docs --frozen-lockfile."
            )
        }
    try:
        result = subprocess.run(
            [runtime, str(checker)],
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=20,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"systemMessage": f"Documentation check could not complete: {error}"}
    if result.returncode == 0:
        return {}
    details = (result.stdout + result.stderr).strip()[:4000]
    reason = (
        f"Fabric O11y documentation check failed (exit {result.returncode}):\n"
        f"{details}\n"
        "Run bun tools/docs/check.mjs. Repair failures within the current task, "
        "or report a pre-existing failure; preserve unrelated implementation work."
    )
    if event.get("stop_hook_active") is True:
        return {"systemMessage": reason + "\nNo further continuation requested."}
    return {"decision": "block", "reason": reason}


def main():
    try:
        event = json.load(sys.stdin)
        if not isinstance(event, dict):
            raise ValueError("hook input must be a JSON object")
    except (ValueError, UnicodeError) as error:
        print(f"Invalid hook input: {error}", file=sys.stderr)
        return 1
    name = event.get("hook_event_name")
    if name == "SessionStart":
        response = session_start()
    elif name == "Stop":
        response = stop(event)
    else:
        response = {}
    print(json.dumps(response))
    return 0


if __name__ == "__main__":
    sys.exit(main())
