#!/usr/bin/env python3
"""Phase-4 retained-history query oracle.

Implementation-independent, exact-scan checker for the query contract in
docs/architecture/retained-history.md. It decodes the Fabric `Batch`
protobuf itself with a minimal hand-written wire-format decoder (see
``parse_fields`` below) and never imports code from this repository's Rust
crates or any other implementation. Field numbers are hard coded from the
vendored ``opentelemetry-proto`` crate sources and ``src/alpha/journal.rs``;
see QUERY_ORACLE.md for the exact numbers used and their provenance.

CLI:
    python3 -B tools/alpha/query_oracle.py --records R --query Q --answer A \
        [--unavailable U]

R is a JSONL file of records (see ``load_records_jsonl``), Q and A and U are
JSON files (a query object, an answer -- a JSON array of pages -- and an
optional unavailable-declaration array). Prints one JSON verdict:

    {"passed": bool, "violations": [{"rule": str, "detail": str}, ...],
     "expected_rows": int|None, "answered_rows": int|None}

Exit 0 pass, 1 fail (a rule was violated), 2 malformed input (see
QUERY_ORACLE.md for the exact malformed-input list; this follows the same
strict-schema convention as tools/alpha/DELIVERY_ORACLE.md: unknown fields,
duplicate JSON keys, wrong fixed types, and NaN/Infinity are all malformed).

Importable API: ``expected(records, query, unavailable=None)`` and
``check(records, query, pages, unavailable=None)``. Both take already
JSON-decoded Python objects in the exact shapes described in
QUERY_ORACLE.md (the same shapes the CLI reads from files), not file paths.

Rule IDs (see QUERY_ORACLE.md for the full description of each):
    ROW-DROPPED, ROW-DUPLICATED, ROW-ORDER, ROW-CONTENT,
    PAGE-LIMIT, PAGE-NEXT, PAGE-SNAPSHOT, ENVELOPE-CONSISTENT,
    RETAINED-WINDOW, FRESHNESS, GAPS, COMPLETE, UNAVAILABLE-SHAPE,
    RATE-VALUE, RATE-RESET, MALFORMED.
"""

from __future__ import annotations

import argparse
import base64
import json
import math
import struct
import sys
from collections import Counter
from dataclasses import dataclass, field
from typing import Any, Optional


# --------------------------------------------------------------------------
# Errors
# --------------------------------------------------------------------------


class MalformedInput(Exception):
    """Raised for any structurally invalid input (see module docstring)."""


class ProtoError(MalformedInput):
    """Raised when the hand-written protobuf decoder cannot parse bytes."""


# --------------------------------------------------------------------------
# Minimal protobuf wire-format decoder (no implementation code imported)
# --------------------------------------------------------------------------
#
# Field numbers used below, verified against the vendored
# opentelemetry-proto-0.33.0 crate sources and src/alpha/journal.rs (see
# QUERY_ORACLE.md "Field numbers"). Unknown fields are ignored, as required.


def _read_varint(buf: bytes, pos: int) -> tuple[int, int]:
    result = 0
    shift = 0
    n = len(buf)
    while True:
        if pos >= n:
            raise ProtoError("truncated varint")
        b = buf[pos]
        pos += 1
        result |= (b & 0x7F) << shift
        if not (b & 0x80):
            return result, pos
        shift += 7
        if shift > 70:
            raise ProtoError("varint too long")


def parse_fields(buf: bytes) -> dict[int, list[tuple[int, Any]]]:
    """Decode a protobuf message into {field_number: [(wire_type, value), ...]}.

    value is: int for wire type 0 (varint), 8 raw bytes for wire type 1
    (fixed64), raw bytes for wire type 2 (length-delimited), 4 raw bytes for
    wire type 5 (fixed32). Wire types 3/4 (deprecated groups) are rejected:
    they cannot be skipped without knowing their schema.
    """
    fields: dict[int, list[tuple[int, Any]]] = {}
    pos = 0
    n = len(buf)
    while pos < n:
        tag, pos = _read_varint(buf, pos)
        field_no = tag >> 3
        wire_type = tag & 0x7
        if wire_type == 0:
            val, pos = _read_varint(buf, pos)
        elif wire_type == 1:
            if pos + 8 > n:
                raise ProtoError("truncated fixed64")
            val = buf[pos : pos + 8]
            pos += 8
        elif wire_type == 2:
            ln, pos = _read_varint(buf, pos)
            if pos + ln > n:
                raise ProtoError("truncated length-delimited field")
            val = buf[pos : pos + ln]
            pos += ln
        elif wire_type == 5:
            if pos + 4 > n:
                raise ProtoError("truncated fixed32")
            val = buf[pos : pos + 4]
            pos += 4
        else:
            raise ProtoError(f"unsupported wire type {wire_type} (field {field_no})")
        fields.setdefault(field_no, []).append((wire_type, val))
    return fields


def _one(fields: dict, num: int, default=None):
    vals = fields.get(num)
    if not vals:
        return default
    return vals[-1][1]  # proto3: last occurrence wins for a singular field


def _many(fields: dict, num: int) -> list:
    return [v for _wt, v in fields.get(num, [])]


def _as_string(raw: Optional[bytes]) -> str:
    if raw is None:
        return ""
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError as e:
        raise ProtoError(f"invalid utf-8 string: {e}") from e


def _as_double(raw8: Optional[bytes]) -> float:
    if raw8 is None:
        return 0.0
    return struct.unpack("<d", raw8)[0]


def _as_sfixed64(raw8: Optional[bytes]) -> int:
    if raw8 is None:
        return 0
    return struct.unpack("<q", raw8)[0]


def _as_fixed64_uint(raw8: Optional[bytes]) -> int:
    if raw8 is None:
        return 0
    return struct.unpack("<Q", raw8)[0]


def _varint_to_int64(v: Optional[int]) -> int:
    if v is None:
        return 0
    v &= (1 << 64) - 1
    if v >= 1 << 63:
        v -= 1 << 64
    return v


# ---- domain messages ------------------------------------------------------


def decode_batch(raw: bytes) -> dict:
    f = parse_fields(raw)
    return {
        "version": _one(f, 1, 0),
        "node_id": _one(f, 2, b""),
        "generation": _one(f, 3, 0),
        "sequence": _one(f, 4, 0),
        "metrics_bytes": _one(f, 5, b""),
        "logs_bytes": _one(f, 6, b""),
        "gaps": [_as_string(g) for g in _many(f, 8)],
    }


def decode_any_value(raw: Optional[bytes]):
    """Returns (kind, value) where kind is one of string/bool/int/double/bytes/none."""
    if raw is None:
        return ("none", None)
    f = parse_fields(raw)
    if 1 in f:
        return ("string", _as_string(_one(f, 1)))
    if 2 in f:
        return ("bool", bool(_one(f, 2)))
    if 3 in f:
        return ("int", _varint_to_int64(_one(f, 3)))
    if 4 in f:
        return ("double", _as_double(_one(f, 4)))
    if 7 in f:
        return ("bytes", _one(f, 7))
    return ("none", None)


def decode_key_value(raw: bytes) -> tuple[str, str, Any]:
    f = parse_fields(raw)
    key = _as_string(_one(f, 1, b""))
    kind, value = decode_any_value(_one(f, 2))
    return key, kind, value


def _string_attributes(raw_list: list[bytes]) -> dict[str, str]:
    """Only string-valued attributes are kept; see QUERY_ORACLE.md ambiguity #2."""
    attrs: dict[str, str] = {}
    for raw in raw_list:
        key, kind, value = decode_key_value(raw)
        if kind == "string":
            attrs[key] = value
    return attrs


def decode_log_record(raw: bytes) -> dict:
    f = parse_fields(raw)
    observed = _as_fixed64_uint(_one(f, 11, b"\x00" * 8))
    body_raw = _one(f, 5)
    body = ""
    if body_raw is not None:
        kind, value = decode_any_value(body_raw)
        if kind == "string":
            body = value
        # non-string body: treated as empty string; see QUERY_ORACLE.md ambiguity #1
    attrs = _string_attributes(_many(f, 6))
    return {"observed_time_unix_nano": observed, "body": body, "attributes": attrs}


def decode_logs_request(raw: bytes) -> list[dict]:
    if not raw:
        return []
    f = parse_fields(raw)
    out = []
    for rl_raw in _many(f, 1):
        rl = parse_fields(rl_raw)
        for sl_raw in _many(rl, 2):
            sl = parse_fields(sl_raw)
            for lr_raw in _many(sl, 2):
                out.append(decode_log_record(lr_raw))
    return out


def decode_number_data_point(raw: bytes) -> dict:
    f = parse_fields(raw)
    start_ns = _as_fixed64_uint(_one(f, 2, b"\x00" * 8))
    time_ns = _as_fixed64_uint(_one(f, 3, b"\x00" * 8))
    attrs = _string_attributes(_many(f, 7))
    if 4 in f:
        value: Any = _as_double(_one(f, 4))
    elif 6 in f:
        value = _as_sfixed64(_one(f, 6))
    else:
        value = 0  # neither oneof arm set; see QUERY_ORACLE.md ambiguity #3
    return {"start_ns": start_ns, "time_ns": time_ns, "attributes": attrs, "value": value}


def decode_metric(raw: bytes) -> Optional[dict]:
    f = parse_fields(raw)
    name = _as_string(_one(f, 1, b""))
    unit = _as_string(_one(f, 3, b""))
    if 5 in f:
        g = parse_fields(_one(f, 5))
        points = [decode_number_data_point(p) for p in _many(g, 1)]
        return {"name": name, "unit": unit, "kind": "gauge", "monotonic": None, "points": points}
    if 7 in f:
        s = parse_fields(_one(f, 7))
        monotonic = bool(_one(s, 3, 0))
        points = [decode_number_data_point(p) for p in _many(s, 1)]
        return {"name": name, "unit": unit, "kind": "sum", "monotonic": monotonic, "points": points}
    return None  # histogram/exponential_histogram/summary: excluded, see QUERY_ORACLE.md ambiguity #4


def decode_metrics_request(raw: bytes) -> list[dict]:
    if not raw:
        return []
    f = parse_fields(raw)
    out = []
    for rm_raw in _many(f, 1):
        rm = parse_fields(rm_raw)
        for sm_raw in _many(rm, 2):
            sm = parse_fields(sm_raw)
            for m_raw in _many(sm, 2):
                m = decode_metric(m_raw)
                if m is not None:
                    out.append(m)
    return out


# --------------------------------------------------------------------------
# Records -> rows
# --------------------------------------------------------------------------


@dataclass
class MaterializedRecord:
    label: str
    node_id: str
    sequence: int
    received_ns: int
    log_rows: list = field(default_factory=list)
    metric_points: list = field(default_factory=list)
    gaps: list = field(default_factory=list)


def materialize_record(label: str, received_ns: int, batch_bytes: bytes) -> MaterializedRecord:
    batch = decode_batch(batch_bytes)
    node_id = batch["node_id"].hex()
    sequence = batch["sequence"]

    log_rows = []
    for idx, lr in enumerate(decode_logs_request(batch["logs_bytes"])):
        log_rows.append(
            {
                "node": label,
                "node_id": node_id,
                "sequence": sequence,
                "index": idx,
                "observed_ns": lr["observed_time_unix_nano"],
                "body": lr["body"],
                "attributes": lr["attributes"],
            }
        )

    metric_points = []
    counter = 0
    for m in decode_metrics_request(batch["metrics_bytes"]):
        for p in m["points"]:
            row = {
                "node": label,
                "node_id": node_id,
                "sequence": sequence,
                "index": counter,
                "name": m["name"],
                "unit": m["unit"],
                "kind": m["kind"],
                "time_ns": p["time_ns"],
                "start_ns": p["start_ns"] if m["kind"] == "sum" else 0,
                "value": p["value"],
                "attributes": p["attributes"],
            }
            if m["kind"] == "sum":
                row["monotonic"] = m["monotonic"]
            metric_points.append(row)
            counter += 1

    gaps = [
        {"node": label, "sequence": sequence, "receive_ns": received_ns, "gap": g}
        for g in batch["gaps"]
    ]

    return MaterializedRecord(
        label=label,
        node_id=node_id,
        sequence=sequence,
        received_ns=received_ns,
        log_rows=log_rows,
        metric_points=metric_points,
        gaps=gaps,
    )


# --------------------------------------------------------------------------
# Strict JSON loading (duplicate keys / NaN / unknown fields are malformed)
# --------------------------------------------------------------------------


def _no_duplicate_keys(pairs):
    d: dict = {}
    for k, v in pairs:
        if k in d:
            raise MalformedInput(f"duplicate JSON key {k!r}")
        d[k] = v
    return d


def _reject_constant(name):
    raise MalformedInput(f"invalid JSON constant {name}")


def _loads_strict(text: str):
    try:
        return json.loads(text, object_pairs_hook=_no_duplicate_keys, parse_constant=_reject_constant)
    except MalformedInput:
        raise
    except json.JSONDecodeError as e:
        raise MalformedInput(f"invalid JSON: {e}") from e


def load_json_file(path: str):
    with open(path, "r", encoding="utf-8") as fh:
        return _loads_strict(fh.read())


def load_records_jsonl(path: str) -> list[dict]:
    records = []
    with open(path, "r", encoding="utf-8") as fh:
        for line_no, line in enumerate(fh, 1):
            line = line.strip()
            if not line:
                continue
            try:
                obj = _loads_strict(line)
            except MalformedInput as e:
                raise MalformedInput(f"records line {line_no}: {e}") from e
            records.append(obj)
    return records


def _check_no_unknown_keys(obj: dict, allowed: set, ctx: str) -> None:
    extra = set(obj.keys()) - allowed
    if extra:
        raise MalformedInput(f"{ctx}: unknown field(s) {sorted(extra)!r}")


def _check_type(obj: dict, key: str, typ, ctx: str, required: bool = True):
    if key not in obj:
        if required:
            raise MalformedInput(f"{ctx}: missing field {key!r}")
        return
    v = obj[key]
    ok = True
    if typ is int:
        ok = isinstance(v, int) and not isinstance(v, bool)
    elif typ is bool:
        ok = isinstance(v, bool)
    elif typ is str:
        ok = isinstance(v, str)
    elif typ is dict:
        ok = isinstance(v, dict)
    elif typ is list:
        ok = isinstance(v, list)
    if not ok:
        raise MalformedInput(f"{ctx}: field {key!r} must be {typ.__name__}")


# --------------------------------------------------------------------------
# records[] -> materialized records
# --------------------------------------------------------------------------

_RECORD_ALLOWED = {"label", "received_ns", "bytes"}


def materialize_all(records: Any) -> list[MaterializedRecord]:
    if not isinstance(records, list):
        raise MalformedInput("records must be a JSON list")
    out = []
    for i, obj in enumerate(records):
        ctx = f"records[{i}]"
        if not isinstance(obj, dict):
            raise MalformedInput(f"{ctx}: must be an object")
        _check_no_unknown_keys(obj, _RECORD_ALLOWED, ctx)
        _check_type(obj, "label", str, ctx)
        _check_type(obj, "received_ns", int, ctx)
        _check_type(obj, "bytes", str, ctx)
        try:
            raw = base64.b64decode(obj["bytes"], validate=True)
        except Exception as e:
            raise MalformedInput(f"{ctx}: invalid base64: {e}") from e
        try:
            mat = materialize_record(obj["label"], obj["received_ns"], raw)
        except ProtoError as e:
            raise MalformedInput(f"{ctx}: protobuf decode error: {e}") from e
        out.append(mat)
    return out


# --------------------------------------------------------------------------
# unavailable[] -> exclusion specs
# --------------------------------------------------------------------------


def validate_unavailable(raw: Any) -> list[dict]:
    if not isinstance(raw, list):
        raise MalformedInput("unavailable must be a JSON list")
    specs = []
    for i, item in enumerate(raw):
        ctx = f"unavailable[{i}]"
        if not isinstance(item, dict):
            raise MalformedInput(f"{ctx}: must be an object")
        if set(item.keys()) == {"node", "sequence"}:
            _check_type(item, "node", str, ctx)
            _check_type(item, "sequence", int, ctx)
            specs.append({"type": "record", "node": item["node"], "sequence": item["sequence"]})
        elif set(item.keys()) == {"from_ns", "to_ns"}:
            _check_type(item, "from_ns", int, ctx)
            _check_type(item, "to_ns", int, ctx)
            specs.append({"type": "range", "from_ns": item["from_ns"], "to_ns": item["to_ns"]})
        else:
            raise MalformedInput(
                f"{ctx}: must have exactly {{node, sequence}} or exactly {{from_ns, to_ns}}"
            )
    return specs


def _is_unavailable(m: MaterializedRecord, specs: list[dict]) -> bool:
    for spec in specs:
        if spec["type"] == "record" and spec["node"] == m.label and spec["sequence"] == m.sequence:
            return True
        if spec["type"] == "range" and spec["from_ns"] <= m.received_ns < spec["to_ns"]:
            return True
    return False


# --------------------------------------------------------------------------
# Query validation
# --------------------------------------------------------------------------

_QUERY_COMMON = {"kind", "from_ns", "to_ns", "node"}
_QUERY_ALLOWED = {
    "logs": _QUERY_COMMON | {"contains", "limit", "page"},
    "metrics": _QUERY_COMMON | {"name", "limit", "page"},
    "rate": _QUERY_COMMON | {"name"},
}


def validate_query(query: Any) -> None:
    if not isinstance(query, dict):
        raise MalformedInput("query must be a JSON object")
    kind = query.get("kind")
    if kind not in ("logs", "metrics", "rate"):
        raise MalformedInput(f"query.kind must be logs|metrics|rate, got {kind!r}")
    _check_no_unknown_keys(query, _QUERY_ALLOWED[kind], "query")
    _check_type(query, "from_ns", int, "query")
    _check_type(query, "to_ns", int, "query")
    if query["from_ns"] > query["to_ns"]:
        raise MalformedInput("query.from_ns must be <= query.to_ns")
    _check_type(query, "node", str, "query", required=False)
    if kind in ("metrics", "rate"):
        _check_type(query, "name", str, "query")
    if kind in ("logs", "metrics"):
        _check_type(query, "limit", int, "query")
        if not (1 <= query["limit"] <= 10000):
            raise MalformedInput("query.limit must be in [1, 10000]")
        _check_type(query, "page", str, "query", required=False)
    if kind == "logs":
        _check_type(query, "contains", str, "query", required=False)


# --------------------------------------------------------------------------
# Full-scan expectation
# --------------------------------------------------------------------------


def _expected_log_rows(mats: list[MaterializedRecord], query: dict) -> list[dict]:
    node = query.get("node")
    contains = query.get("contains")
    from_ns, to_ns = query["from_ns"], query["to_ns"]
    rows = []
    for m in mats:
        if node is not None and m.label != node:
            continue
        for r in m.log_rows:
            if not (from_ns <= r["observed_ns"] < to_ns):
                continue
            if contains is not None and contains not in r["body"]:
                continue
            rows.append(r)
    rows.sort(key=lambda r: (r["observed_ns"], r["node_id"], r["sequence"], r["index"]))
    return rows


def _expected_metric_rows(mats: list[MaterializedRecord], query: dict) -> list[dict]:
    node = query.get("node")
    name = query["name"]
    from_ns, to_ns = query["from_ns"], query["to_ns"]
    rows = []
    for m in mats:
        if node is not None and m.label != node:
            continue
        for r in m.metric_points:
            if r["name"] != name:
                continue
            if not (from_ns <= r["time_ns"] < to_ns):
                continue
            rows.append(r)
    rows.sort(key=lambda r: (r["time_ns"], r["node_id"], r["sequence"], r["index"]))
    return rows


def _expected_rate_rows(mats: list[MaterializedRecord], query: dict) -> list[dict]:
    node = query.get("node")
    name = query["name"]
    from_ns, to_ns = query["from_ns"], query["to_ns"]
    series: dict[tuple, list[dict]] = {}
    for m in mats:
        if node is not None and m.label != node:
            continue
        for r in m.metric_points:
            if r["name"] != name or r["kind"] != "sum" or not r.get("monotonic"):
                continue
            if not (from_ns <= r["time_ns"] < to_ns):
                continue
            key = (r["node"], tuple(sorted(r["attributes"].items())))
            series.setdefault(key, []).append(r)
    out = []
    for key in sorted(series.keys(), key=lambda k: (k[0], k[1])):
        pts = sorted(series[key], key=lambda r: r["time_ns"])
        for prev, cur in zip(pts, pts[1:]):
            # dt<=0 (equal/duplicate timestamps) is treated as a reset too;
            # see QUERY_ORACLE.md ambiguity #6.
            reset = (
                prev["start_ns"] != cur["start_ns"]
                or cur["value"] < prev["value"]
                or cur["time_ns"] <= prev["time_ns"]
            )
            row: dict[str, Any] = {
                "node": key[0],
                "name": name,
                "attributes": dict(key[1]),
                "time_ns": cur["time_ns"],
                "reset": reset,
            }
            if reset:
                row["rate"] = None
            else:
                dt = (cur["time_ns"] - prev["time_ns"]) / 1e9
                row["rate"] = (cur["value"] - prev["value"]) / dt
            out.append(row)
    return out


def _retained_window(mats: list[MaterializedRecord]) -> tuple[int, int]:
    if not mats:
        return 0, 0  # see QUERY_ORACLE.md ambiguity #7 (empty retained set)
    ns = [m.received_ns for m in mats]
    return min(ns), max(ns)


def _freshness(mats: list[MaterializedRecord]) -> dict[str, int]:
    fresh: dict[str, int] = {}
    for m in mats:
        best = None
        for r in m.log_rows:
            best = r["observed_ns"] if best is None else max(best, r["observed_ns"])
        for r in m.metric_points:
            best = r["time_ns"] if best is None else max(best, r["time_ns"])
        if best is not None:
            fresh[m.label] = best if m.label not in fresh else max(fresh[m.label], best)
    return fresh


def _expected_gaps(mats: list[MaterializedRecord], query: dict) -> list[dict]:
    node = query.get("node")
    from_ns, to_ns = query["from_ns"], query["to_ns"]
    out = []
    for m in mats:
        if node is not None and m.label != node:
            continue
        if not (from_ns <= m.received_ns < to_ns):
            continue
        out.extend(m.gaps)
    return out


def _record_could_match(m: MaterializedRecord, query: dict) -> bool:
    kind = query["kind"]
    node = query.get("node")
    if node is not None and m.label != node:
        return False
    from_ns, to_ns = query["from_ns"], query["to_ns"]
    if kind == "logs":
        contains = query.get("contains")
        for r in m.log_rows:
            if not (from_ns <= r["observed_ns"] < to_ns):
                continue
            if contains is not None and contains not in r["body"]:
                continue
            return True
        return False
    if kind == "metrics":
        name = query["name"]
        for r in m.metric_points:
            if r["name"] == name and from_ns <= r["time_ns"] < to_ns:
                return True
        return False
    if kind == "rate":
        name = query["name"]
        for r in m.metric_points:
            if r["name"] == name and r["kind"] == "sum" and r.get("monotonic") and from_ns <= r["time_ns"] < to_ns:
                return True
        return False
    return False


def expected(records: Any, query: dict, unavailable: Any = None) -> dict:
    """Full-scan expectation. See module docstring for input shapes."""
    validate_query(query)
    mats = materialize_all(records)
    specs = validate_unavailable(unavailable) if unavailable is not None else []
    available = [m for m in mats if not _is_unavailable(m, specs)]
    excluded = [m for m in mats if _is_unavailable(m, specs)]

    kind = query["kind"]
    if kind == "logs":
        rows = _expected_log_rows(available, query)
    elif kind == "metrics":
        rows = _expected_metric_rows(available, query)
    else:
        rows = _expected_rate_rows(available, query)

    retained_from_ns, retained_to_ns = _retained_window(available)
    freshness = _freshness(available)
    gaps = _expected_gaps(available, query)
    could_hold = any(_record_could_match(m, query) for m in excluded)
    complete = not (bool(specs) and could_hold)

    return {
        "rows": rows,
        "retained_from_ns": retained_from_ns,
        "retained_to_ns": retained_to_ns,
        "freshness": freshness,
        "gaps": gaps,
        "complete": complete,
        "unavailable_nonempty": not complete,
    }


# --------------------------------------------------------------------------
# Answer shape validation
# --------------------------------------------------------------------------

_ENVELOPE_FIELDS = {
    "complete": bool,
    "retained_from_ns": int,
    "retained_to_ns": int,
    "freshness": dict,
    "gaps": list,
    "unavailable": list,
    "snapshot": str,
}
_PAGE_ALLOWED = set(_ENVELOPE_FIELDS) | {"rows", "next_page"}
_GAP_ALLOWED = {"node", "sequence", "receive_ns", "gap"}

_LOG_ROW_FIELDS = {
    "node": str,
    "node_id": str,
    "sequence": int,
    "index": int,
    "observed_ns": int,
    "body": str,
    "attributes": dict,
}
_METRIC_ROW_BASE = {
    "node": str,
    "node_id": str,
    "sequence": int,
    "index": int,
    "name": str,
    "unit": str,
    "kind": str,
    "time_ns": int,
    "start_ns": int,
    "attributes": dict,
}
_METRIC_ROW_ALLOWED = set(_METRIC_ROW_BASE) | {"value", "monotonic"}
_RATE_ROW_BASE = {
    "node": str,
    "name": str,
    "attributes": dict,
    "time_ns": int,
    "reset": bool,
}
_RATE_ROW_ALLOWED = set(_RATE_ROW_BASE) | {"rate"}


def _validate_attributes(attrs: dict, ctx: str) -> None:
    for k, v in attrs.items():
        if not isinstance(k, str) or not isinstance(v, str):
            raise MalformedInput(f"{ctx}: attributes must map string to string")


def _validate_pages_shape(pages: Any, query: dict) -> None:
    if not isinstance(pages, list) or len(pages) == 0:
        raise MalformedInput("answer must be a non-empty JSON array of pages")
    kind = query["kind"]
    for i, page in enumerate(pages):
        ctx = f"page[{i}]"
        if not isinstance(page, dict):
            raise MalformedInput(f"{ctx}: must be an object")
        _check_no_unknown_keys(page, _PAGE_ALLOWED, ctx)
        for key, typ in _ENVELOPE_FIELDS.items():
            _check_type(page, key, typ, ctx)
        for k, v in page["freshness"].items():
            if not isinstance(k, str) or not isinstance(v, int) or isinstance(v, bool):
                raise MalformedInput(f"{ctx}.freshness: must map string to integer")
        for gi, g in enumerate(page["gaps"]):
            gctx = f"{ctx}.gaps[{gi}]"
            if not isinstance(g, dict):
                raise MalformedInput(f"{gctx}: must be an object")
            _check_no_unknown_keys(g, _GAP_ALLOWED, gctx)
            for key, typ in (("node", str), ("sequence", int), ("receive_ns", int), ("gap", str)):
                _check_type(g, key, typ, gctx)
        _check_type(page, "rows", list, ctx)
        if kind in ("logs", "metrics"):
            if "next_page" not in page or not (page["next_page"] is None or isinstance(page["next_page"], str)):
                raise MalformedInput(f"{ctx}: next_page must be a string or null")
        else:
            if page.get("next_page") is not None:
                raise MalformedInput(f"{ctx}: rate next_page must be null or absent")
        for ri, row in enumerate(page["rows"]):
            rctx = f"{ctx}.rows[{ri}]"
            if not isinstance(row, dict):
                raise MalformedInput(f"{rctx}: must be an object")
            if kind == "logs":
                _check_no_unknown_keys(row, set(_LOG_ROW_FIELDS), rctx)
                for key, typ in _LOG_ROW_FIELDS.items():
                    _check_type(row, key, typ, rctx)
                _validate_attributes(row["attributes"], rctx)
            elif kind == "metrics":
                _check_no_unknown_keys(row, _METRIC_ROW_ALLOWED, rctx)
                for key, typ in _METRIC_ROW_BASE.items():
                    _check_type(row, key, typ, rctx)
                _validate_attributes(row["attributes"], rctx)
                if "value" not in row or isinstance(row["value"], bool) or not isinstance(row["value"], (int, float)):
                    raise MalformedInput(f"{rctx}: value must be a number")
                if row["kind"] == "sum":
                    _check_type(row, "monotonic", bool, rctx)
                elif row["kind"] == "gauge":
                    if "monotonic" in row:
                        raise MalformedInput(f"{rctx}: gauge rows must not include monotonic")
                else:
                    raise MalformedInput(f"{rctx}: kind must be 'gauge' or 'sum'")
            else:  # rate
                _check_no_unknown_keys(row, _RATE_ROW_ALLOWED, rctx)
                for key, typ in _RATE_ROW_BASE.items():
                    _check_type(row, key, typ, rctx)
                _validate_attributes(row["attributes"], rctx)
                rate_val = row.get("rate", "__missing__")
                if rate_val == "__missing__" and "rate" not in row:
                    raise MalformedInput(f"{rctx}: missing field 'rate'")
                rv = row["rate"]
                if not (rv is None or (isinstance(rv, (int, float)) and not isinstance(rv, bool))):
                    raise MalformedInput(f"{rctx}: rate must be a number or null")
                if row["reset"] and rv is not None:
                    raise MalformedInput(f"{rctx}: reset rows must have rate == null")
                if not row["reset"] and rv is None:
                    raise MalformedInput(f"{rctx}: non-reset rows must have a numeric rate")


# --------------------------------------------------------------------------
# Row diffing (shared by logs/metrics/rate)
# --------------------------------------------------------------------------


def _diff_rows(expected_rows, actual_rows, identity_fn, content_eq_fn, content_diff_fn):
    violations = []
    exp_keys = [identity_fn(r) for r in expected_rows]
    act_keys = [identity_fn(r) for r in actual_rows]
    exp_count = Counter(exp_keys)
    act_count = Counter(act_keys)
    missing = exp_count - act_count
    extra = act_count - exp_count
    for k, c in missing.items():
        violations.append(("ROW-DROPPED", f"missing row {k!r} (x{c})"))
    for k, c in extra.items():
        violations.append(("ROW-DUPLICATED", f"unexpected/duplicate row {k!r} (x{c})"))
    if not missing and not extra:
        if exp_keys != act_keys:
            violations.append(
                ("ROW-ORDER", f"row order differs: expected {exp_keys!r} got {act_keys!r}")
            )
        else:
            for er, ar in zip(expected_rows, actual_rows):
                if not content_eq_fn(er, ar):
                    rule, detail = content_diff_fn(er, ar)
                    violations.append((rule, detail))
    return violations


def _numbers_equal(expected_value, actual_value) -> bool:
    if isinstance(expected_value, bool) or isinstance(actual_value, bool):
        return False
    if type(expected_value) is not type(actual_value):
        return False
    if isinstance(expected_value, float):
        return struct.pack(">d", expected_value) == struct.pack(">d", actual_value)
    return expected_value == actual_value


def _row_equal(er: dict, ar: dict, kind: str) -> bool:
    if kind == "logs":
        for f_ in ("node", "node_id", "sequence", "index", "observed_ns", "body"):
            if er[f_] != ar.get(f_):
                return False
        return er["attributes"] == ar.get("attributes")
    for f_ in ("node", "node_id", "sequence", "index", "name", "unit", "kind", "time_ns", "start_ns"):
        if er[f_] != ar.get(f_):
            return False
    if er["attributes"] != ar.get("attributes"):
        return False
    if not _numbers_equal(er["value"], ar.get("value")):
        return False
    if er["kind"] == "sum" and er["monotonic"] != ar.get("monotonic"):
        return False
    return True


def _row_diff(er: dict, ar: dict, kind: str) -> tuple[str, str]:
    fields_to_check = (
        ["node", "node_id", "sequence", "index", "observed_ns", "body", "attributes"]
        if kind == "logs"
        else [
            "node",
            "node_id",
            "sequence",
            "index",
            "name",
            "unit",
            "kind",
            "time_ns",
            "start_ns",
            "attributes",
            "value",
            "monotonic",
        ]
    )
    diffs = []
    for f_ in fields_to_check:
        ev = er.get(f_, "<absent>")
        av = ar.get(f_, "<absent>")
        if f_ == "value":
            if not _numbers_equal(er.get("value"), ar.get("value")):
                diffs.append(f"value: expected {ev!r} got {av!r}")
        elif ev != av:
            diffs.append(f"{f_}: expected {ev!r} got {av!r}")
    return "ROW-CONTENT", f"row {er.get('node_id')}/{er.get('sequence')}/{er.get('index')}: " + "; ".join(diffs)


def _rate_row_equal(er: dict, ar: dict) -> bool:
    if er["reset"] != ar.get("reset"):
        return False
    if er["reset"]:
        return ar.get("rate") is None
    ar_rate = ar.get("rate")
    if ar_rate is None or isinstance(ar_rate, bool) or not isinstance(ar_rate, (int, float)):
        return False
    return math.isclose(er["rate"], float(ar_rate), rel_tol=1e-9, abs_tol=0.0) or er["rate"] == ar_rate == 0.0


def _rate_row_diff(er: dict, ar: dict) -> tuple[str, str]:
    if er["reset"] != ar.get("reset"):
        return (
            "RATE-RESET",
            f"series {er['node']}/{er['attributes']}@{er['time_ns']}: expected reset={er['reset']} got {ar.get('reset')!r}",
        )
    return (
        "RATE-VALUE",
        f"series {er['node']}/{er['attributes']}@{er['time_ns']}: expected rate={er['rate']!r} got {ar.get('rate')!r}",
    )


def _gap_multiset(gaps: list[dict]) -> Counter:
    return Counter((g["node"], g["sequence"], g["receive_ns"], g["gap"]) for g in gaps)


# --------------------------------------------------------------------------
# check()
# --------------------------------------------------------------------------


def _check_paginated(exp: dict, pages: list[dict], query: dict) -> list[tuple[str, str]]:
    violations = []
    limit = query["limit"]
    for i, p in enumerate(pages):
        if len(p["rows"]) > limit:
            violations.append(("PAGE-LIMIT", f"page[{i}] has {len(p['rows'])} rows, exceeds limit {limit}"))
    for i, p in enumerate(pages):
        is_last = i == len(pages) - 1
        if is_last and p["next_page"] is not None:
            violations.append(("PAGE-NEXT", f"page[{i}] is last but next_page is not null"))
        if not is_last and p["next_page"] is None:
            violations.append(("PAGE-NEXT", f"page[{i}] is not last but next_page is null"))
    snapshots = {p["snapshot"] for p in pages}
    if len(snapshots) > 1:
        violations.append(("PAGE-SNAPSHOT", f"pages carry different snapshot tokens: {sorted(snapshots)!r}"))

    actual_rows = [row for p in pages for row in p["rows"]]
    key_fn = lambda r: (r["node_id"], r["sequence"], r["index"])
    kind = query["kind"]
    violations.extend(
        _diff_rows(
            exp["rows"],
            actual_rows,
            key_fn,
            lambda er, ar: _row_equal(er, ar, kind),
            lambda er, ar: _row_diff(er, ar, kind),
        )
    )
    return violations


def _check_rate(exp: dict, pages: list[dict], query: dict) -> list[tuple[str, str]]:
    violations = []
    if len(pages) != 1:
        violations.append(("PAGE-LIMIT", f"rate answers must be exactly one page, got {len(pages)}"))
    page = pages[0]
    actual_rows = page["rows"]
    key_fn = lambda r: (r["node"], tuple(sorted(r["attributes"].items())), r["time_ns"])
    violations.extend(
        _diff_rows(exp["rows"], actual_rows, key_fn, _rate_row_equal, _rate_row_diff)
    )
    return violations


def _check_envelope(exp: dict, pages: list[dict], query: dict) -> list[tuple[str, str]]:
    violations = []
    first = pages[0]
    consistent_keys = ("complete", "retained_from_ns", "retained_to_ns", "freshness", "gaps", "unavailable")
    for i, p in enumerate(pages[1:], start=1):
        for key in consistent_keys:
            if p[key] != first[key]:
                violations.append(("ENVELOPE-CONSISTENT", f"page[{i}].{key} differs from page[0].{key}"))

    p0 = pages[0]
    if p0["retained_from_ns"] != exp["retained_from_ns"] or p0["retained_to_ns"] != exp["retained_to_ns"]:
        violations.append(
            (
                "RETAINED-WINDOW",
                f"expected [{exp['retained_from_ns']}, {exp['retained_to_ns']}) "
                f"got [{p0['retained_from_ns']}, {p0['retained_to_ns']})",
            )
        )
    if p0["freshness"] != exp["freshness"]:
        violations.append(("FRESHNESS", f"expected {exp['freshness']!r} got {p0['freshness']!r}"))
    if _gap_multiset(exp["gaps"]) != _gap_multiset(p0["gaps"]):
        violations.append(("GAPS", f"expected gaps {exp['gaps']!r} got {p0['gaps']!r}"))
    if p0["complete"] != exp["complete"]:
        violations.append(("COMPLETE", f"expected complete={exp['complete']!r} got {p0['complete']!r}"))
    unavailable_nonempty = len(p0["unavailable"]) > 0
    if unavailable_nonempty != exp["unavailable_nonempty"]:
        violations.append(
            (
                "UNAVAILABLE-SHAPE",
                f"expected unavailable-nonempty={exp['unavailable_nonempty']!r} got {unavailable_nonempty!r}",
            )
        )
    return violations


def check(records: Any, query: dict, pages: Any, unavailable: Any = None) -> dict:
    """Grade an implementation's answer against the full-scan expectation.

    See module docstring for input shapes and the verdict shape.
    """
    try:
        exp = expected(records, query, unavailable)
    except MalformedInput as e:
        return {
            "passed": False,
            "violations": [{"rule": "MALFORMED", "detail": str(e)}],
            "expected_rows": None,
            "answered_rows": None,
        }

    expected_rows = len(exp["rows"])

    try:
        _validate_pages_shape(pages, query)
    except MalformedInput as e:
        return {
            "passed": False,
            "violations": [{"rule": "MALFORMED", "detail": str(e)}],
            "expected_rows": expected_rows,
            "answered_rows": None,
        }

    answered_rows = sum(len(p["rows"]) for p in pages)

    kind = query["kind"]
    violations: list[tuple[str, str]] = []
    if kind == "rate":
        violations.extend(_check_rate(exp, pages, query))
    else:
        violations.extend(_check_paginated(exp, pages, query))
    violations.extend(_check_envelope(exp, pages, query))

    return {
        "passed": len(violations) == 0,
        "violations": [{"rule": r, "detail": d} for r, d in violations],
        "expected_rows": expected_rows,
        "answered_rows": answered_rows,
    }


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(description="Fabric O11y phase-4 retained-history query oracle")
    parser.add_argument("--records", required=True)
    parser.add_argument("--query", required=True)
    parser.add_argument("--answer", required=True)
    parser.add_argument("--unavailable")
    args = parser.parse_args(argv)

    try:
        records = load_records_jsonl(args.records)
        query = load_json_file(args.query)
        answer = load_json_file(args.answer)
        unavailable = load_json_file(args.unavailable) if args.unavailable else None
    except MalformedInput as e:
        verdict = {
            "passed": False,
            "violations": [{"rule": "MALFORMED", "detail": str(e)}],
            "expected_rows": None,
            "answered_rows": None,
        }
        print(json.dumps(verdict))
        return 2

    verdict = check(records, query, answer, unavailable)
    print(json.dumps(verdict))
    if any(v["rule"] == "MALFORMED" for v in verdict["violations"]):
        return 2
    return 0 if verdict["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
