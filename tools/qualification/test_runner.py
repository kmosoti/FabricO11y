import json
import shutil
import subprocess
import sys
import time
import unittest
from pathlib import Path

from runner import MARKER, clear_owned, live_bytes, owned_root, run


def process_start(pid):
    try:
        return int(Path(f"/proc/{pid}/stat").read_text().rpartition(") ")[2].split()[19])
    except FileNotFoundError:
        return None


class RunnerControls(unittest.TestCase):
    def setUp(self):
        self.path = Path(__file__).resolve().parents[2] / "target" / "alpha-runner-test"
        if self.path.exists():
            self.assertTrue((self.path / ".fabric-alpha-owned").is_file())
            self.assertFalse(self.path.is_symlink())
            shutil.rmtree(self.path)
        owned_root(self.path)

    def tearDown(self):
        shutil.rmtree(self.path)

    def test_duration_violation_fails(self):
        result = run(self.path, [sys.executable, "-c", "import time; time.sleep(2)"],
                     0.05, 10_000, 10_000)
        self.assertEqual(result["stop_reason"], "duration_limit")
        self.assertFalse(result["passed"])

    def test_disk_violation_fails(self):
        result = run(self.path, [sys.executable, "-c", "open('large', 'wb').write(b'x' * 10000)"],
                     2, 1000, 10_000)
        self.assertEqual(result["stop_reason"], "disk_limit")
        self.assertFalse(result["passed"])

    def test_output_evidence_limit_fails(self):
        result = run(self.path, [sys.executable, "-c", "import time; print('x'*10000, flush=True); time.sleep(2)"],
                     2, 100_000, 1000)
        self.assertEqual(result["stop_reason"], "evidence_limit")
        self.assertFalse(result["passed"])

    def test_fast_exit_disk_violation_fails(self):
        for attempt in range(8):
            result = run(self.path, [sys.executable, "-c", "open('large', 'wb').write(b'x'*100000)"],
                         2, 1000, 1000)
            self.assertFalse(result["passed"], attempt)
            self.assertEqual(result["stop_reason"], "disk_limit")
            (self.path / "large").unlink()
            (self.path / "result.json").unlink()
            (self.path / "command-output.bin").unlink()

    def test_fast_exit_output_violation_fails(self):
        result = run(self.path, [sys.executable, "-c", "print('x'*10000)"],
                     2, 100_000, 1000)
        self.assertFalse(result["passed"])
        self.assertEqual(result["stop_reason"], "evidence_limit")
        self.assertTrue(result["output_truncated"])

    def test_fast_exit_duration_violation_fails(self):
        result = run(self.path, [sys.executable, "-c", "pass"],
                     1e-9, 100_000, 100_000)
        self.assertFalse(result["passed"])
        self.assertEqual(result["stop_reason"], "duration_limit")

    def test_positive_control(self):
        result = run(self.path, [sys.executable, "-c", "print('ok')"],
                     2, 100_000, 100_000)
        self.assertTrue(result["passed"])
        self.assertIsNone(result["stop_reason"])
        self.assertEqual(result["exit_code"], 0)

    def test_invalid_limits_direct_and_cli(self):
        for duration in (float("nan"), float("inf"), float("-inf"), 0, -1):
            with self.assertRaises(ValueError):
                run(self.path, [sys.executable, "-c", "pass"], duration, 1000, 1000)
        for disk, output in ((0, 1000), (1000, 0), (float("nan"), 1000)):
            with self.assertRaises(ValueError):
                run(self.path, [sys.executable, "-c", "pass"], 1, disk, output)
        for invalid_tier in (10.0, True, float("nan")):
            with self.assertRaises(ValueError):
                run(self.path, [sys.executable, "-c", "pass"], 1, 1000, 1000,
                    (invalid_tier, 1))
        cli = subprocess.run([sys.executable, str(Path(__file__).with_name("runner.py")),
                              "--out", str(self.path), "--duration-s", "nan",
                              "--disk-bytes", "1000", "--", sys.executable, "-c",
                              "open('should-not-run','w').close()"], capture_output=True, text=True)
        self.assertEqual(cli.returncode, 2)
        self.assertFalse((self.path / "should-not-run").exists())

    def test_rejects_tampered_marker(self):
        (self.path / MARKER).write_text("not ours\n")
        with self.assertRaises(ValueError):
            owned_root(self.path)
        (self.path / MARKER).write_text("fabric-alpha-runner-v1\n")
        (self.path / MARKER).unlink()
        (self.path / MARKER).symlink_to("/tmp/anything")
        with self.assertRaises(ValueError):
            owned_root(self.path)
        (self.path / MARKER).unlink()
        (self.path / MARKER).write_text("fabric-alpha-runner-v1\n")

    def test_orphan_descendant_is_killed(self):
        script = ("import subprocess,sys; "
                  "child=subprocess.Popen([sys.executable,'-c',"
                  "'import os,time; from pathlib import Path; "
                  "start=Path(\"/proc/self/stat\").read_text().split()[21]; "
                  "Path(\"descendant-ready\").write_text(str(os.getpid())+\" \"+start); time.sleep(10)']); "
                  "open('descendant-pid','w').write(str(child.pid)); "
                  "exec('while not __import__(\"os\").path.exists(\"descendant-ready\"): "
                  "__import__(\"time\").sleep(0.01)'); sys.exit(0)")
        result = run(self.path, [sys.executable, "-c", script], 2, 100_000, 100_000)
        self.assertFalse(result["passed"])
        self.assertEqual(result["stop_reason"], "orphan_descendant")
        self.assertTrue(result["process_group_cleanup_ok"])
        pid = int((self.path / "descendant-pid").read_text())
        ready_pid, start = (self.path / "descendant-ready").read_text().split()
        self.assertEqual(pid, int(ready_pid))
        self.assertNotEqual(process_start(pid), int(start))

    def test_detached_descendant_ignoring_term_is_killed(self):
        late = ("import os,signal,time; from pathlib import Path; "
                "signal.signal(signal.SIGTERM, signal.SIG_IGN); "
                "Path('detached-ready').write_text(str(os.getpid())); "
                "time.sleep(1); Path('late.txt').write_text('late')")
        script = ("import subprocess,sys,time; from pathlib import Path; "
                  f"p=subprocess.Popen([sys.executable,'-c',{late!r}],start_new_session=True); "
                  "Path('detached-pid').write_text(str(p.pid)); "
                  "exec('while not Path(\"detached-ready\").exists(): time.sleep(0.01)')")
        result = run(self.path, [sys.executable, "-c", script], 3, 100_000, 100_000)
        self.assertFalse(result["passed"])
        self.assertEqual(result["stop_reason"], "orphan_descendant")
        self.assertTrue(result["process_group_cleanup_ok"])
        pid = int((self.path / "detached-pid").read_text())
        self.assertFalse(Path(f"/proc/{pid}").exists())
        time.sleep(1.1)
        self.assertFalse((self.path / "late.txt").exists())

    def test_completed_detached_child_and_unrelated_process(self):
        unrelated = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(5)"],
                                     start_new_session=True)
        try:
            done = ("import subprocess,sys; "
                    "p=subprocess.Popen([sys.executable,'-c','pass'],start_new_session=True); "
                    "assert p.wait()==0")
            passed = run(self.path, [sys.executable, "-c", done], 3, 100_000, 100_000)
            self.assertTrue(passed["passed"], passed)
            self.assertIsNone(unrelated.poll())
            orphan = ("import subprocess,sys; "
                      "subprocess.Popen([sys.executable,'-c',"
                      "'import time; time.sleep(10)'],start_new_session=True)")
            failed = run(self.path, [sys.executable, "-c", orphan], 3, 100_000, 100_000)
            self.assertEqual(failed["stop_reason"], "orphan_descendant")
            self.assertIsNone(unrelated.poll())
        finally:
            unrelated.terminate()
            unrelated.wait(timeout=2)

    def test_tmpdir_and_temp_churn_within_budget(self):
        code = ("import os,tempfile,time; from pathlib import Path; "
                "assert Path(tempfile.gettempdir()) == Path.cwd()/'tmp'; "
                "end=time.monotonic()+0.4; "
                "exec('while time.monotonic()<end: "
                "p=Path(tempfile.gettempdir())/\"short\"; p.write_bytes(b\"x\"*100); p.unlink()')")
        result = run(self.path, [sys.executable, "-c", code], 3, 100_000, 100_000)
        self.assertTrue(result["passed"], result)

    def test_rate_artifacts_must_be_fresh_and_result_invalidated(self):
        workload = Path(__file__).with_name("workload.py")
        command = [sys.executable, str(workload), "--tier", "10", "--seconds", "1",
                   "--seed", "0xA11FA001", "--out", "offers.jsonl",
                   "--byte-cap", "1000000"]
        first = run(self.path, command, 3, 500_000, 100_000, (10, 1))
        self.assertTrue(first["passed"], first)
        second = run(self.path, [sys.executable, "-c", "pass"],
                     3, 500_000, 100_000, (10, 1))
        self.assertFalse(second["passed"])
        self.assertEqual(second["stop_reason"], "rate_contract")
        self.assertNotEqual(first["invocation_id"], second["invocation_id"])
        self.assertFalse(json.loads((self.path / "result.json").read_text())["passed"])
        with self.assertRaises(FileNotFoundError):
            run(self.path, ["/definitely/missing/alpha-command"], 3, 500_000, 100_000)
        pending = json.loads((self.path / "result.json").read_text())
        self.assertFalse(pending["passed"])
        self.assertEqual(pending["stop_reason"], "in_progress")

    def test_interrupted_rerun_leaves_failing_current_result(self):
        first = run(self.path, [sys.executable, "-c", "pass"], 3, 100_000, 100_000)
        self.assertTrue(first["passed"])
        cli = subprocess.Popen(
            [sys.executable, str(Path(__file__).with_name("runner.py")),
             "--out", str(self.path), "--duration-s", "3", "--disk-bytes", "100000",
             "--", sys.executable, "-c", "import time; time.sleep(0.5)"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        try:
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                # The runner unlinks the old result before writing the new
                # one, so a poll can land in that gap or on a partial write.
                try:
                    current = json.loads((self.path / "result.json").read_text())
                except (FileNotFoundError, json.JSONDecodeError):
                    time.sleep(0.01)
                    continue
                if current.get("invocation_id") != first["invocation_id"]:
                    break
                time.sleep(0.01)
            else:
                self.fail("rerun did not invalidate the old result")
            self.assertFalse(current["passed"])
            self.assertEqual(current["stop_reason"], "in_progress")
            cli.kill()
            self.assertEqual(cli.wait(timeout=2), -9)
            current = json.loads((self.path / "result.json").read_text())
            self.assertFalse(current["passed"])
            self.assertEqual(current["stop_reason"], "in_progress")
        finally:
            if cli.poll() is None:
                cli.kill()
                cli.wait(timeout=2)
            time.sleep(0.55)

    def test_rate_overrun_fails_runner(self):
        script = ("from pathlib import Path; "
                  "Path('offered.csv').write_text("
                  "'second,offered_logs,offered_metrics,admitted,committed,backlog,gaps\\n'"
                  "+'0,21,320,341,0,341,0\\n')")
        result = run(self.path, [sys.executable, "-c", script],
                     2, 100_000, 100_000, (10, 1))
        self.assertFalse(result["passed"])
        self.assertEqual(result["stop_reason"], "rate_contract")

    def test_rejects_symlink(self):
        link = self.path / "escape"
        link.symlink_to("/tmp")
        with self.assertRaises(ValueError):
            live_bytes(self.path)
        with self.assertRaises(ValueError):
            clear_owned(self.path)
        link.unlink()

    def test_cleanup_requires_owned_direct_root(self):
        other = self.path.with_name("not-alpha-owned")
        with self.assertRaises(ValueError):
            clear_owned(other)
        self.assertGreater(clear_owned(self.path), 0)
        self.assertFalse(self.path.exists())
        owned_root(self.path)


if __name__ == "__main__":
    unittest.main()
