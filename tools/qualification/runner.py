"""Sample a trusted alpha command in an owned target/alpha-* directory.

Polling detects violations during and after execution. Arbitrary children can
write beyond a disk budget between samples; our generators enforce byte caps
before each write. This runner never cleans data automatically.
"""

import argparse
import ctypes
import json
import math
import os
from pathlib import Path
import secrets
import signal
import shutil
import stat
import subprocess
import sys
import time
from rate_oracle import check_rate, check_source

MARKER = ".fabric-alpha-owned"
MARKER_CONTENT = "fabric-alpha-runner-v1\n"
MAX_DISK = 5 * 1024**3
MAX_EVIDENCE = 50 * 1024**2
MAX_DURATION = 7200


def validate_limits(duration_s, disk_bytes, max_output_bytes):
    if (not isinstance(duration_s, (int, float)) or isinstance(duration_s, bool)
            or not math.isfinite(duration_s) or not 0 < duration_s <= MAX_DURATION):
        raise ValueError("duration outside finite registered range")
    if (not isinstance(disk_bytes, int) or isinstance(disk_bytes, bool)
            or not 0 < disk_bytes <= MAX_DISK):
        raise ValueError("disk budget outside registered range")
    if (not isinstance(max_output_bytes, int) or isinstance(max_output_bytes, bool)
            or not 0 < max_output_bytes <= MAX_EVIDENCE):
        raise ValueError("evidence budget outside registered range")


def owned_root(path: Path) -> Path:
    target = (Path(__file__).resolve().parents[2] / "target").resolve()
    path = path.absolute()
    if path.is_symlink() or not path.name.startswith("alpha-") or path.parent.resolve() != target:
        raise ValueError("output must be a direct, non-symlink target/alpha-* child")
    if path.exists():
        marker = path / MARKER
        if (not path.is_dir() or marker.is_symlink() or not marker.is_file()
                or marker.read_text() != MARKER_CONTENT):
            raise ValueError("existing output has no valid alpha ownership marker")
    else:
        path.mkdir(parents=True)
        (path / MARKER).write_text(MARKER_CONTENT)
    return path


def live_bytes(root: Path) -> int:
    total = 0
    def walk_error(error):
        if isinstance(error, FileNotFoundError):
            return
        raise error
    for directory, dirs, files in os.walk(root, followlinks=False, onerror=walk_error):
        for name in dirs + files:
            p = Path(directory) / name
            try:
                info = p.lstat()
            except FileNotFoundError:
                # A benchmark may create and remove a temp file between the
                # directory listing and this one metadata read.
                continue
            mode = info.st_mode
            if stat.S_ISLNK(mode) or not (stat.S_ISDIR(mode) or stat.S_ISREG(mode)):
                raise ValueError(f"symlink or special file in disposable data: {p}")
            if stat.S_ISREG(mode):
                total += info.st_size
    return total


def clear_owned(path: Path) -> int:
    """Delete only an owned direct target child after validating its whole tree."""
    if not path.exists() or path.is_symlink():
        raise ValueError("disposable root is missing or linked")
    root = owned_root(path)
    size = live_bytes(root)
    shutil.rmtree(root)
    return size


def is_tagged(pid: int, token: str) -> bool:
    try:
        path = Path(f"/proc/{pid}")
        if path.stat().st_uid != os.getuid():
            return False
        needle = f"FABRIC_ALPHA_INVOCATION={token}".encode()
        if needle not in (path / "environ").read_bytes().split(b"\0"):
            return False
        state = (path / "stat").read_text().rpartition(") ")[2].split()[0]
        return state != "Z"
    except (FileNotFoundError, ProcessLookupError, PermissionError, IndexError, ValueError):
        return False


def tagged_members(token: str) -> list[int]:
    """Find live Linux processes started by this trusted invocation.

    Descendants inherit the environment token even after setsid() or parent
    exit. This does not track commands that deliberately scrub their env.
    """
    result = []
    for entry in os.scandir("/proc"):
        if entry.name.isdigit() and is_tagged(int(entry.name), token):
            result.append(int(entry.name))
    return result


def tagged_rss_bytes(pids: list[int]) -> int:
    total = 0
    for pid in pids:
        try:
            for line in Path(f"/proc/{pid}/status").read_text().splitlines():
                if line.startswith("VmRSS:"):
                    total += int(line.split()[1]) * 1024
                    break
        except (FileNotFoundError, ProcessLookupError):
            pass
    return total


def signal_tagged(pids: list[int], token: str, sig: signal.Signals) -> None:
    """Use pidfds so PID reuse cannot redirect a cleanup signal."""
    for pid in pids:
        try:
            fd = os.pidfd_open(pid)
            try:
                if is_tagged(pid, token):
                    signal.pidfd_send_signal(fd, sig)
            finally:
                os.close(fd)
        except ProcessLookupError:
            pass


def terminate_tagged(token: str, seen: set[int]) -> bool:
    current = tagged_members(token)
    seen.update(current)
    signal_tagged(current, token, signal.SIGTERM)
    deadline = time.monotonic() + 0.5
    while time.monotonic() < deadline:
        current = tagged_members(token)
        seen.update(current)
        if not current:
            return True
        time.sleep(0.01)
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        current = tagged_members(token)
        seen.update(current)
        if not current:
            return True
        signal_tagged(current, token, signal.SIGKILL)
        time.sleep(0.01)
    return not tagged_members(token)


def reap_seen(seen: set[int]) -> bool:
    # The runner is a Linux subreaper during one invocation. Reap only PIDs
    # observed with its token; unrelated child processes are never signaled.
    remaining = set(seen)
    deadline = time.monotonic() + 2
    while remaining and time.monotonic() < deadline:
        for pid in tuple(remaining):
            try:
                waited, _ = os.waitpid(pid, os.WNOHANG)
                if waited == pid:
                    remaining.remove(pid)
            except ChildProcessError:
                # Already reaped, or still owned by an intermediate parent.
                # Remaining live descendants are checked by the token scan.
                remaining.remove(pid)
        if remaining:
            time.sleep(0.01)
    return not remaining


def run(root: Path, command: list[str], duration_s: float, disk_bytes: int,
        max_output_bytes: int, rate_contract: tuple[int, int] | None = None) -> dict:
    validate_limits(duration_s, disk_bytes, max_output_bytes)
    if rate_contract is not None:
        if (not isinstance(rate_contract, tuple) or len(rate_contract) != 2
                or not isinstance(rate_contract[0], int) or isinstance(rate_contract[0], bool)
                or rate_contract[0] not in (10, 100, 1000)
                or not isinstance(rate_contract[1], int) or isinstance(rate_contract[1], bool)
                or not 0 < rate_contract[1] <= 135):
            raise ValueError("invalid rate contract")
    if not command or not all(isinstance(part, str) for part in command):
        raise ValueError("command must be a nonempty string argument vector")
    root = owned_root(root)
    output = root / "command-output.bin"
    report = root / "result.json"
    if output.is_symlink() or report.is_symlink():
        raise ValueError("output path is a symlink")
    live_bytes(root)
    # Invalidate the prior result before any valid rerun can fail or crash.
    report.unlink(missing_ok=True)
    if rate_contract is not None:
        for name in ("offered.csv", "offers.jsonl"):
            artifact = root / name
            if artifact.exists():
                if not artifact.is_file() or artifact.is_symlink():
                    raise ValueError(f"reserved rate artifact is not a regular file: {name}")
                artifact.unlink()
    tmpdir = root / "tmp"
    if tmpdir.is_symlink() or (tmpdir.exists() and not tmpdir.is_dir()):
        raise ValueError("owned TMPDIR is not a directory")
    tmpdir.mkdir(exist_ok=True)
    reserve = len(json.dumps({"command": command})) + 600
    starting_bytes = live_bytes(root)
    if starting_bytes + reserve >= disk_bytes or reserve >= max_output_bytes:
        raise ValueError("preflight budget cannot retain command result")
    output_allowance = max_output_bytes - reserve
    start = time.monotonic()
    reason = None
    peak_rss = 0
    peak_live = starting_bytes
    raw_output_bytes = 0
    token = secrets.token_hex(16)
    report.write_text(json.dumps({"invocation_id": token, "passed": False,
                                  "stop_reason": "in_progress"}, sort_keys=True) + "\n")
    child_env = os.environ.copy()
    child_env["FABRIC_ALPHA_INVOCATION"] = token
    child_env["TMPDIR"] = str(tmpdir)
    seen = set()
    libc = ctypes.CDLL(None)
    old_subreaper = ctypes.c_int()
    if sys.platform == "linux":
        if libc.prctl(37, ctypes.byref(old_subreaper), 0, 0, 0) != 0:
            raise OSError("cannot inspect Linux subreaper state")
        if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
            raise OSError("cannot supervise descendant process tree")
    with output.open("wb") as sink:
        try:
            child = subprocess.Popen(command, cwd=root, env=child_env, stdout=sink,
                                     stderr=subprocess.STDOUT, start_new_session=True)
            seen.add(child.pid)
            try:
                while True:
                    members = tagged_members(token)
                    seen.update(members)
                    peak_rss = max(peak_rss, tagged_rss_bytes(members))
                    current_live = live_bytes(root)
                    peak_live = max(peak_live, current_live)
                    raw_output_bytes = max(raw_output_bytes, output.stat().st_size)
                    elapsed = time.monotonic() - start
                    # Limits are checked even after the leader exits.
                    if elapsed > duration_s:
                        reason = "duration_limit"
                    elif current_live + reserve > disk_bytes:
                        reason = "disk_limit"
                    elif raw_output_bytes > output_allowance:
                        reason = "evidence_limit"
                    if reason:
                        terminate_tagged(token, seen)
                        break
                    if child.poll() is not None:
                        if tagged_members(token):
                            reason = "orphan_descendant"
                            terminate_tagged(token, seen)
                        break
                    time.sleep(0.01)
            finally:
                cleanup_ok = terminate_tagged(token, seen)
                try:
                    child.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
                    cleanup_ok = False
                cleanup_ok = reap_seen(seen) and cleanup_ok
        finally:
            if sys.platform == "linux":
                libc.prctl(36, old_subreaper.value, 0, 0, 0)
    raw_output_bytes = max(raw_output_bytes, output.stat().st_size)
    peak_live = max(peak_live, live_bytes(root))
    if time.monotonic() - start > duration_s and reason is None:
        reason = "duration_limit"
    if peak_live + reserve > disk_bytes and reason is None:
        reason = "disk_limit"
    if raw_output_bytes > output_allowance and reason is None:
        reason = "evidence_limit"
    if not cleanup_ok and reason is None:
        reason = "process_group_cleanup_failed"
    rate_error = None
    source_seed = None
    if rate_contract is not None:
        try:
            check_rate(root / "offered.csv", *rate_contract)
            source_seed = check_source(root / "offers.jsonl", *rate_contract)
        except ValueError as error:
            rate_error = str(error)
            reason = reason or "rate_contract"
    output_truncated = raw_output_bytes > output_allowance
    if output_truncated:
        with output.open("r+b") as sink:
            sink.truncate(output_allowance)
    result = {"invocation_id": token, "command": command, "exit_code": child.returncode,
              "elapsed_s": time.monotonic() - start,
              "peak_tagged_descendant_rss_bytes_sampled": peak_rss,
              "peak_live_bytes_sampled": peak_live,
              "raw_output_bytes_sampled": raw_output_bytes,
              "output_truncated": output_truncated,
              "process_group_cleanup_ok": cleanup_ok,
              "rate_contract_error": rate_error,
              "source_seed": source_seed,
              "stop_reason": reason, "passed": child.returncode == 0 and reason is None}
    for _ in range(4):
        report.write_text(json.dumps(result, sort_keys=True) + "\n")
        retained = live_bytes(root)
        evidence_retained = output.stat().st_size + report.stat().st_size
        if (result.get("retained_live_bytes") == retained
                and result.get("retained_evidence_bytes") == evidence_retained):
            break
        result["retained_live_bytes"] = retained
        result["retained_evidence_bytes"] = evidence_retained
    if retained > disk_bytes or evidence_retained > max_output_bytes:
        result["stop_reason"] = result["stop_reason"] or "retained_budget_limit"
        result["retained_budget_exceeded"] = True
        result["passed"] = False
        report.write_text(json.dumps(result, sort_keys=True) + "\n")
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--duration-s", required=True, type=float)
    parser.add_argument("--disk-bytes", required=True, type=int)
    parser.add_argument("--max-output-bytes", default=MAX_EVIDENCE, type=int)
    parser.add_argument("--rate-tier", type=int)
    parser.add_argument("--rate-seconds", type=int)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    try:
        validate_limits(args.duration_s, args.disk_bytes, args.max_output_bytes)
    except ValueError as error:
        parser.error(str(error))
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("missing command")
    if (args.rate_tier is None) != (args.rate_seconds is None):
        parser.error("rate tier and seconds must be supplied together")
    rate_contract = ((args.rate_tier, args.rate_seconds)
                     if args.rate_tier is not None else None)
    result = run(args.out, command, args.duration_s, args.disk_bytes,
                 args.max_output_bytes, rate_contract)
    print(json.dumps(result, sort_keys=True))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError) as error:
        print(f"alpha runner: {error}", file=sys.stderr)
        sys.exit(2)
