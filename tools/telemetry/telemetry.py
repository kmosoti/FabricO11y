"""Passive, local Codex runtime telemetry primitives (schema version 1)."""

from __future__ import annotations

from datetime import datetime
from decimal import Decimal
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import time
import uuid


_ENVELOPE_KEYS = {
    "schema_version", "event_id", "session_id", "turn_id", "operation_id",
    "type", "source", "observed_at", "data",
}
_SESSION_TYPES = {"session.started", "session.ended"}
_TURN_TYPES = {"turn.started", "turn.stop_requested", "turn.interrupted"}
_TOOL_TYPES = {"tool.started", "tool.finished"}
_EVENT_TYPES = _SESSION_TYPES | _TURN_TYPES | _TOOL_TYPES
_TIMESTAMP_RE = re.compile(
    r"^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})"
    r"(?:\.(\d+))?(Z|[+-]\d{2}:\d{2})$"
)
_MAX_EVENT_BYTES = 8192
_LOCK_TIMEOUT = 0.5


def _timestamp(value: object) -> tuple[datetime, Decimal]:
    """Validate the timestamp and preserve every fractional digit for ordering."""
    if not isinstance(value, str) or len(value) > 64:
        raise ValueError("observed_at must be a timezone-aware ISO timestamp")
    match = _TIMESTAMP_RE.fullmatch(value)
    if match is None:
        raise ValueError("observed_at must be a timezone-aware ISO timestamp")
    zone = match.group(8)
    if zone != "Z":
        offset_hour = int(zone[1:3])
        offset_minute = int(zone[4:6])
        if offset_hour > 23 or offset_minute > 59:
            raise ValueError("observed_at has an invalid UTC offset")
    try:
        # The regular expression limits spellings to the subset represented in
        # the schema. fromisoformat then checks calendar, clock, and offset ranges.
        parsed = datetime.fromisoformat(value[:-1] + "+00:00" if value.endswith("Z") else value)
    except ValueError as exc:
        raise ValueError("observed_at is not a valid timestamp") from exc
    if parsed.utcoffset() is None:
        raise ValueError("observed_at must include a timezone")
    # datetime keeps only microseconds. Compare whole seconds and the original
    # fraction separately so externally supplied finer timestamps remain ordered.
    return parsed.replace(microsecond=0), Decimal("0." + (match.group(7) or "0"))


def validate_event(event: object) -> None:
    """Validate an event against the exact v1 envelope and variant contract."""
    if not isinstance(event, dict) or set(event) != _ENVELOPE_KEYS:
        raise ValueError("event must have exactly the v1 envelope fields")
    if type(event["schema_version"]) is not int or event["schema_version"] != 1:
        raise ValueError("schema_version must be integer 1")

    for key in ("event_id", "session_id"):
        value = event[key]
        if not isinstance(value, str) or not value or len(value) > 256:
            raise ValueError(f"{key} must be a nonempty string of at most 256 characters")
    for key in ("turn_id", "operation_id"):
        value = event[key]
        if value is not None and (not isinstance(value, str) or not value or len(value) > 256):
            raise ValueError(f"{key} must be null or a nonempty string of at most 256 characters")

    if event["source"] != "runtime_observer":
        raise ValueError("source must be runtime_observer")
    _timestamp(event["observed_at"])
    kind = event["type"]
    if not isinstance(kind, str) or kind not in _EVENT_TYPES:
        raise ValueError("unsupported event type")
    data = event["data"]
    if not isinstance(data, dict):
        raise ValueError("data must be an object")

    if kind in _SESSION_TYPES:
        if event["turn_id"] is not None or event["operation_id"] is not None or data:
            raise ValueError("session events require null IDs and empty data")
    elif kind in _TURN_TYPES:
        if not isinstance(event["turn_id"], str) or event["operation_id"] is not None or data:
            raise ValueError("turn events require a turn ID, null operation ID, and empty data")
    else:
        if not isinstance(event["turn_id"], str) or not isinstance(event["operation_id"], str):
            raise ValueError("tool events require turn and operation IDs")
        expected = {"tool_name"} if kind == "tool.started" else {"tool_name", "exit_code"}
        if set(data) != expected:
            raise ValueError("tool event data has incorrect fields")
        name = data["tool_name"]
        if not isinstance(name, str) or not name or len(name) > 128:
            raise ValueError("tool_name must be a nonempty string of at most 128 characters")
        if kind == "tool.finished":
            code = data["exit_code"]
            if code is not None and type(code) is not int:
                raise ValueError("exit_code must be an integer or null")
    return None


def event_schema() -> dict:
    """Return the JSON Schema 2020-12 schema for all v1 event variants."""
    string_id = {"type": "string", "minLength": 1, "maxLength": 256}
    common = {
        "schema_version": {"const": 1},
        "event_id": dict(string_id),
        "session_id": dict(string_id),
        "turn_id": {"anyOf": [dict(string_id), {"type": "null"}]},
        "operation_id": {"anyOf": [dict(string_id), {"type": "null"}]},
        "source": {"const": "runtime_observer"},
        "observed_at": {
            "type": "string",
            "maxLength": 64,
            "pattern": r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$",
            "format": "date-time",
        },
    }
    variants = []
    for kind in sorted(_SESSION_TYPES | _TURN_TYPES | _TOOL_TYPES):
        props = dict(common)
        props["type"] = {"const": kind}
        if kind in _SESSION_TYPES:
            props["turn_id"] = {"type": "null"}
            props["operation_id"] = {"type": "null"}
            data = {"type": "object", "properties": {}, "required": [], "additionalProperties": False}
        elif kind in _TURN_TYPES:
            props["turn_id"] = dict(string_id)
            props["operation_id"] = {"type": "null"}
            data = {"type": "object", "properties": {}, "required": [], "additionalProperties": False}
        elif kind == "tool.started":
            props["turn_id"] = dict(string_id)
            props["operation_id"] = dict(string_id)
            data = {
                "type": "object",
                "properties": {"tool_name": {"type": "string", "minLength": 1, "maxLength": 128}},
                "required": ["tool_name"],
                "additionalProperties": False,
            }
        else:
            props["turn_id"] = dict(string_id)
            props["operation_id"] = dict(string_id)
            data = {
                "type": "object",
                "properties": {
                    "tool_name": {"type": "string", "minLength": 1, "maxLength": 128},
                    "exit_code": {"anyOf": [{"type": "integer"}, {"type": "null"}]},
                },
                "required": ["tool_name", "exit_code"],
                "additionalProperties": False,
            }
        props["data"] = data
        variants.append({
            "type": "object",
            "properties": props,
            "required": sorted(_ENVELOPE_KEYS),
            "additionalProperties": False,
        })
    return {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "oneOf": variants,
    }


_HOOK_TYPES = {
    "SessionStart": "session.started",
    "SessionEnd": "session.ended",
    "UserPromptSubmit": "turn.started",
    "Stop": "turn.stop_requested",
    "Interrupt": "turn.interrupted",
    "PreToolUse": "tool.started",
    "PostToolUse": "tool.finished",
}


def normalize_hook(payload: object, observed_at: str) -> dict | None:
    """Project a supported hook payload onto the minimal telemetry schema."""
    if not isinstance(payload, dict):
        raise ValueError("hook payload must be an object")
    hook = payload.get("hook_event_name")
    kind = _HOOK_TYPES.get(hook) if isinstance(hook, str) else None
    if kind is None:
        return None

    session = payload.get("session_id")
    if not isinstance(session, str) or not session or len(session) > 256:
        raise ValueError("supported hook requires a valid session_id")
    turn = None
    operation = None
    data = {}
    if kind in _TURN_TYPES or kind in _TOOL_TYPES:
        turn = payload.get("turn_id")
        if not isinstance(turn, str) or not turn or len(turn) > 256:
            raise ValueError("supported hook requires a valid turn_id")
    if kind in _TOOL_TYPES:
        operation = payload.get("tool_use_id")
        if not isinstance(operation, str) or not operation or len(operation) > 256:
            raise ValueError("tool hook requires a valid tool_use_id")
        name = payload.get("tool_name")
        if not isinstance(name, str) or not name or len(name) > 128:
            raise ValueError("tool hook requires a valid tool_name")
        data = {"tool_name": name}
        if kind == "tool.finished":
            response = payload.get("tool_response")
            code = response.get("exit_code") if isinstance(response, dict) else None
            if name != "Bash" or type(code) is not int:
                code = None
            data["exit_code"] = code

    result = {
        "schema_version": 1,
        "event_id": str(uuid.uuid4()),
        "session_id": session,
        "turn_id": turn,
        "operation_id": operation,
        "type": kind,
        "source": "runtime_observer",
        "observed_at": observed_at,
        "data": data,
    }
    validate_event(result)
    return result


def _journal_path(root: Path, session_id: str) -> Path:
    digest = hashlib.sha256(session_id.encode("utf-8")).hexdigest()
    return Path(root) / (digest + ".jsonl")


def _acquire_lock(lock_path: Path, *, shared: bool) -> int:
    """Open and acquire a flock without waiting longer than half a second."""
    fd = os.open(lock_path, os.O_CREAT | os.O_RDWR, 0o600)
    operation = fcntl.LOCK_SH if shared else fcntl.LOCK_EX
    deadline = time.monotonic() + _LOCK_TIMEOUT
    try:
        while True:
            try:
                fcntl.flock(fd, operation | fcntl.LOCK_NB)
                return fd
            except BlockingIOError:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("timed out acquiring telemetry journal lock")
                time.sleep(min(0.01, remaining))
    except BaseException:
        os.close(fd)
        raise


def _serialized_event(event: dict) -> bytes:
    try:
        raw = json.dumps(event, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    except (TypeError, UnicodeError, ValueError) as exc:
        raise ValueError("event cannot be serialized as UTF-8 JSON") from exc
    if len(raw) > _MAX_EVENT_BYTES:
        raise ValueError("serialized event exceeds 8192 bytes")
    return raw


def append_event(root: Path, event: dict) -> Path:
    """Validate and append one complete JSONL record under an exclusive lock."""
    validate_event(event)
    raw = _serialized_event(event) + b"\n"
    path = _journal_path(Path(root), event["session_id"])
    path.parent.mkdir(parents=True, exist_ok=True)
    lock_fd = _acquire_lock(Path(str(path) + ".lock"), shared=False)
    try:
        fd = os.open(path, os.O_CREAT | os.O_APPEND | os.O_WRONLY, 0o600)
        try:
            written = os.write(fd, raw)
            if written != len(raw):
                raise OSError("short write while appending telemetry event")
        finally:
            os.close(fd)
    finally:
        os.close(lock_fd)
    return path


def _reject_duplicate_keys(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON object key")
        result[key] = value
    return result


def read_events(path: Path) -> list[dict]:
    """Read and validate every complete record from a JSONL journal."""
    path = Path(path)
    if not path.exists():
        raise FileNotFoundError(path)
    lock_fd = _acquire_lock(Path(str(path) + ".lock"), shared=True)
    try:
        try:
            raw = path.read_bytes()
        except FileNotFoundError:
            raise
        except OSError:
            raise
    finally:
        os.close(lock_fd)

    if not raw:
        return []
    if not raw.endswith(b"\n"):
        raise ValueError("journal has a truncated final line")
    events = []
    session_id = None
    for line in raw.splitlines(keepends=True):
        if not line.endswith(b"\n") or len(line) - 1 > _MAX_EVENT_BYTES:
            raise ValueError("journal line is truncated or exceeds 8192 bytes")
        try:
            text = line[:-1].decode("utf-8")
            event = json.loads(text, object_pairs_hook=_reject_duplicate_keys)
            validate_event(event)
        except (UnicodeError, json.JSONDecodeError, RecursionError) as exc:
            raise ValueError("journal contains malformed JSON or UTF-8") from exc
        except ValueError as exc:
            raise ValueError("journal contains an invalid event") from exc
        if session_id is None:
            session_id = event["session_id"]
        elif event["session_id"] != session_id:
            raise ValueError("journal contains mixed session IDs")
        events.append(event)
    return events


def reduce_events(events) -> dict:
    """Reduce a single session's ordered event trace into monotonic state."""
    unique = []
    seen = {}
    session_id = None
    latest_dt = None
    latest_value = None
    session_ended = False
    turns = {}

    for event in events:
        validate_event(event)
        event_id = event["event_id"]
        previous = seen.get(event_id)
        if previous is not None:
            if previous != event:
                raise ValueError("event_id has conflicting event contents")
            continue
        seen[event_id] = event
        if session_id is None:
            session_id = event["session_id"]
        elif event["session_id"] != session_id:
            raise ValueError("events contain mixed session IDs")
        unique.append(event)

        observed = _timestamp(event["observed_at"])
        if latest_dt is None or observed > latest_dt:
            latest_dt = observed
            latest_value = event["observed_at"]

        kind = event["type"]
        if kind == "session.ended":
            session_ended = True
        elif kind in _TURN_TYPES or kind in _TOOL_TYPES:
            turn_id = event["turn_id"]
            turn = turns.setdefault(turn_id, {
                "stop_requested": False,
                "interrupted": False,
                "operations": {},
            })
            if kind == "turn.stop_requested":
                turn["stop_requested"] = True
            elif kind == "turn.interrupted":
                turn["interrupted"] = True
            elif kind in _TOOL_TYPES:
                operation_id = event["operation_id"]
                name = event["data"]["tool_name"]
                operation = turn["operations"].get(operation_id)
                if operation is None:
                    operation = {
                        "tool_name": name,
                        "started": False,
                        "finished": False,
                        "exit_code": None,
                    }
                    turn["operations"][operation_id] = operation
                elif operation["tool_name"] != name:
                    raise ValueError("operation changed tool_name")
                if kind == "tool.started":
                    operation["started"] = True
                else:
                    code = event["data"]["exit_code"]
                    known = operation["exit_code"]
                    if known is not None and code is not None and known != code:
                        raise ValueError("operation has conflicting finish outcomes")
                    operation["finished"] = True
                    if code is not None:
                        operation["exit_code"] = code

    return {
        "schema_version": 1,
        "session_id": session_id,
        "session_ended": session_ended,
        "last_observed_at": latest_value,
        "turns": turns,
        "recent_activity": unique[-50:],
    }
