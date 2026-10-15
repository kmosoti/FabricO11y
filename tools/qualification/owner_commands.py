"""Bounded command transport for the registered fresh-owner walkthrough.

Guide blocks remain unchanged. A fresh noninteractive bash runs each block in
the guest's login directory; source spans and that shell boundary are recorded.
Tokens are stdin, never shell text or argv. Raw output is private evidence. On
any failure the caller must stop the complete guest pair before another action.
This transport does not grade the walkthrough or replace semantic observations.
"""
import hashlib
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import time


MAX_OUTPUT = 8 * 1024**2
PHASES = (
    ("boot", 600), ("install", 120), ("tls", 120), ("first-owner", 90),
    ("edge", 180), ("queries-revoke-restart", 120), ("backup-restore-recovery", 240),
    ("upgrade", 120), ("remove-reset-reenroll", 165), ("archive-stop", 45),
)


def private_write(path, raw):
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as stream:
        stream.write(raw)


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for part in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(part)
    return value.hexdigest()


class Commands:
    def __init__(self, evidence):
        self.evidence = Path(evidence)
        self.evidence.mkdir(mode=0o700, exist_ok=False)
        self.index = -1
        self.deadline_ns = None
        self.records = []
        self.failed = False

    def phase(self, name):
        if self.failed or self.index + 1 >= len(PHASES) or PHASES[self.index + 1][0] != name:
            raise ValueError("walkthrough phase omitted, reordered or resumed after failure")
        now = time.monotonic_ns()
        if self.deadline_ns is not None and now > self.deadline_ns:
            self.failed = True
            raise TimeoutError("previous walkthrough phase exceeded its allocation")
        self.index += 1
        self.deadline_ns = now + PHASES[self.index][1] * 1_000_000_000
        self.records.append({"kind": "phase", "phase": name, "start_monotonic_ns": now,
                             "deadline_monotonic_ns": self.deadline_ns})
        self.save()

    def remaining(self):
        if self.failed or self.deadline_ns is None:
            raise ValueError("active nonfailed walkthrough phase required")
        remaining = (self.deadline_ns - time.monotonic_ns()) / 1_000_000_000
        if remaining <= 0:
            self.failed = True
            raise TimeoutError("walkthrough phase exceeded its allocation")
        return remaining

    def save(self):
        # Each checkpoint is exclusive; interrupted runs keep their earlier records.
        private_write(self.evidence / f"transcript-{len(self.records):04d}.json",
                      (json.dumps({"classification": "ungraded command transcript",
                                   "failed": self.failed, "records": self.records}, indent=2) + "\n").encode())

    def shell(self, guest, block, *, stdin=None):
        """Execute one admitted block verbatim using Guest's pinned SSH transport."""
        self.remaining()
        if not guest.boot_id or guest.role not in ("server", "edge"):
            raise ValueError("declared booted guest role required")
        if stdin is not None and (not isinstance(stdin, bytes) or len(stdin) > 1024):
            raise ValueError("bounded protected stdin required")
        # SSH joins remote arguments with spaces. Supply a single correctly quoted
        # command string, not an argv list whose shell syntax would be reinterpreted.
        remote = shlex.join(["bash", "-c", block.command.decode("utf-8")])
        command = [*guest.ssh, remote]
        sequence = len(self.records)
        stdout_path = self.evidence / f"command-{sequence:04d}.stdout"
        stderr_path = self.evidence / f"command-{sequence:04d}.stderr"
        record = {"kind": "guide-command", "phase": PHASES[self.index][0],
                  "role": guest.role, "family": guest.family, "boot_id": guest.boot_id,
                  "source": block.record(), "substitutions": [],
                  "shell": "fresh bash -c in SSH login directory",
                  "protected_stdin": stdin is not None,
                  "start_monotonic_ns": time.monotonic_ns(), "exit": None}
        child = None
        try:
            with os.fdopen(os.open(stdout_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as out, \
                 os.fdopen(os.open(stderr_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as err:
                child = subprocess.Popen(command, stdin=subprocess.PIPE if stdin is not None else subprocess.DEVNULL,
                                         stdout=out, stderr=err, start_new_session=True)
                if stdin is not None:
                    child.stdin.write(stdin)
                    child.stdin.close()
                while True:
                    remaining = self.remaining()
                    if stdout_path.stat().st_size + stderr_path.stat().st_size > MAX_OUTPUT:
                        raise RuntimeError("guide command output exceeded its evidence bound")
                    try:
                        record["exit"] = child.wait(timeout=min(.1, remaining))
                        break
                    except subprocess.TimeoutExpired:
                        continue
                if stdout_path.stat().st_size + stderr_path.stat().st_size > MAX_OUTPUT:
                    raise RuntimeError("guide command output exceeded its evidence bound")
                if record["exit"] != 0:
                    raise RuntimeError("guide command failed; stop guest pair before continuing")
                self.remaining()
        except BaseException as error:
            self.failed = True
            record["failure_type"] = type(error).__name__
            raise
        finally:
            # This owns only SSH. Remote descendants are owned by the caller's VM
            # boundary; no resumed command is allowed after a transport failure.
            if child is not None and child.poll() is None:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait(timeout=5)
            record["end_monotonic_ns"] = time.monotonic_ns()
            record["ssh_reaped"] = child is not None and child.poll() is not None
            for label, path in (("stdout", stdout_path), ("stderr", stderr_path)):
                if path.exists():
                    record[label] = {"path": str(path), "bytes": path.stat().st_size,
                                     "sha256": digest(path)}
            self.records.append(record)
            self.save()
        return record
