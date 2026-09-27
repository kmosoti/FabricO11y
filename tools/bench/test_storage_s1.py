#!/usr/bin/env python3
"""Independent malformed-CSV probes for the S1 result analyzer."""

import csv
import tempfile
import unittest
from pathlib import Path

from summarize_storage_s1 import BUILD_FIELDS, QUERY_FIELDS, SEEDS, WORKLOADS, analyze


def write_csv(path, fields, rows):
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader()
        writer.writerows(rows)


def fixture():
    builds = []
    queries = []
    for workload in WORKLOADS:
        for seed in SEEDS:
            builds.append(dict(zip(BUILD_FIELDS, (workload, seed, 2048, 10, 20, 30, 4096, 16896, 1024))))
            for trial in range(5):
                for query in range(128):
                    case = query % 8
                    for mode in ("scan", "pruned"):
                        skipped = 32 if mode == "pruned" and workload == "clustered_logs" and case in (1, 4) else 0
                        queries.append(dict(zip(QUERY_FIELDS, (workload, seed, trial, query, case, mode,
                            100 + trial + query, 2048 - 64 * skipped, skipped, 0, 0, 1))))
    return builds, queries


class SummaryChecks(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.builds, cls.queries = fixture()

    def run_analyzer(self, builds=None, queries=None):
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            write_csv(directory / "builds.csv", BUILD_FIELDS, builds if builds is not None else self.builds)
            write_csv(directory / "queries.csv", QUERY_FIELDS, queries if queries is not None else self.queries)
            return analyze(directory)

    def test_complete_grid_and_registered_gate(self):
        result = self.run_analyzer()
        self.assertEqual(result["status"], "advances")
        self.assertEqual(result["workloads"]["clustered_logs"]["42"]["cases"]["1"]["pruned_to_scan_ratio"], 0)
        self.assertEqual(result["workloads"]["gauge"]["43"]["latency"]["scan"]["median_trial_p50_ns"], 165)

    def test_rejects_missing_and_duplicate_cells(self):
        for rows in (self.queries[:-1], self.queries + [self.queries[0]]):
            with self.subTest(length=len(rows)), self.assertRaises(ValueError):
                self.run_analyzer(queries=rows)

    def test_rejects_corrupt_query_fields(self):
        mutations = (
            ("equal", 0), ("matches", 1), ("expected_matches", 1),
            ("scanned_events", 2047), ("skipped_blocks", 1),
            ("mode", "bogus"), ("case", 7), ("trial", 5),
            ("elapsed_ns", "nan"),
        )
        for field, value in mutations:
            with self.subTest(field=field), self.assertRaises(ValueError):
                rows = list(self.queries)
                rows[0] = dict(rows[0], **{field: value})
                self.run_analyzer(queries=rows)

    def test_rejects_inconsistent_repeated_query(self):
        rows = list(self.queries)
        rows[1] = dict(rows[1], expected_matches=1, matches=1)
        with self.assertRaisesRegex(ValueError, "vary across"):
            self.run_analyzer(queries=rows)

    def test_rejects_build_count(self):
        builds = list(self.builds)
        builds[0] = dict(builds[0], summary_bytes=0)
        with self.assertRaises(ValueError):
            self.run_analyzer(builds=builds)

    def test_failed_performance_gate_is_inconclusive(self):
        rows = [dict(row, scanned_events=2048, skipped_blocks=0) if row["workload"] == "clustered_logs" and row["seed"] == 42 and row["case"] == 1 and row["mode"] == "pruned" else row for row in self.queries]
        result = self.run_analyzer(queries=rows)
        self.assertEqual(result["status"], "inconclusive")
        self.assertTrue(result["gate_failures"])


if __name__ == "__main__":
    unittest.main()
