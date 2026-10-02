"""Calibration bounds of the observation cost model against the committed
measurements. `python3 -B tools/model/test_observation_model.py`."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import observation_model as om  # noqa: E402


def quantile(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(len(xs) * q))]


class Calibration(unittest.TestCase):
    rows = om.calibrate()

    def bench(self, min_n):
        return [r for r in self.rows if r["source"] == "fobbench" and int(r["case"].split()[1][1:]) >= min_n]

    def test_bytes_are_exact_at_block_sizes_from_sixteen(self):
        errs = [abs(e) for e in om.errors(self.bench(16))["bytes"]]
        self.assertLessEqual(quantile(errs, 0.5), 0.01)
        self.assertLessEqual(quantile(errs, 0.9), 0.13)
        self.assertLessEqual(max(errs), 0.20)
        errs = [abs(e) for e in om.errors(self.bench(256))["bytes"]]
        self.assertLessEqual(quantile(errs, 0.9), 0.10)

    def test_times_are_within_planning_tolerance(self):
        e = om.errors(self.bench(16))
        self.assertLessEqual(quantile([abs(x) for x in e["encode_ns"]], 0.5), 0.25)
        self.assertLessEqual(quantile([abs(x) for x in e["decode_ns"]], 0.5), 0.25)

    def test_journal_bytes_within_ten_percent(self):
        journal = [r for r in self.rows if r["source"] == "encoding run 01"]
        self.assertEqual(len(journal), 2)
        for r in journal:
            p, m = r["bytes"]
            self.assertLessEqual(abs(p - m) / m, 0.10, r["case"])
            p, m = r["compressed_bytes"]
            self.assertLessEqual(abs(p - m) / m, 0.15, r["case"])


class Structure(unittest.TestCase):
    def test_varint_lengths(self):
        for v, n in [(0, 1), (127, 1), (128, 2), (16_383, 2), (16_384, 3), (2**63, 10), (2**64 - 1, 10)]:
            self.assertEqual(om.varint_len(v), n)

    def test_overhead_floor_for_a_bare_line_is_ten_bytes(self):
        w = om.Workload(records=4096, share_lines=1.0, share_points=0.0, body_bytes=0, attrs=0, attr_distinct_keys=0,
                        attr_distinct_values_total=0, time_jitter_ns=0, sequence_step=0, index_step=1, first_sequence_bytes=2)
        self.assertAlmostEqual(om.bytes_per_record(w), 10.0, delta=0.1)

    def test_more_records_per_block_never_costs_more_per_record(self):
        prev = None
        for n in (1, 2, 16, 256, 4096, 65536):
            b = om.bytes_per_record(om.Workload(records=n))
            if prev is not None:
                self.assertLessEqual(b, prev + 1e-9)
            prev = b

    def test_capabilities_scale_linearly_in_nodes(self):
        w, m = om.Workload(), om.Machine()
        a = om.capabilities(w, m, om.Fleet(nodes=100))
        b = om.capabilities(w, m, om.Fleet(nodes=10_000))
        self.assertAlmostEqual(b["journal_MB_per_s"] / a["journal_MB_per_s"], 100, delta=1e-6)
        self.assertAlmostEqual(a["retention_hours"] / b["retention_hours"], 100, delta=1e-6)


if __name__ == "__main__":
    unittest.main()
