#!/usr/bin/env python3
"""Read-only, bounded joins of caller-pinned evidence; never execute a plan."""
import argparse
import contextlib
import hashlib
import heapq
import json
import math
import os
from pathlib import Path
import re
import stat
import sys

JSON_LIMIT = 4 * 1024**2
JSON_TOTAL_LIMIT = 64 * 1024**2
CHUNK = 1024**2
FILE_LIMIT = 8 * 1024**3
TOTAL_LIMIT = 32 * 1024**3
ITEM_LIMIT = 4096
EDGE_LIMIT = 65536
ID = re.compile(r"[a-z][a-z0-9-]{0,79}\Z")
SHA = re.compile(r"[0-9a-f]{64}\Z")
REVISION = re.compile(r"[0-9a-f]{40}\Z")
UNIT = re.compile(r"fabric-work-[0-9a-f]+\Z")


def _require(condition, message):
    if not condition:
        raise ValueError(message)


def strict_json(raw: bytes) -> dict:
    """Decode one bounded object, rejecting duplicates and non-JSON numbers."""
    _require(type(raw) is bytes and len(raw) <= JSON_LIMIT, "JSON size/type")
    text = raw.decode("utf-8", errors="strict")
    depth, quoted, escaped = 0, False, False
    for character in text:
        if quoted:
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == '"':
                quoted = False
        elif character == '"':
            quoted = True
        elif character in "[{":
            depth += 1
            _require(depth <= 32, "JSON nesting exceeds 32")
        elif character in "]}":
            depth -= 1

    def pairs(entries):
        result = {}
        for key, value in entries:
            _require(key not in result, "duplicate JSON key")
            result[key] = value
        return result

    def finite(value):
        result = float(value)
        _require(math.isfinite(result), "non-finite JSON number")
        return result

    def invalid_constant(value):
        raise ValueError("non-finite JSON constant: " + value)

    result = json.loads(text, object_pairs_hook=pairs, parse_float=finite,
                        parse_constant=invalid_constant)
    _require(type(result) is dict, "JSON root must be an object")
    return result


def _matches(pattern, value):
    return type(value) is str and pattern.fullmatch(value) is not None


def _keys(value, keys, label):
    _require(type(value) is dict and set(value) == set(keys), label + " keys")


def ordered_jobs(jobs: list[dict]) -> list[str]:
    """Lexicographic Kahn traversal; validate only the id/needs graph fields."""
    _require(type(jobs) is list and 0 < len(jobs) <= ITEM_LIMIT, "job count")
    indexed = {}
    for job in jobs:
        _require(type(job) is dict and _matches(ID, job.get("id")), "job id")
        _require(job["id"] not in indexed, "duplicate job id")
        indexed[job["id"]] = job
    children = {name: [] for name in indexed}
    indegrees = {}
    edges = 0
    for name, job in indexed.items():
        needs = job.get("needs")
        _require(type(needs) is list, "needs must be a list")
        edges += len(needs)
        _require(edges <= EDGE_LIMIT, "edge count")
        seen = set()
        for parent in needs:
            _require(_matches(ID, parent) and parent in indexed and parent != name,
                     "unknown or self dependency")
            _require(parent not in seen, "duplicate dependency")
            seen.add(parent)
            children[parent].append(name)
        indegrees[name] = len(needs)
    ready = [name for name, count in indegrees.items() if count == 0]
    heapq.heapify(ready)
    result = []
    while ready:
        name = heapq.heappop(ready)
        result.append(name)
        for child in children[name]:
            indegrees[child] -= 1
            if indegrees[child] == 0:
                heapq.heappush(ready, child)
    _require(len(result) == len(jobs), "dependency cycle")
    return result


def _parts(path):
    _require(type(path) is str and path and not path.startswith("/")
             and "\x00" not in path, "relative POSIX path required")
    parts = path.split("/")
    _require(all(part not in ("", ".", "..") for part in parts),
             "noncanonical path")
    return parts


def _identity(info):
    return (info.st_dev, info.st_ino, info.st_size,
            info.st_mtime_ns, info.st_ctime_ns)


class ArtifactStore:
    """One invocation's descriptors and digest index; no persistent cache."""
    def __init__(self, root):
        self.root = str(root)
        _require(self.root.startswith("/"), "absolute root required")
        if self.root != "/":
            _parts(self.root[1:])
        self.root_fd = self._absolute_root()
        try:
            self.root_identity = os.fstat(self.root_fd)
        except BaseException:
            os.close(self.root_fd)
            raise
        self.records = {}
        self.total = 0
        self.bytes_hashed = 0
        self.json_total = 0

    def _absolute_root(self):
        descriptor = os.open("/", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            for part in ([] if self.root == "/" else _parts(self.root[1:])):
                child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                dir_fd=descriptor)
                os.close(descriptor)
                descriptor = child
            return descriptor
        except BaseException:
            os.close(descriptor)
            raise

    def close(self):
        os.close(self.root_fd)

    @contextlib.contextmanager
    def parent(self, path):
        parts = _parts(path)
        descriptor = os.dup(self.root_fd)
        try:
            for part in parts[:-1]:
                child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                dir_fd=descriptor)
                os.close(descriptor)
                descriptor = child
            yield descriptor, parts[-1]
        finally:
            os.close(descriptor)

    def _stat(self, path):
        with self.parent(path) as (parent, name):
            info = os.stat(name, dir_fd=parent, follow_symlinks=False)
        _require(stat.S_ISREG(info.st_mode), "artifact is not regular")
        return _identity(info)

    def read(self, path, expected, as_json=False, *, artifact=True):
        _parts(path)
        _require(_matches(SHA, expected), "invalid SHA-256")
        if path in self.records:
            old = self.records[path]
            _require(old["sha256"] == expected, "conflicting path hash")
            if artifact and not old["artifact"]:
                _require(self.total + old["identity"][2] <= TOTAL_LIMIT,
                         "total artifact size limit")
                self.total += old["identity"][2]
                old["artifact"] = True
            if as_json and "json" not in old:
                # JSON consumers are identified before hashing artifacts.
                raise ValueError("JSON payload was not admitted")
            return old.get("json")
        with self.parent(path) as (parent, name):
            descriptor = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                                 dir_fd=parent)
            try:
                info = os.fstat(descriptor)
                _require(stat.S_ISREG(info.st_mode), "artifact is not regular")
                before = _identity(info)
                _require(before == self._stat(path), "artifact replaced before read")
                _require(info.st_size <= FILE_LIMIT, "artifact size limit")
                _require(not artifact or self.total + info.st_size <= TOTAL_LIMIT,
                         "total artifact size limit")
                if as_json:
                    _require(info.st_size <= JSON_LIMIT, "JSON size limit")
                    _require(self.json_total + info.st_size <= JSON_TOTAL_LIMIT,
                             "aggregate JSON size limit")
                digest, payload, size = hashlib.sha256(), bytearray(), 0
                while True:
                    chunk = os.read(descriptor, CHUNK)
                    if not chunk:
                        break
                    size += len(chunk)
                    _require(size <= FILE_LIMIT and
                             (not artifact or self.total + size <= TOTAL_LIMIT),
                             "streamed artifact size limit")
                    digest.update(chunk)
                    self.bytes_hashed += len(chunk)
                    if as_json:
                        _require(size <= JSON_LIMIT, "JSON size limit")
                        _require(self.json_total + size <= JSON_TOTAL_LIMIT,
                                 "aggregate JSON size limit")
                        payload.extend(chunk)
                _require(before == _identity(os.fstat(descriptor)) == self._stat(path)
                         and size == info.st_size, "artifact changed during read")
            finally:
                os.close(descriptor)
        actual = digest.hexdigest()
        _require(actual == expected, "artifact hash mismatch: " + path)
        record = {"identity": before, "sha256": actual, "artifact": artifact}
        if as_json:
            record["json"] = strict_json(bytes(payload))
            self.json_total += size
        self.records[path] = record
        if artifact:
            self.total += size
        return record.get("json")

    def recheck(self):
        descriptor = self._absolute_root()
        try:
            current = os.fstat(descriptor)
            _require((current.st_dev, current.st_ino) ==
                     (self.root_identity.st_dev, self.root_identity.st_ino), "root replaced")
        finally:
            os.close(descriptor)
        for path, record in self.records.items():
            _require(self._stat(path) == record["identity"], "artifact changed after read")


def _command(command):
    _require(type(command) is list and 0 < len(command) <= 256, "command count")
    for argument in command:
        _require(type(argument) is str and argument and len(argument) <= CHUNK,
                 "command argument")
        _require(len(argument.encode("utf-8")) <= CHUNK, "command argument size")


def _exit(value, expected):
    _require(type(value) is int and 0 <= value <= 255 and value == expected,
             "exit mismatch or type")


def _receipt_path(path, root):
    _require(type(path) is str, "receipt path type")
    if path.startswith("/"):
        prefix = root.rstrip("/") + "/"
        _require(path.startswith(prefix), "receipt path outside root")
        path = path[len(prefix):]
    _parts(path)
    return path


def _join(job, wrapper, resource, artifact, root):
    expected, command, revision = job["expected_exit"], job["command"], job["harness_revision"]
    _require(wrapper.get("state") == "completed" and wrapper.get("command") == command,
             "wrapper state/command")
    _require(wrapper.get("harness_revision") == revision, "wrapper harness revision")
    _require(wrapper.get("source_status_before") == "" and
             wrapper.get("source_status_after") == "", "dirty source status")
    for field in ("exit", "launcher_exit"):
        _exit(wrapper.get(field), expected)
    entries = wrapper.get("resource_receipts")
    _require(type(entries) is list and len(entries) == 1 and type(entries[0]) is dict,
             "exactly one resource receipt required")
    entry = entries[0]
    _require(_receipt_path(entry.get("path"), root) == artifact["path"] and
             entry.get("sha256") == artifact["sha256"], "resource pin mismatch")
    _exit(entry.get("exit"), expected)
    _require(entry.get("stop_confirmed") is True, "wrapper stop unconfirmed")
    _require(resource.get("command") == command and resource.get("storage_root") == root,
             "resource command/storage root")
    for field in ("exit", "wrapper_exit"):
        _exit(resource.get(field), expected)
    _require(resource.get("stop_confirmed") is True and
             "stop_error" in resource and resource["stop_error"] is None,
             "resource stop unconfirmed")
    unit = resource.get("unit")
    _require(_matches(UNIT, unit), "resource unit")
    child = resource.get("command_receipt")
    _require(type(child) is dict and child.get("state") == "completed" and
             child.get("command") == command and child.get("unit") == unit,
             "child receipt mismatch")
    _exit(child.get("exit"), expected)
    cgroup = child.get("cgroup")
    _require(type(cgroup) is str and cgroup.startswith("/sys/fs/cgroup/"), "child cgroup")
    parts = _parts(cgroup[1:])
    service = unit + ".service"
    direct = parts[-1] == service
    delegated = (len(parts) >= 5 and parts[-2:] == [service, "workload"]
                 and resource.get("delegated") is True)
    _require(direct or delegated, "cgroup unit mismatch")
    if "cgroup" in resource:
        _require(resource["cgroup"] == cgroup, "resource/child cgroup mismatch")
    for receipt in (resource, child):
        if "revision" in receipt:
            _require(receipt["revision"] == revision, "child revision mismatch")


def verify_plan(root: Path, plan_path: str, plan_sha256: str, candidate: str) -> dict:
    """Check pinned bytes and receipt joins, returning only structural states."""
    _require(_matches(REVISION, candidate), "candidate revision")
    files = ArtifactStore(root)
    try:
        plan = files.read(plan_path, plan_sha256, as_json=True, artifact=False)
        _keys(plan, ("version", "candidate", "artifacts", "jobs"), "plan")
        _require(type(plan["version"]) is int and plan["version"] == 1, "plan version")
        _require(plan["candidate"] == candidate, "candidate disagreement")
        artifacts, jobs = plan["artifacts"], plan["jobs"]
        _require(type(artifacts) is list and len(artifacts) <= ITEM_LIMIT, "artifact count")
        order = ordered_jobs(jobs)
        indexed, paths = {}, set()
        for artifact in artifacts:
            _keys(artifact, ("id", "path", "sha256"), "artifact")
            _require(_matches(ID, artifact["id"]) and artifact["id"] not in indexed,
                     "artifact id duplicate/type")
            _parts(artifact["path"])
            _require(artifact["path"] not in paths, "duplicate artifact path")
            _require(_matches(SHA, artifact["sha256"]), "artifact digest")
            indexed[artifact["id"]] = artifact
            paths.add(artifact["path"])
        receipts, by_job = set(), {}
        for job in jobs:
            _keys(job, ("id", "needs", "command", "harness_revision", "expected_exit", "evidence"), "job")
            _command(job["command"])
            _require(_matches(REVISION, job["harness_revision"]), "harness revision")
            _exit(job["expected_exit"], job["expected_exit"])
            evidence = job["evidence"]
            if evidence is not None:
                _keys(evidence, ("wrapper", "resource"), "evidence")
                for receipt in evidence.values():
                    _require(_matches(ID, receipt) and receipt in indexed,
                             "unknown receipt artifact")
                    _require(receipt not in receipts, "shared receipt")
                    receipts.add(receipt)
            by_job[job["id"]] = job
        objects = {}
        for name, artifact in indexed.items():
            value = files.read(artifact["path"], artifact["sha256"], name in receipts)
            if name in receipts:
                objects[name] = value
        states, output, units = {}, [], set()
        for name in order:
            job = by_job[name]
            admitted = all(states[parent] == "matched" for parent in job["needs"])
            evidence = job["evidence"]
            if evidence is None:
                state = "pending" if admitted else "blocked"
            else:
                _require(admitted, "evidence with unmet dependency")
                _join(job, objects[evidence["wrapper"]], objects[evidence["resource"]],
                      indexed[evidence["resource"]], files.root)
                unit = objects[evidence["resource"]]["unit"]
                _require(unit not in units, "resource unit shared between jobs")
                units.add(unit)
                state = "matched"
            states[name] = state
            output.append({"id": name, "state": state})
        files.recheck()
        return {"status": "matched" if all(s == "matched" for s in states.values()) else "pending",
                "jobs": output}
    finally:
        files.close()


class _Parser(argparse.ArgumentParser):
    def error(self, message):
        raise ValueError(message)


def main(arguments=None):
    try:
        parser = _Parser(description=__doc__, add_help=False)
        for name in ("root", "plan", "plan-sha256", "candidate"):
            parser.add_argument("--" + name, required=True)
        args = parser.parse_args(arguments)
        # Support both `python -m tools.engineering.evidence` and a script path.
        sys.dont_write_bytecode = True
        sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
        from tools import resource_group
        resource_group.require_limits()
        _require(args.root == str(resource_group.STORAGE), "root differs from launcher storage")
        result = verify_plan(Path(args.root), args.plan, args.plan_sha256, args.candidate)
        print(json.dumps(result, sort_keys=True))
        return 0 if result["status"] == "matched" else 3
    except (ValueError, OSError, RuntimeError) as error:
        print(json.dumps({"status": "invalid", "error": str(error)}, sort_keys=True))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
