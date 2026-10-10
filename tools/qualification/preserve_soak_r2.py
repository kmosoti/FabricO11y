#!/usr/bin/env python3
"""Preserve, verify, then optionally remove only the completed R2 soak freeze.

The preserve action creates a private archive and per-member manifest. The
cleanup action requires that archive plus a successful frozen-post receipt.
Both actions are intended to run through tools/resource_group.py.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import sys
import tarfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from resource_group import DATA_DRIVE, require_limits, STORAGE  # noqa: E402

FREEZE = STORAGE / "scratch/soak-r2-02"
MARKER = ".fabric-soak-freeze-owned"
MARKER_TEXT = "fabric-soak-freeze-r2-v1\n"
FULL_RUN = "alpha-soak-r2-full"
RUNNER_MARKER = ".fabric-alpha-owned"
RUNNER_MARKER_TEXT = "fabric-alpha-runner-v1\n"
EXPECTED_LAUNCHER_ID = "5f9ee6b610a1418bb7ff96d172385516"
EXPECTED_LAUNCHER_UNIT = "fabric-work-" + EXPECTED_LAUNCHER_ID
RESULTS = STORAGE / "results/readiness-continuation-soak-r2-01"
ARCHIVE = RESULTS / "soak-r2-02-full-freeze.tar.gz"
MEMBERS = RESULTS / "soak-r2-02-full-freeze-members.json"
PRESERVE_RECEIPT = RESULTS / "soak-r2-02-full-freeze-preservation.json"
CLEANUP_RECEIPT = RESULTS / "soak-r2-02-full-freeze-cleanup.json"


def sha256_path(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def private_new(path):
    return os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb")


def ensure_private_external(path):
    if not DATA_DRIVE.is_mount():
        raise ValueError("required data drive is not mounted")
    RESULTS.mkdir(parents=True, exist_ok=True)
    if (RESULTS.is_symlink() or not RESULTS.is_dir()
            or RESULTS.stat().st_uid != os.getuid()):
        raise ValueError("external evidence directory is not an owned regular directory")
    path.parent.resolve(strict=True).relative_to(STORAGE.resolve(strict=True))
    if path.exists() or path.is_symlink():
        raise ValueError(f"evidence path already exists: {path}")


def require_archive_space(freeze_bytes):
    cap = 100_000_000_000
    project_bytes = 0
    for directory, dirs, files in os.walk(STORAGE, followlinks=False):
        for name in list(dirs):
            path = Path(directory) / name
            if path.is_symlink():
                dirs.remove(name)
                project_bytes += path.lstat().st_size
        for name in files:
            path = Path(directory) / name
            project_bytes += path.lstat().st_size
    reserve = freeze_bytes + 1024**3
    if project_bytes + reserve > cap or shutil.disk_usage(STORAGE).free < reserve:
        raise ValueError("archive reserve exceeds FabricO11y data budget or free space")


def ownership_checks(postcheck_path, launcher_receipt_path):
    storage = STORAGE.resolve(strict=True)
    require_no_symlink_components(FREEZE)
    owned = FREEZE.resolve(strict=True)
    owned.relative_to((storage / "scratch").resolve(strict=True))
    if owned != (storage / "scratch/soak-r2-02").resolve(strict=True):
        raise ValueError("unexpected freeze path")
    marker = owned / MARKER
    if marker.is_symlink() or not marker.is_file() or marker.read_text() != MARKER_TEXT:
        raise ValueError("exact freeze ownership marker missing")

    run = owned / "frozen/target" / FULL_RUN
    if run.is_symlink() or not run.is_dir():
        raise ValueError("full R2 runner output missing")
    run_marker = run / RUNNER_MARKER
    if run_marker.is_symlink() or not run_marker.is_file() or run_marker.read_text() != RUNNER_MARKER_TEXT:
        raise ValueError("full R2 runner ownership marker missing")
    runner_result = json.loads((run / "result.json").read_text())
    token = runner_result.get("invocation_id")
    if (not isinstance(token, str) or not token or not runner_result.get("process_group_cleanup_ok")
            or runner_result.get("stop_reason") is not None):
        raise ValueError("full runner did not record complete child-process cleanup")
    if tagged_processes(token):
        raise ValueError("full-run tagged processes remain live")

    postcheck_path = Path(postcheck_path)
    launcher_receipt_path = Path(launcher_receipt_path)
    for evidence in (postcheck_path, launcher_receipt_path):
        if evidence.is_symlink() or not evidence.is_file() or evidence.stat().st_nlink != 1:
            raise ValueError("postcheck and launcher receipt must be regular unlinked files")
        evidence.resolve(strict=True).relative_to(storage)
    launcher = json.loads(launcher_receipt_path.read_text())
    child = launcher.get("command_receipt")
    command = launcher.get("command")
    validate_launcher_receipt(launcher, child, command)
    if launcher.get("exit") != child.get("exit"):
        raise ValueError("launcher and command exits disagree")
    cgroup = Path(child["cgroup"])
    if (not cgroup.is_absolute() or cgroup.parts[:4] != ("/", "sys", "fs", "cgroup")
            or cgroup.name != EXPECTED_LAUNCHER_UNIT + ".service"
            or ".." in cgroup.parts):
        raise ValueError("launcher cgroup path does not identify the exact full-soak unit")
    if cgroup.exists():
        events_path = cgroup / "cgroup.events"
        events = dict(line.split() for line in events_path.read_text().splitlines())
        if events.get("populated") != "0":
            raise ValueError("full-soak unit cgroup remains populated")

    postcheck = json.loads(postcheck_path.read_text())
    manifest_path = owned / "provenance/manifest.json"
    validate_postcheck(postcheck, manifest_path, sha256_path(manifest_path))
    return owned, runner_result, launcher, postcheck


def require_no_symlink_components(path):
    absolute = Path(os.path.abspath(path))
    cursor = Path(absolute.anchor)
    for part in absolute.parts[1:]:
        cursor = cursor / part
        if cursor.is_symlink():
            raise ValueError(f"symlink in pinned data path: {cursor}")
        if cursor.exists() and not cursor.is_dir():
            raise ValueError(f"non-directory in pinned data path: {cursor}")


def validate_launcher_receipt(launcher, child, command):
    if (launcher.get("unit") != EXPECTED_LAUNCHER_UNIT
            or launcher.get("stop_confirmed") is not True
            or not isinstance(child, dict) or child.get("state") != "completed"
            or not isinstance(command, list) or not any(FULL_RUN in str(arg) for arg in command)):
        raise ValueError("launcher receipt does not prove the named full soak unit stopped")
    return True


def validate_postcheck(postcheck, manifest_path, manifest_sha):
    if (postcheck.get("passed") is not True or postcheck.get("label") != "full-post"
            or postcheck.get("manifest") != str(manifest_path)
            or postcheck.get("manifest_sha256") != manifest_sha):
        raise ValueError("successful full-post verification of the exact current manifest is required")
    return True


def tagged_processes(token):
    needle = f"FABRIC_ALPHA_INVOCATION={token}".encode()
    found = []
    for entry in os.scandir("/proc"):
        if not entry.name.isdigit():
            continue
        try:
            proc = Path("/proc") / entry.name
            if proc.stat().st_uid != os.getuid():
                continue
            values = (proc / "environ").read_bytes().split(b"\0")
            if needle in values:
                state = (proc / "stat").read_text().rpartition(") ")[2].split()[0]
                if state != "Z":
                    found.append(int(entry.name))
        except (FileNotFoundError, ProcessLookupError, PermissionError, IndexError):
            continue
    return found


def tree_manifest(root):
    result = [{"path": ".", "type": "directory",
               "mode": stat.S_IMODE(root.stat().st_mode), "size": 0, "sha256": None}]
    for directory, dirs, files in os.walk(root, topdown=True, followlinks=False):
        base = Path(directory)
        dirs.sort()
        files.sort()
        for name in dirs:
            path = base / name
            if path.is_symlink() or not path.is_dir():
                raise ValueError(f"unsupported directory entry: {path}")
            result.append({"path": path.relative_to(root).as_posix(), "type": "directory",
                           "mode": stat.S_IMODE(path.stat().st_mode), "size": 0, "sha256": None})
        for name in files:
            path = base / name
            info = path.lstat()
            if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
                raise ValueError(f"archive accepts only unlinked regular files: {path}")
            result.append({"path": path.relative_to(root).as_posix(), "type": "file",
                           "mode": stat.S_IMODE(info.st_mode), "size": info.st_size,
                           "sha256": sha256_path(path)})
    return sorted(result, key=lambda row: row["path"])


def archive_tree(root, expected):
    with private_new(ARCHIVE) as raw:
        with tarfile.open(fileobj=raw, mode="w:gz", format=tarfile.PAX_FORMAT) as archive:
            for row in expected:
                if row["path"] == ".":
                    archive.add(root, arcname=root.name, recursive=False)
                else:
                    path = root / PurePosixPath(row["path"])
                    archive.add(path, arcname=(root.name + "/" + row["path"]), recursive=False)
        raw.flush()
        os.fsync(raw.fileno())
    os.chmod(ARCHIVE, 0o600)
    verify_archive(ARCHIVE, root.name, expected)


def verify_archive(path, top_name, expected):
    seen = []
    expected_by_path = {row["path"]: row for row in expected}
    if len(expected_by_path) != len(expected):
        raise ValueError("duplicate paths in expected archive manifest")
    with tarfile.open(path, "r:gz") as archive:
        for member in archive:
            relative = PurePosixPath(member.name)
            if (relative.is_absolute() or ".." in relative.parts or not relative.parts
                    or relative.parts[0] != top_name):
                raise ValueError("unsafe archive member path")
            rel = PurePosixPath(*relative.parts[1:]).as_posix() if len(relative.parts) > 1 else "."
            info = expected_by_path.get(rel)
            if info is None or member.issym() or member.islnk() or member.isdev():
                raise ValueError(f"unexpected archive member {member.name}")
            expected_type = "directory" if member.isdir() else "file" if member.isfile() else None
            if expected_type != info["type"] or stat.S_IMODE(member.mode) != info["mode"]:
                raise ValueError(f"archive member metadata mismatch: {rel}")
            actual_digest = None
            actual_size = 0
            if member.isfile():
                stream = archive.extractfile(member)
                if stream is None:
                    raise ValueError(f"cannot read archived member {rel}")
                digest = hashlib.sha256()
                for block in iter(lambda: stream.read(1024 * 1024), b""):
                    actual_size += len(block)
                    digest.update(block)
                actual_digest = digest.hexdigest()
            if actual_size != info["size"] or actual_digest != info["sha256"]:
                raise ValueError(f"archive bytes differ: {rel}")
            seen.append(rel)
    wanted = [row["path"] for row in expected]
    if sorted(seen) != wanted or len(seen) != len(set(seen)):
        raise ValueError("archive members are missing, duplicated or extra")
    return {"member_count": len(seen), "archive_sha256": sha256_path(path),
            "members_sha256": hashlib.sha256(json.dumps(expected, sort_keys=True,
                                                        separators=(",", ":")).encode()).hexdigest()}


def validate_preservation(receipt, manifest, owned, archive_sha, member_sha):
    if (receipt.get("state") != "preserved_and_verified"
            or receipt.get("archive_sha256") != archive_sha
            or manifest.get("freeze") != owned
            or manifest.get("archive_sha256") != receipt.get("archive_sha256")
            or manifest.get("members_sha256") != receipt.get("members_sha256")
            or member_sha != receipt.get("members_sha256")):
        raise ValueError("preservation receipt or archive digest mismatch")
    return True


def preserve(postcheck, launcher_receipt):
    for path in (ARCHIVE, MEMBERS, PRESERVE_RECEIPT, CLEANUP_RECEIPT,
                 CLEANUP_RECEIPT.with_name(CLEANUP_RECEIPT.name + ".pending")):
        ensure_private_external(path)
    owned, runner, launcher, check = ownership_checks(postcheck, launcher_receipt)
    expected = tree_manifest(owned)
    require_archive_space(sum(row["size"] for row in expected if row["type"] == "file"))
    archive_tree(owned, expected)
    if tree_manifest(owned) != expected:
        raise ValueError("owned tree changed during archival")
    verification = verify_archive(ARCHIVE, owned.name, expected)
    members_payload = {"freeze": str(owned), "members": expected, **verification}
    with private_new(MEMBERS) as stream:
        stream.write((json.dumps(members_payload, sort_keys=True, indent=2) + "\n").encode())
        stream.flush()
        os.fsync(stream.fileno())
    receipt = {"state": "preserved_and_verified", "freeze": str(owned),
               "launcher_unit": launcher["unit"], "launcher_exit": launcher["exit"],
               "runner_exit": runner["exit_code"], "postcheck": str(postcheck),
               "archive": str(ARCHIVE), "archive_sha256": verification["archive_sha256"],
               "members_manifest": str(MEMBERS), "members_sha256": verification["members_sha256"],
               "member_count": verification["member_count"],
               "credentials_classification": "synthetic owned soak fixtures; archive and receipts mode 0600",
               "ownership_manifest_sha256": check["manifest_sha256"]}
    with private_new(PRESERVE_RECEIPT) as stream:
        stream.write((json.dumps(receipt, sort_keys=True, indent=2) + "\n").encode())
        stream.flush()
        os.fsync(stream.fileno())
    return receipt


def cleanup(postcheck, launcher_receipt):
    owned, runner, launcher, check = ownership_checks(postcheck, launcher_receipt)
    if (RESULTS.is_symlink() or not RESULTS.is_dir()
            or RESULTS.stat().st_uid != os.getuid()):
        raise ValueError("external evidence directory is not an owned regular directory")
    if any(path.is_symlink() or not path.is_file() or path.stat().st_nlink != 1
           for path in (ARCHIVE, MEMBERS, PRESERVE_RECEIPT)):
        raise ValueError("verified archive, member manifest and preservation receipt required")
    if any(path.stat().st_mode & 0o077 or path.stat().st_uid != os.getuid()
           for path in (ARCHIVE, MEMBERS, PRESERVE_RECEIPT)):
        raise ValueError("preservation evidence permissions must remain private")
    receipt = json.loads(PRESERVE_RECEIPT.read_text())
    manifest = json.loads(MEMBERS.read_text())
    expected = manifest["members"]
    member_digest = hashlib.sha256(json.dumps(expected, sort_keys=True,
                                               separators=(",", ":")).encode()).hexdigest()
    validate_preservation(receipt, manifest, str(owned), sha256_path(ARCHIVE), member_digest)
    verify_archive(ARCHIVE, owned.name, expected)
    if tree_manifest(owned) != expected:
        raise ValueError("live freeze differs from verified archive member manifest")
    if CLEANUP_RECEIPT.exists() or CLEANUP_RECEIPT.is_symlink():
        raise ValueError("cleanup receipt already exists")
    pending = CLEANUP_RECEIPT.with_name(CLEANUP_RECEIPT.name + ".pending")
    if pending.exists() or pending.is_symlink():
        raise ValueError("cleanup receipt temporary path already exists")
    if tagged_processes(runner["invocation_id"]):
        raise ValueError("full-run process appeared before cleanup")
    started = {"state": "cleanup_started", "freeze": str(owned),
               "archive_sha256": receipt["archive_sha256"],
               "members_manifest_sha256": sha256_path(MEMBERS),
               "launcher_unit": launcher["unit"], "postcheck": str(postcheck)}
    with private_new(CLEANUP_RECEIPT) as stream:
        stream.write((json.dumps(started, sort_keys=True, indent=2) + "\n").encode())
        stream.flush()
        os.fsync(stream.fileno())
    shutil.rmtree(owned)
    if owned.exists():
        raise ValueError("owned freeze remains after deletion")
    cleanup_receipt = {"state": "owned_freeze_removed", "freeze": str(owned),
                       "archive_sha256": receipt["archive_sha256"],
                       "members_manifest_sha256": sha256_path(MEMBERS),
                       "launcher_unit": launcher["unit"], "postcheck": str(postcheck)}
    with private_new(pending) as stream:
        stream.write((json.dumps(cleanup_receipt, sort_keys=True, indent=2) + "\n").encode())
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(pending, CLEANUP_RECEIPT)
    return cleanup_receipt


def main():
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("preserve", "cleanup"))
    parser.add_argument("--postcheck", required=True, type=Path)
    parser.add_argument("--launcher-receipt", required=True, type=Path)
    args = parser.parse_args()
    if args.action == "preserve":
        result = preserve(args.postcheck, args.launcher_receipt)
    else:
        result = cleanup(args.postcheck, args.launcher_receipt)
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, KeyError, TypeError, tarfile.TarError) as error:
        print(f"soak preservation: NOT COMPLETE: {error}", file=sys.stderr)
        raise SystemExit(2)
