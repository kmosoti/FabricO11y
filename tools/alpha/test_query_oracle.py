#!/usr/bin/env python3
"""Tests for tools/alpha/query_oracle.py.

Builds protobuf `Batch` fixtures with a small hand-written encoder (never
importing any implementation code), then exercises `expected()`/`check()`
directly and the CLI via subprocess (for exit-code and malformed-input
behavior). Mutation controls: starting from one correct multi-page answer,
each mutant changes exactly one thing and must fail with the rule the
mutation targets; the unmutated original must pass.

Run: python3 -B tools/alpha/test_query_oracle.py
"""

import base64
import json
import os
import struct
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import query_oracle as qo  # noqa: E402

THIS_DIR = os.path.dirname(os.path.abspath(__file__))
ORACLE_PATH = os.path.join(THIS_DIR, "query_oracle.py")


# --------------------------------------------------------------------------
# A tiny, independent protobuf encoder (mirrors the field numbers listed in
# QUERY_ORACLE.md; written fresh here, not shared with query_oracle.py).
# --------------------------------------------------------------------------


def _varint(n: int) -> bytes:
    assert n >= 0
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def _tag(field_no: int, wire_type: int) -> bytes:
    return _varint((field_no << 3) | wire_type)


def _ld(field_no: int, payload: bytes) -> bytes:
    return _tag(field_no, 2) + _varint(len(payload)) + payload


def _fixed64(field_no: int, raw8: bytes) -> bytes:
    return _tag(field_no, 1) + raw8


def _vint(field_no: int, n: int) -> bytes:
    return _tag(field_no, 0) + _varint(n)


def _str_field(field_no: int, s: str) -> bytes:
    return _ld(field_no, s.encode("utf-8"))


def any_value_string(s: str) -> bytes:
    return _str_field(1, s)


def key_value(k: str, any_value_bytes: bytes) -> bytes:
    return _ld(1, k.encode()) + _ld(2, any_value_bytes)


def attrs_fields(field_no: int, attrs: dict) -> bytes:
    out = b""
    for k, v in attrs.items():
        out += _ld(field_no, key_value(k, any_value_string(v)))
    return out


def log_record(time_ns: int, observed_ns: int, body, attrs: dict) -> bytes:
    out = _fixed64(1, struct.pack("<Q", time_ns))
    out += _fixed64(11, struct.pack("<Q", observed_ns))
    if body is not None:
        out += _ld(5, any_value_string(body))
    out += attrs_fields(6, attrs)
    return out


def logs_request(records: list) -> bytes:
    scope_logs = b"".join(_ld(2, r) for r in records)
    resource_logs = _ld(2, scope_logs)
    return _ld(1, resource_logs)


def number_data_point(start_ns: int, time_ns: int, attrs: dict, value, is_double: bool) -> bytes:
    out = _fixed64(2, struct.pack("<Q", start_ns))
    out += _fixed64(3, struct.pack("<Q", time_ns))
    out += attrs_fields(7, attrs)
    if is_double:
        out += _fixed64(4, struct.pack("<d", value))
    else:
        out += _fixed64(6, struct.pack("<q", value))
    return out


def gauge_metric(name: str, unit: str, points: list) -> bytes:
    gauge = b"".join(_ld(1, p) for p in points)
    return _str_field(1, name) + _str_field(3, unit) + _ld(5, gauge)


def sum_metric(name: str, unit: str, monotonic: bool, points: list) -> bytes:
    sum_msg = b"".join(_ld(1, p) for p in points)
    sum_msg += _vint(3, 1 if monotonic else 0)
    return _str_field(1, name) + _str_field(3, unit) + _ld(7, sum_msg)


def histogram_metric(name: str, unit: str) -> bytes:
    """A metric kind the contract does not define rows for (tag 9)."""
    return _str_field(1, name) + _str_field(3, unit) + _ld(9, b"")


def metrics_request(metrics: list) -> bytes:
    scope_metrics = b"".join(_ld(2, m) for m in metrics)
    resource_metrics = _ld(2, scope_metrics)
    return _ld(1, resource_metrics)


def encode_batch(node_id16: bytes, generation: int, sequence: int, metrics_bytes: bytes,
                  logs_bytes: bytes, gaps: list) -> bytes:
    out = _vint(1, 1)
    out += _ld(2, node_id16)
    out += _vint(3, generation)
    out += _vint(4, sequence)
    if metrics_bytes:
        out += _ld(5, metrics_bytes)
    if logs_bytes:
        out += _ld(6, logs_bytes)
    for g in gaps:
        out += _str_field(8, g)
    return out


def record_dict(label: str, received_ns: int, node_id16: bytes, generation: int, sequence: int,
                 metrics_bytes: bytes = b"", logs_bytes: bytes = b"", gaps=None) -> dict:
    raw = encode_batch(node_id16, generation, sequence, metrics_bytes, logs_bytes, gaps or [])
    return {"label": label, "received_ns": received_ns, "bytes": base64.b64encode(raw).decode("ascii")}


NODE_A = bytes([0xAA] * 16)
NODE_B = bytes([0xBB] * 16)


# --------------------------------------------------------------------------
# Fixture: five log rows across two nodes, used for the pagination tests.
# --------------------------------------------------------------------------


def build_log_fixture():
    r1 = record_dict(
        "nodeA", 1000, NODE_A, 1, 1,
        logs_bytes=logs_request([
            log_record(100, 100, "alpha one", {"env": "prod"}),
            log_record(200, 200, "alpha two", {"env": "prod"}),
            log_record(300, 300, "beta three", {"env": "prod"}),
        ]),
        gaps=["gap-a1"],
    )
    r2 = record_dict(
        "nodeB", 1500, NODE_B, 1, 1,
        logs_bytes=logs_request([
            log_record(150, 150, "gamma four", {"env": "prod"}),
            log_record(400, 400, "alpha five", {"env": "prod"}),
        ]),
    )
    return [r1, r2]


LOG_QUERY = {"kind": "logs", "from_ns": 0, "to_ns": 2000, "limit": 2}

# Expected order: observed_ns, node_id, sequence, index
#   100 nodeA idx0 "alpha one"
#   150 nodeB idx0 "gamma four"
#   200 nodeA idx1 "alpha two"
#   300 nodeA idx2 "beta three"
#   400 nodeB idx1 "alpha five"


def build_correct_log_pages(records, query):
    exp = qo.expected(records, query)
    rows = exp["rows"]
    limit = query["limit"]
    pages = []
    snapshot = "snap-1"
    for i in range(0, len(rows), limit):
        chunk = rows[i : i + limit]
        is_last = i + limit >= len(rows)
        pages.append(
            {
                "complete": exp["complete"],
                "retained_from_ns": exp["retained_from_ns"],
                "retained_to_ns": exp["retained_to_ns"],
                "freshness": exp["freshness"],
                "gaps": exp["gaps"],
                "unavailable": [],
                "snapshot": snapshot,
                "rows": chunk,
                "next_page": None if is_last else f"tok-{i}",
            }
        )
    return pages, exp


# --------------------------------------------------------------------------
# Fixture: rate query with a reset-by-start-change and a reset-by-decrease.
# --------------------------------------------------------------------------


def build_rate_fixture():
    # Series A: node nodeA, attrs {ep: x}: normal interval then a start_ns reset.
    r1 = record_dict(
        "nodeA", 1000, NODE_A, 1, 1,
        metrics_bytes=metrics_request([
            sum_metric("reqs", "1", True, [
                number_data_point(1, 100, {"ep": "x"}, 5, False),
                number_data_point(1, 200, {"ep": "x"}, 8, False),
            ]),
        ]),
    )
    r2 = record_dict(
        "nodeA", 2000, NODE_A, 1, 2,
        metrics_bytes=metrics_request([
            sum_metric("reqs", "1", True, [
                number_data_point(2, 300, {"ep": "x"}, 1, False),  # start changed -> reset
            ]),
        ]),
    )
    # Series B: node nodeB, attrs {ep: y}: a decrease then a normal interval.
    r3 = record_dict(
        "nodeB", 1500, NODE_B, 1, 1,
        metrics_bytes=metrics_request([
            sum_metric("reqs", "1", True, [
                number_data_point(5, 120, {"ep": "y"}, 100, False),
                number_data_point(5, 220, {"ep": "y"}, 90, False),  # decrease -> reset
                number_data_point(5, 320, {"ep": "y"}, 95, False),
            ]),
        ]),
    )
    return [r1, r2, r3]


RATE_QUERY = {"kind": "rate", "from_ns": 0, "to_ns": 10_000, "name": "reqs"}


def build_correct_rate_page(records, query):
    exp = qo.expected(records, query)
    page = {
        "complete": exp["complete"],
        "retained_from_ns": exp["retained_from_ns"],
        "retained_to_ns": exp["retained_to_ns"],
        "freshness": exp["freshness"],
        "gaps": exp["gaps"],
        "unavailable": [],
        "snapshot": "snap-rate",
        "rows": exp["rows"],
        "next_page": None,
    }
    return [page], exp


# --------------------------------------------------------------------------
# Decoder tests
# --------------------------------------------------------------------------


class TestDecoder(unittest.TestCase):
    def test_log_record_roundtrip(self):
        raw = log_record(10, 20, "hello", {"a": "b", "c": "d"})
        decoded = qo.decode_log_record(raw)
        self.assertEqual(decoded["observed_time_unix_nano"], 20)
        self.assertEqual(decoded["body"], "hello")
        self.assertEqual(decoded["attributes"], {"a": "b", "c": "d"})

    def test_non_string_attribute_dropped(self):
        kv = key_value("n", _fixed64(4, struct.pack("<d", 3.0)))  # double-valued attribute
        raw = _fixed64(1, struct.pack("<Q", 0)) + _fixed64(11, struct.pack("<Q", 0)) + _ld(6, kv)
        decoded = qo.decode_log_record(raw)
        self.assertEqual(decoded["attributes"], {})

    def test_unknown_field_ignored(self):
        raw = log_record(10, 20, "hello", {}) + _str_field(99, "future-field")
        decoded = qo.decode_log_record(raw)
        self.assertEqual(decoded["body"], "hello")

    def test_gauge_and_sum_points(self):
        gp = number_data_point(0, 100, {"h": "a"}, 2.5, True)
        gauge = gauge_metric("cpu", "pct", [gp])
        sp = number_data_point(9, 200, {"p": "/x"}, 7, False)
        summ = sum_metric("hits", "1", True, [sp])
        hist = histogram_metric("latency", "ms")
        req = metrics_request([gauge, summ, hist])
        decoded = qo.decode_metrics_request(req)
        self.assertEqual(len(decoded), 2)  # histogram excluded
        self.assertEqual(decoded[0]["kind"], "gauge")
        self.assertEqual(decoded[0]["points"][0]["value"], 2.5)
        self.assertIsInstance(decoded[0]["points"][0]["value"], float)
        self.assertEqual(decoded[1]["kind"], "sum")
        self.assertTrue(decoded[1]["monotonic"])
        self.assertEqual(decoded[1]["points"][0]["value"], 7)
        self.assertIsInstance(decoded[1]["points"][0]["value"], int)

    def test_batch_field_numbers(self):
        raw = encode_batch(NODE_A, 4, 9, b"", b"", ["g1", "g2"])
        decoded = qo.decode_batch(raw)
        self.assertEqual(decoded["node_id"], NODE_A)
        self.assertEqual(decoded["generation"], 4)
        self.assertEqual(decoded["sequence"], 9)
        self.assertEqual(decoded["gaps"], ["g1", "g2"])


# --------------------------------------------------------------------------
# expected() tests
# --------------------------------------------------------------------------


class TestExpectedLogs(unittest.TestCase):
    def setUp(self):
        self.records = build_log_fixture()

    def test_full_scan_order(self):
        exp = qo.expected(self.records, LOG_QUERY)
        bodies = [r["body"] for r in exp["rows"]]
        self.assertEqual(bodies, ["alpha one", "gamma four", "alpha two", "beta three", "alpha five"])

    def test_contains_filter(self):
        q = dict(LOG_QUERY, contains="alpha")
        exp = qo.expected(self.records, q)
        bodies = [r["body"] for r in exp["rows"]]
        self.assertEqual(bodies, ["alpha one", "alpha two", "alpha five"])

    def test_node_filter(self):
        q = dict(LOG_QUERY, node="nodeB")
        exp = qo.expected(self.records, q)
        bodies = [r["body"] for r in exp["rows"]]
        self.assertEqual(bodies, ["gamma four", "alpha five"])

    def test_half_open_range_excludes_to_ns(self):
        q = {"kind": "logs", "from_ns": 100, "to_ns": 300, "limit": 10}
        exp = qo.expected(self.records, q)
        bodies = [r["body"] for r in exp["rows"]]
        self.assertNotIn("beta three", bodies)  # observed_ns==300 excluded
        self.assertIn("alpha one", bodies)  # observed_ns==100 included

    def test_gaps_in_range(self):
        exp = qo.expected(self.records, LOG_QUERY)
        self.assertEqual(len(exp["gaps"]), 1)
        self.assertEqual(exp["gaps"][0]["gap"], "gap-a1")

    def test_retained_window_and_freshness(self):
        exp = qo.expected(self.records, LOG_QUERY)
        self.assertEqual((exp["retained_from_ns"], exp["retained_to_ns"]), (1000, 1500))
        self.assertEqual(exp["freshness"], {"nodeA": 300, "nodeB": 400})

    def test_complete_true_without_unavailable(self):
        exp = qo.expected(self.records, LOG_QUERY)
        self.assertTrue(exp["complete"])
        self.assertFalse(exp["unavailable_nonempty"])


class TestExpectedRate(unittest.TestCase):
    def setUp(self):
        self.records = build_rate_fixture()

    def test_series_order_and_reset_markers(self):
        exp = qo.expected(self.records, RATE_QUERY)
        rows = exp["rows"]
        self.assertEqual(len(rows), 4)
        # Series A (nodeA) sorts before series B (nodeB)
        self.assertEqual([r["node"] for r in rows], ["nodeA", "nodeA", "nodeB", "nodeB"])
        self.assertEqual([r["time_ns"] for r in rows], [200, 300, 220, 320])
        self.assertEqual([r["reset"] for r in rows], [False, True, True, False])
        self.assertIsNone(rows[1]["rate"])
        self.assertIsNone(rows[2]["rate"])
        self.assertAlmostEqual(rows[0]["rate"], (8 - 5) / ((200 - 100) / 1e9))
        self.assertAlmostEqual(rows[3]["rate"], (95 - 90) / ((320 - 220) / 1e9))

    def test_node_filter_restricts_series(self):
        q = dict(RATE_QUERY, node="nodeB")
        exp = qo.expected(self.records, q)
        self.assertTrue(all(r["node"] == "nodeB" for r in exp["rows"]))
        self.assertEqual(len(exp["rows"]), 2)


class TestUnavailable(unittest.TestCase):
    def test_record_form_excludes_rows_and_marks_incomplete(self):
        records = build_log_fixture()
        unavailable = [{"node": "nodeA", "sequence": 1}]
        exp = qo.expected(records, LOG_QUERY, unavailable)
        bodies = [r["body"] for r in exp["rows"]]
        self.assertNotIn("alpha one", bodies)
        self.assertNotIn("alpha two", bodies)
        self.assertNotIn("beta three", bodies)
        self.assertFalse(exp["complete"])
        self.assertTrue(exp["unavailable_nonempty"])
        # gaps from the excluded record must not appear either
        self.assertEqual(exp["gaps"], [])

    def test_unavailable_record_outside_query_leaves_complete_true(self):
        records = build_log_fixture()
        unavailable = [{"node": "nodeB", "sequence": 1}]
        # Query only asks for a range that nodeA's remaining data covers;
        # nodeB's excluded record's rows are outside [0,50) so it "could not"
        # match this particular (very narrow) query.
        q = {"kind": "logs", "from_ns": 0, "to_ns": 50, "limit": 10}
        exp = qo.expected(records, q, unavailable)
        self.assertTrue(exp["complete"])

    def test_range_form(self):
        records = build_log_fixture()
        unavailable = [{"from_ns": 900, "to_ns": 1100}]  # covers nodeA's receive time 1000
        exp = qo.expected(records, LOG_QUERY, unavailable)
        bodies = [r["body"] for r in exp["rows"]]
        self.assertNotIn("alpha one", bodies)
        self.assertFalse(exp["complete"])


# --------------------------------------------------------------------------
# check() / pagination mutants
# --------------------------------------------------------------------------


class TestPaginationCorrect(unittest.TestCase):
    def test_correct_answer_passes(self):
        records = build_log_fixture()
        pages, _exp = build_correct_log_pages(records, LOG_QUERY)
        verdict = qo.check(records, LOG_QUERY, pages)
        self.assertTrue(verdict["passed"], verdict["violations"])
        self.assertEqual(verdict["expected_rows"], 5)
        self.assertEqual(verdict["answered_rows"], 5)


class TestPaginationMutants(unittest.TestCase):
    def setUp(self):
        self.records = build_log_fixture()
        self.pages, self.exp = build_correct_log_pages(self.records, LOG_QUERY)

    def _rules(self, pages):
        verdict = qo.check(self.records, LOG_QUERY, pages)
        return verdict, {v["rule"] for v in verdict["violations"]}

    def test_drop_a_row(self):
        import copy

        pages = copy.deepcopy(self.pages)
        pages[1]["rows"].pop(0)  # drop "beta three"
        verdict, rules = self._rules(pages)
        self.assertFalse(verdict["passed"])
        self.assertIn("ROW-DROPPED", rules)

    def test_duplicate_row_across_pages(self):
        import copy

        pages = copy.deepcopy(self.pages)
        pages[1]["rows"].append(pages[0]["rows"][0])  # duplicate "alpha one" into page 2
        verdict, rules = self._rules(pages)
        self.assertFalse(verdict["passed"])
        self.assertIn("ROW-DUPLICATED", rules)

    def test_reorder_two_rows(self):
        import copy

        pages = copy.deepcopy(self.pages)
        pages[0]["rows"][0], pages[0]["rows"][1] = pages[0]["rows"][1], pages[0]["rows"][0]
        verdict, rules = self._rules(pages)
        self.assertFalse(verdict["passed"])
        self.assertIn("ROW-ORDER", rules)

    def test_change_one_body(self):
        import copy

        pages = copy.deepcopy(self.pages)
        pages[0]["rows"][0]["body"] = "TAMPERED"
        verdict, rules = self._rules(pages)
        self.assertFalse(verdict["passed"])
        self.assertIn("ROW-CONTENT", rules)

    def test_snapshot_mismatch_between_pages(self):
        import copy

        pages = copy.deepcopy(self.pages)
        pages[1]["snapshot"] = "different-snapshot"
        verdict, rules = self._rules(pages)
        self.assertFalse(verdict["passed"])
        self.assertIn("PAGE-SNAPSHOT", rules)

    def test_page_exceeds_limit(self):
        import copy

        pages = copy.deepcopy(self.pages)
        # Merge all rows into page 0, exceeding limit=2.
        pages[0]["rows"] = self.exp["rows"]
        pages[0]["next_page"] = None
        pages = [pages[0]]
        verdict, rules = self._rules(pages)
        self.assertFalse(verdict["passed"])
        self.assertIn("PAGE-LIMIT", rules)

    def test_next_page_wrong_on_last(self):
        import copy

        pages = copy.deepcopy(self.pages)
        pages[-1]["next_page"] = "should-be-null"
        verdict, rules = self._rules(pages)
        self.assertFalse(verdict["passed"])
        self.assertIn("PAGE-NEXT", rules)

    def test_next_page_missing_on_nonlast(self):
        import copy

        pages = copy.deepcopy(self.pages)
        pages[0]["next_page"] = None
        verdict, rules = self._rules(pages)
        self.assertFalse(verdict["passed"])
        self.assertIn("PAGE-NEXT", rules)

    def test_report_complete_true_while_unavailable(self):
        import copy

        pages = copy.deepcopy(self.pages)
        unavailable = [{"node": "nodeA", "sequence": 1}]
        for p in pages:
            p["complete"] = True
            p["unavailable"] = []
        verdict = qo.check(self.records, LOG_QUERY, pages, unavailable)
        rules = {v["rule"] for v in verdict["violations"]}
        self.assertFalse(verdict["passed"])
        self.assertIn("COMPLETE", rules)

    def test_wrong_retained_window(self):
        import copy

        pages = copy.deepcopy(self.pages)
        for p in pages:
            p["retained_to_ns"] += 1
        verdict, rules = self._rules(pages)
        self.assertFalse(verdict["passed"])
        self.assertIn("RETAINED-WINDOW", rules)

    def test_wrong_freshness(self):
        import copy

        pages = copy.deepcopy(self.pages)
        for p in pages:
            p["freshness"] = dict(p["freshness"], nodeA=999999)
        verdict, rules = self._rules(pages)
        self.assertFalse(verdict["passed"])
        self.assertIn("FRESHNESS", rules)

    def test_wrong_gaps(self):
        import copy

        pages = copy.deepcopy(self.pages)
        for p in pages:
            p["gaps"] = []
        verdict, rules = self._rules(pages)
        self.assertFalse(verdict["passed"])
        self.assertIn("GAPS", rules)

    def test_envelope_inconsistent_across_pages(self):
        import copy

        pages = copy.deepcopy(self.pages)
        pages[1]["retained_to_ns"] = pages[1]["retained_to_ns"] + 5
        verdict, rules = self._rules(pages)
        self.assertFalse(verdict["passed"])
        self.assertIn("ENVELOPE-CONSISTENT", rules)


class TestRateMutants(unittest.TestCase):
    def setUp(self):
        self.records = build_rate_fixture()
        self.pages, self.exp = build_correct_rate_page(self.records, RATE_QUERY)

    def test_correct_rate_answer_passes(self):
        verdict = qo.check(self.records, RATE_QUERY, self.pages)
        self.assertTrue(verdict["passed"], verdict["violations"])

    def test_wrong_rate_at_reset(self):
        import copy

        pages = copy.deepcopy(self.pages)
        # Flip the reset at index 1 (series A's t=300) to a bogus rate.
        for row in pages[0]["rows"]:
            if row["node"] == "nodeA" and row["time_ns"] == 300:
                row["reset"] = False
                row["rate"] = 42.0
        verdict = qo.check(self.records, RATE_QUERY, pages)
        rules = {v["rule"] for v in verdict["violations"]}
        self.assertFalse(verdict["passed"])
        self.assertIn("RATE-RESET", rules)

    def test_wrong_rate_value(self):
        import copy

        pages = copy.deepcopy(self.pages)
        for row in pages[0]["rows"]:
            if row["node"] == "nodeA" and row["time_ns"] == 200:
                row["rate"] = row["rate"] * 2
        verdict = qo.check(self.records, RATE_QUERY, pages)
        rules = {v["rule"] for v in verdict["violations"]}
        self.assertFalse(verdict["passed"])
        self.assertIn("RATE-VALUE", rules)

    def test_extra_page_for_rate_is_rejected(self):
        import copy

        pages = copy.deepcopy(self.pages) + copy.deepcopy(self.pages)
        verdict = qo.check(self.records, RATE_QUERY, pages)
        rules = {v["rule"] for v in verdict["violations"]}
        self.assertFalse(verdict["passed"])
        self.assertIn("PAGE-LIMIT", rules)


# --------------------------------------------------------------------------
# Value-type exactness (int stays an int; doubles bit-exact)
# --------------------------------------------------------------------------


class TestValueTypeExactness(unittest.TestCase):
    def build(self):
        r = record_dict(
            "nodeA", 1000, NODE_A, 1, 1,
            metrics_bytes=metrics_request([
                gauge_metric("cpu", "pct", [number_data_point(0, 100, {}, 7, False)]),  # int value
            ]),
        )
        query = {"kind": "metrics", "from_ns": 0, "to_ns": 1000, "name": "cpu", "limit": 10}
        return [r], query

    def _page_for(self, exp, query):
        return [
            {
                "complete": exp["complete"],
                "retained_from_ns": exp["retained_from_ns"],
                "retained_to_ns": exp["retained_to_ns"],
                "freshness": exp["freshness"],
                "gaps": exp["gaps"],
                "unavailable": [],
                "snapshot": "s",
                "rows": exp["rows"],
                "next_page": None,
            }
        ]

    def test_int_reported_as_float_fails(self):
        records, query = self.build()
        exp = qo.expected(records, query)
        page = self._page_for(exp, query)
        page[0]["rows"] = [dict(r) for r in page[0]["rows"]]
        page[0]["rows"][0]["value"] = 7.0  # int became float
        verdict = qo.check(records, query, page)
        rules = {v["rule"] for v in verdict["violations"]}
        self.assertFalse(verdict["passed"])
        self.assertIn("ROW-CONTENT", rules)

    def test_int_as_int_passes(self):
        records, query = self.build()
        exp = qo.expected(records, query)
        page = self._page_for(exp, query)
        verdict = qo.check(records, query, page)
        self.assertTrue(verdict["passed"], verdict["violations"])
        self.assertIsInstance(page[0]["rows"][0]["value"], int)


class TestMonotonicAndStartNsShape(unittest.TestCase):
    def test_gauge_rows_must_not_carry_monotonic(self):
        r = record_dict(
            "nodeA", 1000, NODE_A, 1, 1,
            metrics_bytes=metrics_request([
                gauge_metric("cpu", "pct", [number_data_point(0, 100, {}, 1.0, True)]),
            ]),
        )
        query = {"kind": "metrics", "from_ns": 0, "to_ns": 1000, "name": "cpu", "limit": 10}
        exp = qo.expected([r], query)
        row = dict(exp["rows"][0])
        row["monotonic"] = False  # not allowed on a gauge row
        page = [
            {
                "complete": True,
                "retained_from_ns": exp["retained_from_ns"],
                "retained_to_ns": exp["retained_to_ns"],
                "freshness": exp["freshness"],
                "gaps": [],
                "unavailable": [],
                "snapshot": "s",
                "rows": [row],
                "next_page": None,
            }
        ]
        verdict = qo.check([r], query, page)
        rules = {v["rule"] for v in verdict["violations"]}
        self.assertFalse(verdict["passed"])
        self.assertIn("MALFORMED", rules)

    def test_gauge_start_ns_is_zero(self):
        r = record_dict(
            "nodeA", 1000, NODE_A, 1, 1,
            metrics_bytes=metrics_request([
                gauge_metric("cpu", "pct", [number_data_point(999, 100, {}, 1.0, True)]),
            ]),
        )
        query = {"kind": "metrics", "from_ns": 0, "to_ns": 1000, "name": "cpu", "limit": 10}
        exp = qo.expected([r], query)
        self.assertEqual(exp["rows"][0]["start_ns"], 0)  # gauge start_ns forced to 0


# --------------------------------------------------------------------------
# Malformed input -> exit 2 (both via check() and via the CLI)
# --------------------------------------------------------------------------


class TestMalformedViaCheck(unittest.TestCase):
    def setUp(self):
        self.records = build_log_fixture()
        self.pages, _ = build_correct_log_pages(self.records, LOG_QUERY)

    def test_missing_envelope_field(self):
        import copy

        pages = copy.deepcopy(self.pages)
        del pages[0]["snapshot"]
        verdict = qo.check(self.records, LOG_QUERY, pages)
        rules = {v["rule"] for v in verdict["violations"]}
        self.assertIn("MALFORMED", rules)

    def test_wrong_fixed_type(self):
        import copy

        pages = copy.deepcopy(self.pages)
        pages[0]["complete"] = "true"  # string, not bool
        verdict = qo.check(self.records, LOG_QUERY, pages)
        rules = {v["rule"] for v in verdict["violations"]}
        self.assertIn("MALFORMED", rules)

    def test_unknown_field(self):
        import copy

        pages = copy.deepcopy(self.pages)
        pages[0]["extra_field"] = 1
        verdict = qo.check(self.records, LOG_QUERY, pages)
        rules = {v["rule"] for v in verdict["violations"]}
        self.assertIn("MALFORMED", rules)

    def test_bad_records_input(self):
        verdict = qo.check([{"label": "x"}], LOG_QUERY, self.pages)  # missing fields
        rules = {v["rule"] for v in verdict["violations"]}
        self.assertIn("MALFORMED", rules)
        self.assertIsNone(verdict["expected_rows"])

    def test_bad_unavailable_input(self):
        verdict = qo.check(self.records, LOG_QUERY, self.pages, unavailable=[{"node": "x"}])
        rules = {v["rule"] for v in verdict["violations"]}
        self.assertIn("MALFORMED", rules)


class TestCLI(unittest.TestCase):
    def _write(self, tmp, name, obj, jsonl=False):
        path = os.path.join(tmp, name)
        with open(path, "w", encoding="utf-8") as fh:
            if jsonl:
                for line in obj:
                    fh.write(json.dumps(line) + "\n")
            else:
                fh.write(json.dumps(obj))
        return path

    def _run(self, tmp, records_path, query_path, answer_path, unavailable_path=None):
        cmd = [
            sys.executable,
            "-B",
            ORACLE_PATH,
            "--records",
            records_path,
            "--query",
            query_path,
            "--answer",
            answer_path,
        ]
        if unavailable_path:
            cmd += ["--unavailable", unavailable_path]
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
        return proc

    def test_cli_pass_exit_0(self):
        with tempfile.TemporaryDirectory() as tmp:
            records = build_log_fixture()
            pages, _ = build_correct_log_pages(records, LOG_QUERY)
            rp = self._write(tmp, "records.jsonl", records, jsonl=True)
            qp = self._write(tmp, "query.json", LOG_QUERY)
            ap = self._write(tmp, "answer.json", pages)
            proc = self._run(tmp, rp, qp, ap)
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            verdict = json.loads(proc.stdout)
            self.assertTrue(verdict["passed"])

    def test_cli_fail_exit_1(self):
        with tempfile.TemporaryDirectory() as tmp:
            records = build_log_fixture()
            pages, _ = build_correct_log_pages(records, LOG_QUERY)
            pages[0]["rows"][0]["body"] = "TAMPERED"
            rp = self._write(tmp, "records.jsonl", records, jsonl=True)
            qp = self._write(tmp, "query.json", LOG_QUERY)
            ap = self._write(tmp, "answer.json", pages)
            proc = self._run(tmp, rp, qp, ap)
            self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
            verdict = json.loads(proc.stdout)
            self.assertFalse(verdict["passed"])

    def test_cli_malformed_duplicate_key_exit_2(self):
        with tempfile.TemporaryDirectory() as tmp:
            records = build_log_fixture()
            pages, _ = build_correct_log_pages(records, LOG_QUERY)
            rp = self._write(tmp, "records.jsonl", records, jsonl=True)
            qp = self._write(tmp, "query.json", LOG_QUERY)
            ap = os.path.join(tmp, "answer.json")
            with open(ap, "w", encoding="utf-8") as fh:
                # Hand-write JSON with a duplicate key at the top-level page object.
                fh.write('[{"complete": true, "complete": false}]')
            proc = self._run(tmp, rp, qp, ap)
            self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)
            verdict = json.loads(proc.stdout)
            self.assertEqual(verdict["violations"][0]["rule"], "MALFORMED")

    def test_cli_malformed_nan_exit_2(self):
        with tempfile.TemporaryDirectory() as tmp:
            records = build_log_fixture()
            rp = self._write(tmp, "records.jsonl", records, jsonl=True)
            qp = self._write(tmp, "query.json", LOG_QUERY)
            ap = os.path.join(tmp, "answer.json")
            with open(ap, "w", encoding="utf-8") as fh:
                fh.write('[{"complete": NaN}]')
            proc = self._run(tmp, rp, qp, ap)
            self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)

    def test_cli_malformed_bad_query_exit_2(self):
        with tempfile.TemporaryDirectory() as tmp:
            records = build_log_fixture()
            pages, _ = build_correct_log_pages(records, LOG_QUERY)
            rp = self._write(tmp, "records.jsonl", records, jsonl=True)
            bad_query = dict(LOG_QUERY)
            bad_query["kind"] = "not-a-real-kind"
            qp = self._write(tmp, "query.json", bad_query)
            ap = self._write(tmp, "answer.json", pages)
            proc = self._run(tmp, rp, qp, ap)
            self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
