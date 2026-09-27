#!/usr/bin/env python3
"""Passive hook sink and explicit JSON replay command; no third-party packages."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import sys

import telemetry

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_STORE = ROOT / ".local" / "codex-telemetry"
MAX_INPUT_BYTES = 1_048_576


def timestamp():
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def diagnostic(root, error):
    """Best effort; never echo a payload or an exception message into context."""
    try:
        root.mkdir(parents=True, exist_ok=True)
        record = json.dumps({"observed_at": timestamp(), "error": type(error).__name__}) + "\n"
        # One small append. Diagnostics are advisory, not part of the event journal.
        fd = os.open(root / "observer-errors.log", os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
        try:
            os.write(fd, record.encode("utf-8"))
        finally:
            os.close(fd)
    except Exception:
        pass


def observe(root):
    try:
        observed_at = timestamp()
        raw = sys.stdin.buffer.read(MAX_INPUT_BYTES + 1)
        if len(raw) > MAX_INPUT_BYTES:
            raise ValueError("hook input too large")
        payload = json.loads(raw, object_pairs_hook=unique_object)
        event = telemetry.normalize_hook(payload, observed_at)
        if event is not None:
            telemetry.append_event(root, event)
    except Exception as error:
        diagnostic(root, error)
    # Hooks never return additionalContext, decisions, or status messages.
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    observer = commands.add_parser("observe", help="silently record a hook payload from stdin")
    observer.add_argument("--root", type=Path, default=DEFAULT_STORE)
    snapshot = commands.add_parser("snapshot", help="replay one session as JSON; errors are explicit")
    snapshot.add_argument("--root", type=Path, default=DEFAULT_STORE)
    snapshot.add_argument("--session", required=True)
    commands.add_parser("schema", help="print the event JSON Schema")
    args = parser.parse_args()
    if args.command == "observe":
        return observe(args.root)
    try:
        if args.command == "schema":
            value = telemetry.event_schema()
        else:
            name = hashlib.sha256(args.session.encode("utf-8")).hexdigest() + ".jsonl"
            value = telemetry.reduce_events(telemetry.read_events(args.root / name))
            if value["session_id"] not in (None, args.session):
                raise ValueError("journal identity does not match requested session")
        print(json.dumps(value, indent=2, ensure_ascii=True, allow_nan=False))
    except (OSError, ValueError, TimeoutError) as error:
        print(f"Telemetry read failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
