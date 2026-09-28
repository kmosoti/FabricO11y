"""Independent phase-2 delivery/dedup oracle.

Checks a JSONL transcript of a small, explicitly non-OTLP test protocol
against the safety contract in docs/ALPHA.md and the D4 delivery rule in
docs/ALPHA-PLAN.md. The transcript format and rule list are specified in
DELIVERY_ORACLE.md, which lives next to this file.

This module knows nothing about any Rust parser, wire format, or server
implementation. Identity <-> bytes mapping comes only from the transcript's
own base64 bytes; no hidden decoding is performed.

CLI:
    python3 -B tools/qualification/delivery_oracle.py <transcript.jsonl>

Prints one JSON object to stdout and exits 0 (pass), 1 (violation) or
2 (malformed input). Importable as `check(lines) -> Verdict`.
"""

from __future__ import annotations

import base64
import binascii
import json
import re
import sys
from dataclasses import dataclass, field
from typing import Any, Dict, Iterable, List, NamedTuple, Optional, Tuple

U64_MAX = 2**64 - 1
MAX_LINE_BYTES = 2 * 1024 * 1024
MAX_LABEL_BYTES = 256
_NODE_ID_RE = re.compile(r"^[0-9a-f]{32}$")
_RESPONSE_KINDS = {"ack", "conflict", "gap", "unauthorized", "bad_request",
                    "too_large", "unavailable", "no_response"}
_KNOWN_TYPES = {"source", "attempt", "response", "fault", "node_state",
                 "recovered", "end"}


class MalformedTranscript(ValueError):
    """Raised for any schema-level defect. The CLI maps this to exit 2."""


def _no_dup_keys(pairs: List[Tuple[str, Any]]) -> Dict[str, Any]:
    seen: Dict[str, Any] = {}
    for key, value in pairs:
        if key in seen:
            raise MalformedTranscript(f"duplicate JSON key: {key!r}")
        seen[key] = value
    return seen


def _reject_constant(name: str) -> Any:
    raise MalformedTranscript(f"disallowed JSON constant: {name}")


def _require_keys(obj: Dict[str, Any], expected: set, kind: str) -> None:
    actual = set(obj.keys())
    if actual != expected:
        missing, extra = sorted(expected - actual), sorted(actual - expected)
        detail = []
        if missing:
            detail.append(f"missing {missing}")
        if extra:
            detail.append(f"unknown {extra}")
        raise MalformedTranscript(f"{kind} record field mismatch: {'; '.join(detail)}")


def _u64(name: str, value: Any) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise MalformedTranscript(f"{name} must be a JSON integer, got {value!r}")
    if value < 0 or value > U64_MAX:
        raise MalformedTranscript(f"{name} out of u64 range: {value!r}")
    return value


def _node_id(value: Any) -> str:
    if not isinstance(value, str) or not _NODE_ID_RE.fullmatch(value):
        raise MalformedTranscript(f"node_id must be 32 lowercase hex chars, got {value!r}")
    return value


def _bytes_field(name: str, value: Any) -> bytes:
    if not isinstance(value, str):
        raise MalformedTranscript(f"{name} must be a base64 string, got {value!r}")
    try:
        decoded = base64.b64decode(value, validate=True)
    except (binascii.Error, ValueError) as exc:
        raise MalformedTranscript(f"{name} is not valid base64: {exc}") from exc
    if len(decoded) == 0:
        raise MalformedTranscript(f"{name} decodes to empty bytes")
    return decoded


def _bool_field(name: str, value: Any) -> bool:
    if not isinstance(value, bool):
        raise MalformedTranscript(f"{name} must be a JSON boolean, got {value!r}")
    return value


def _label(value: Any) -> str:
    if not isinstance(value, str):
        raise MalformedTranscript(f"label must be a string, got {value!r}")
    if not (1 <= len(value.encode("utf-8")) <= MAX_LABEL_BYTES):
        raise MalformedTranscript("fault label must be 1..=256 UTF-8 bytes")
    return value


def _seq_set(value: Any) -> Tuple[int, ...]:
    if not isinstance(value, list):
        raise MalformedTranscript(f"retained_sequences must be a JSON array, got {value!r}")
    items = tuple(_u64("retained_sequences[]", item) for item in value)
    if len(set(items)) != len(items):
        raise MalformedTranscript("retained_sequences must not repeat a sequence")
    return items


class Identity(NamedTuple):
    node_id: str
    generation: int
    sequence: int

    @property
    def stream(self) -> Tuple[str, int]:
        return (self.node_id, self.generation)


def _identity(obj: Dict[str, Any]) -> Identity:
    return Identity(_node_id(obj["node_id"]), _u64("generation", obj["generation"]),
                     _u64("sequence", obj["sequence"]))


class SourceRec(NamedTuple):
    identity: Identity
    data: bytes
    order: int


class AttemptRec(NamedTuple):
    identity: Identity
    data: bytes
    injected_conflict: bool
    order: int


class ResponseRec(NamedTuple):
    identity: Identity
    kind: str
    committed_through: Optional[int]
    order: int


class FaultRec(NamedTuple):
    label: str
    order: int


class NodeStateRec(NamedTuple):
    stream: Tuple[str, int]
    ack_cursor: int
    retained: Tuple[int, ...]
    order: int


class RecoveredRec(NamedTuple):
    identity: Identity
    data: bytes
    order: int


@dataclass
class Transcript:
    sources: Dict[Identity, SourceRec] = field(default_factory=dict)
    attempts: List[AttemptRec] = field(default_factory=list)
    responses: List[ResponseRec] = field(default_factory=list)
    faults: List[FaultRec] = field(default_factory=list)
    node_states: List[NodeStateRec] = field(default_factory=list)
    recovered: List[RecoveredRec] = field(default_factory=list)


def _parse_record(obj: Dict[str, Any], order: int, t: Transcript) -> bool:
    """Parses one decoded JSON object into `t`. Returns True for `end`."""
    rtype = obj.get("type")
    if rtype not in _KNOWN_TYPES:
        raise MalformedTranscript(f"unknown or missing record type: {rtype!r}")

    if rtype == "source":
        _require_keys(obj, {"type", "node_id", "generation", "sequence", "bytes"}, "source")
        identity = _identity(obj)
        data = _bytes_field("bytes", obj["bytes"])
        if identity in t.sources:
            raise MalformedTranscript(f"duplicate source identity {identity}")
        t.sources[identity] = SourceRec(identity, data, order)
        return False

    if rtype == "attempt":
        _require_keys(obj, {"type", "node_id", "generation", "sequence", "bytes",
                             "injected_conflict"}, "attempt")
        data = _bytes_field("bytes", obj["bytes"])
        conflict = _bool_field("injected_conflict", obj["injected_conflict"])
        t.attempts.append(AttemptRec(_identity(obj), data, conflict, order))
        return False

    if rtype == "response":
        base = {"type", "node_id", "generation", "sequence", "kind"}
        kind = obj.get("kind")
        if kind not in _RESPONSE_KINDS:
            raise MalformedTranscript(f"unknown response kind: {kind!r}")
        _require_keys(obj, base | ({"committed_through"} if kind == "ack" else set()), "response")
        identity = _identity(obj)
        committed_through = (_u64("committed_through", obj["committed_through"])
                              if kind == "ack" else None)
        t.responses.append(ResponseRec(identity, kind, committed_through, order))
        return False

    if rtype == "fault":
        _require_keys(obj, {"type", "label"}, "fault")
        t.faults.append(FaultRec(_label(obj["label"]), order))
        return False

    if rtype == "node_state":
        _require_keys(obj, {"type", "node_id", "generation", "ack_cursor",
                             "retained_sequences"}, "node_state")
        stream = (_node_id(obj["node_id"]), _u64("generation", obj["generation"]))
        ack_cursor = _u64("ack_cursor", obj["ack_cursor"])
        t.node_states.append(NodeStateRec(stream, ack_cursor, _seq_set(obj["retained_sequences"]),
                                           order))
        return False

    if rtype == "recovered":
        _require_keys(obj, {"type", "node_id", "generation", "sequence", "bytes"}, "recovered")
        t.recovered.append(RecoveredRec(_identity(obj), _bytes_field("bytes", obj["bytes"]),
                                         order))
        return False

    _require_keys(obj, {"type"}, "end")  # rtype == "end"
    return True


def _parse_lines(lines: Iterable[str]) -> Transcript:
    t = Transcript()
    end_seen = False
    order = 0
    for raw in lines:
        stripped = raw.rstrip("\r\n") if raw.endswith(("\n", "\r")) else raw
        if stripped.strip() == "":
            continue  # a blank line is a harmless separator, never a record
        if len(stripped.encode("utf-8")) > MAX_LINE_BYTES:
            raise MalformedTranscript("transcript line exceeds 2 MiB")
        if end_seen:
            raise MalformedTranscript("record found after `end`")
        try:
            obj = json.loads(stripped, object_pairs_hook=_no_dup_keys,
                              parse_constant=_reject_constant)
        except MalformedTranscript:
            raise
        except (json.JSONDecodeError, ValueError) as exc:
            raise MalformedTranscript(f"invalid JSON: {exc}") from exc
        if not isinstance(obj, dict):
            raise MalformedTranscript("each record must be a JSON object")
        end_seen = _parse_record(obj, order, t)
        order += 1
    if not end_seen:
        raise MalformedTranscript("transcript is missing the `end` record")
    return t


@dataclass
class Verdict:
    passed: bool
    violations: List[Dict[str, str]]
    counts: Dict[str, Any]

    def to_json(self) -> Dict[str, Any]:
        return {"passed": self.passed, "violations": self.violations, "counts": self.counts}


def _add(violations: List[Dict[str, str]], rule: str, detail: str) -> None:
    violations.append({"rule": rule, "detail": detail})


def _by_kind(responses: List[ResponseRec]) -> Dict[str, int]:
    out: Dict[str, int] = {}
    for resp in responses:
        out[resp.kind] = out.get(resp.kind, 0) + 1
    return out


def _check_semantics(t: Transcript) -> Verdict:
    violations: List[Dict[str, str]] = []

    # Ack history per stream in transcript order, for ACKED-DURABLE and
    # NODE-RETAINS-UNACKED; the running max, for CONFLICT-NOT-REPLACED.
    ack_history: Dict[Tuple[str, int], List[Tuple[int, int]]] = {}
    for resp in t.responses:
        if resp.kind == "ack":
            ack_history.setdefault(resp.identity.stream, []).append(
                (resp.order, resp.committed_through))
    for entries in ack_history.values():
        entries.sort(key=lambda pair: pair[0])
    max_committed = {stream: max(v for _, v in entries)
                      for stream, entries in ack_history.items()}

    max_sourced: Dict[Tuple[str, int], int] = {}
    for identity in t.sources:
        max_sourced[identity.stream] = max(max_sourced.get(identity.stream, 0), identity.sequence)

    # --- IN-ORDER-ACK: non-decreasing, and never past what was sourced ---
    for stream, entries in ack_history.items():
        prev = 0
        for _, committed_through in entries:
            if committed_through < prev:
                _add(violations, "IN-ORDER-ACK",
                     f"stream {stream}: committed_through decreased from {prev} to "
                     f"{committed_through}")
            prev = max(prev, committed_through)
            bound = max_sourced.get(stream, 0)
            if committed_through > bound:
                _add(violations, "IN-ORDER-ACK",
                     f"stream {stream}: committed_through {committed_through} exceeds "
                     f"highest sourced sequence {bound}")

    # --- EXACT-RETRY ---
    for attempt in t.attempts:
        source = t.sources.get(attempt.identity)
        if source is None:
            _add(violations, "EXACT-RETRY",
                 f"attempt for {attempt.identity} has no matching source to verify against")
        elif not attempt.injected_conflict and attempt.data != source.data:
            _add(violations, "EXACT-RETRY",
                 f"attempt for {attempt.identity} bytes differ from source bytes "
                 "without injected_conflict")

    # --- CONFLICT-NOT-REPLACED ---
    for attempt in t.attempts:
        if not attempt.injected_conflict:
            continue
        source = t.sources.get(attempt.identity)
        if source is not None and attempt.data == source.data:
            continue  # marked conflicting but not actually different: nothing to enforce
        bound = max_committed.get(attempt.identity.stream, 0)
        if bound >= attempt.identity.sequence:
            _add(violations, "CONFLICT-NOT-REPLACED",
                 f"{attempt.identity}: conflicting bytes were committed/acked "
                 f"(committed_through={bound})")

    recovered_by_identity: Dict[Identity, List[RecoveredRec]] = {}
    for rec in t.recovered:
        recovered_by_identity.setdefault(rec.identity, []).append(rec)

    # --- ACKED-DURABLE ---
    for identity in t.sources:
        if identity.sequence <= max_committed.get(identity.stream, 0):
            if not recovered_by_identity.get(identity):
                _add(violations, "ACKED-DURABLE",
                     f"acked identity {identity} is missing from recovered records")

    # --- NO-DUPLICATE ---
    for identity, entries in recovered_by_identity.items():
        if len(entries) > 1:
            _add(violations, "NO-DUPLICATE",
                 f"identity {identity} appears {len(entries)} times in recovered records")

    # --- NO-FABRICATION ---
    for identity, entries in recovered_by_identity.items():
        source = t.sources.get(identity)
        if source is None:
            _add(violations, "NO-FABRICATION",
                 f"recovered identity {identity} has no matching source batch")
            continue
        for entry in entries:
            if entry.data != source.data:
                _add(violations, "NO-FABRICATION",
                     f"recovered identity {identity} bytes differ from source bytes")

    # --- NODE-RETAINS-UNACKED ---
    # Only sources the harness had already observed (order < ns.order) can be
    # judged retained-or-not at that snapshot; a batch sourced later simply
    # did not exist yet and says nothing about the node's behavior.
    sources_by_stream: Dict[Tuple[str, int], List[SourceRec]] = {}
    for rec in t.sources.values():
        sources_by_stream.setdefault(rec.identity.stream, []).append(rec)

    for ns in t.node_states:
        observed_max = max((v for order, v in ack_history.get(ns.stream, []) if order < ns.order),
                            default=0)
        if ns.ack_cursor > observed_max:
            _add(violations, "NODE-RETAINS-UNACKED",
                 f"stream {ns.stream}: ack_cursor {ns.ack_cursor} exceeds observed "
                 f"committed_through {observed_max} at that point")
        retained_set = set(ns.retained)
        for rec in sources_by_stream.get(ns.stream, []):
            if rec.order >= ns.order:
                continue  # not yet sourced as of this snapshot
            identity = rec.identity
            if identity.sequence > observed_max and identity.sequence not in retained_set:
                _add(violations, "NODE-RETAINS-UNACKED",
                     f"{identity}: not yet observed as acked but missing from node "
                     "retention (early forget)")

    counts = {
        "sources": len(t.sources), "attempts": len(t.attempts),
        "responses": len(t.responses), "responses_by_kind": _by_kind(t.responses),
        "faults": len(t.faults), "node_states": len(t.node_states),
        "recovered": len(t.recovered),
        "streams": len({identity.stream for identity in t.sources}),
    }
    return Verdict(passed=len(violations) == 0, violations=violations, counts=counts)


def check(lines: Iterable[str]) -> Verdict:
    return _check_semantics(_parse_lines(lines))


def main(argv: Optional[List[str]] = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    if len(argv) != 1:
        print(json.dumps({"error": "usage: delivery_oracle.py <transcript.jsonl>"}))
        return 2
    try:
        with open(argv[0], "r", encoding="utf-8") as handle:
            lines = handle.read().split("\n")
    except OSError as exc:
        print(json.dumps({"error": f"cannot read transcript: {exc}"}))
        return 2
    except UnicodeDecodeError as exc:
        print(json.dumps({"error": f"transcript is not valid UTF-8: {exc}"}))
        return 2
    try:
        verdict = check(lines)
    except MalformedTranscript as exc:
        print(json.dumps({"error": str(exc)}))
        return 2
    print(json.dumps(verdict.to_json()))
    return 0 if verdict.passed else 1


if __name__ == "__main__":
    sys.exit(main())
