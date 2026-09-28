import csv
import hashlib
import json
import subprocess
import sys
import unittest
from pathlib import Path

from rate_oracle import check_rate, check_source
from workload import SEEDS, entropy_body, records_at


class WorkloadContract(unittest.TestCase):
    def test_independent_registered_fixture(self):
        root = Path(__file__).resolve().parents[2] / "target" / "alpha-phase0-fixture-test"
        # Fresh disposable output is needed because the emitter refuses overwrite.
        from runner import owned_root, live_bytes
        import shutil
        if root.exists():
            owned_root(root)
            live_bytes(root)
            shutil.rmtree(root)
        args = [sys.executable, str(Path(__file__).with_name("runner.py")),
                "--out", str(root), "--duration-s", "5", "--disk-bytes", "2000000",
                "--max-output-bytes", "100000", "--rate-tier", "10", "--rate-seconds", "2",
                "--", sys.executable, str(Path(__file__).with_name("workload.py")),
                "--tier", "10", "--seconds", "2", "--seed", "0xA11FA001",
                "--out", "offers.jsonl", "--byte-cap", "1000000"]
        result = subprocess.run(args, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        data = (root / "offers.jsonl").read_bytes()
        self.assertEqual(len(data), 49196)
        self.assertEqual(len(data.splitlines()), 360)
        # This digest is retained as a fixed known-result fixture. The count and
        # first record properties below are checked separately from the emitter.
        self.assertEqual(hashlib.sha256(data).hexdigest(),
                         "a6e5fca651aabb25fa954c51903fd35f837bc7e244712c311f6ee9765d164f15")
        first = json.loads(data.splitlines()[0])
        self.assertEqual(first, {"kind": "log", "node": 0, "scheduled_ms": 0, "body": "R" * 512})
        check_rate(root / "offered.csv", 10, 2)
        self.assertEqual(check_source(root / "offers.jsonl", 10, 2), SEEDS[0])
        self.assertEqual(json.loads(result.stdout)["passed"], True)
        lines = [json.loads(line) for line in data.splitlines()]
        entropy = next(i for i, item in enumerate(lines)
                       if item["kind"] == "log" and item["scheduled_ms"] == 500)
        lines[entropy]["body"] = "S" * 512
        (root / "offers.jsonl").write_text("".join(json.dumps(item) + "\n" for item in lines))
        with self.assertRaisesRegex(ValueError, "seeded entropy body mismatch"):
            check_source(root / "offers.jsonl", 10, 2)
        lines[entropy] = json.loads(data.splitlines()[entropy])
        lines[0]["arbitrary"] = "extra"
        (root / "offers.jsonl").write_text("".join(json.dumps(item) + "\n" for item in lines))
        with self.assertRaisesRegex(ValueError, "unexpected offer fields"):
            check_source(root / "offers.jsonl", 10, 2)
        lines[0] = json.loads(data.splitlines()[0])
        metric = next(i for i, item in enumerate(lines) if item["kind"] == "metric")
        lines[metric]["value"] ^= 1
        (root / "offers.jsonl").write_text("".join(json.dumps(item) + "\n" for item in lines))
        with self.assertRaisesRegex(ValueError, "seeded metric value mismatch"):
            check_source(root / "offers.jsonl", 10, 2)
        (root / "offers.jsonl").write_bytes(b"\n".join(data.splitlines()[:-1]) + b"\n")
        with self.assertRaisesRegex(ValueError, "missing offers"):
            check_source(root / "offers.jsonl", 10, 2)
        shutil.rmtree(root)

    def test_log_shape_and_seed_variation(self):
        zero = list(records_at(10, 0, SEEDS[0]))
        self.assertEqual(len(zero), 340)
        self.assertEqual(sum(record["kind"] == "log" for record in zero), 20)
        self.assertEqual(len(entropy_body(SEEDS[0], 0, 1)), 512)
        self.assertNotEqual(entropy_body(SEEDS[0], 0, 1), entropy_body(SEEDS[1], 0, 1))
        self.assertEqual(len(list(records_at(10, 1, SEEDS[0]))), 20)

    def test_every_registered_seed_matches_exact_oracle(self):
        from runner import clear_owned, owned_root
        root = Path(__file__).resolve().parents[2] / "target" / "alpha-all-seeds-test"
        if root.exists():
            clear_owned(root)
        owned_root(root)
        for seed in SEEDS:
            data = "".join(json.dumps(item) + "\n" for item in records_at(10, 0, seed))
            (root / "offers.jsonl").write_text(data)
            self.assertEqual(check_source(root / "offers.jsonl", 10, 1), seed)
        clear_owned(root)

    def test_paced_source_preserves_schedule_and_accounts_offers(self):
        from runner import clear_owned, owned_root
        root = Path(__file__).resolve().parents[2] / "target" / "alpha-paced-test"
        if root.exists():
            clear_owned(root)
        owned_root(root)
        args = [sys.executable, str(Path(__file__).with_name("runner.py")),
                "--out", str(root), "--duration-s", "3", "--disk-bytes", "2000000",
                "--max-output-bytes", "100000", "--rate-tier", "10", "--rate-seconds", "1",
                "--", sys.executable, str(Path(__file__).with_name("workload.py")),
                "--tier", "10", "--seconds", "1", "--seed", "0xA11FA001",
                "--out", "offers.jsonl", "--byte-cap", "1000000", "--paced"]
        result = subprocess.run(args, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        records = [json.loads(line) for line in (root / "offers.jsonl").read_text().splitlines()]
        self.assertEqual(len(records), 340)
        self.assertEqual([r["scheduled_ms"] for r in records],
                         sorted(r["scheduled_ms"] for r in records))
        self.assertTrue(all(r["emitted_ms"] >= r["scheduled_ms"] for r in records))
        self.assertGreaterEqual(json.loads(result.stdout)["elapsed_s"], 0.5)
        check_rate(root / "offered.csv", 10, 1)
        check_source(root / "offers.jsonl", 10, 1)
        clear_owned(root)

    def test_rate_oracle_rejects_overrun_and_balance(self):
        root = Path(__file__).resolve().parents[2] / "target" / "alpha-rate-test"
        from runner import owned_root
        import shutil
        if root.exists():
            owned_root(root)
            shutil.rmtree(root)
        owned_root(root)
        path = root / "offered.csv"
        def write(logs, admitted, backlog):
            with path.open("w", newline="") as sink:
                rows = csv.writer(sink)
                rows.writerow(("second", "offered_logs", "offered_metrics", "admitted", "committed", "backlog", "gaps"))
                rows.writerow((0, logs, 320, admitted, 0, backlog, 0))
        write(20, 340, 340)
        check_rate(path, 10, 1)
        write(21, 341, 341)
        with self.assertRaisesRegex(ValueError, "offered rate mismatch"):
            check_rate(path, 10, 1)
        write(20, 340, 339)
        with self.assertRaisesRegex(ValueError, "backlog mismatch"):
            check_rate(path, 10, 1)
        shutil.rmtree(root)


if __name__ == "__main__":
    unittest.main()
