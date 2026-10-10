#!/usr/bin/env python3
"""Build the install .deb in a disposable, rootless Debian 12 container.

The Debian 12 build sysroot keeps Rust 1.98's weak pidfd references within the
registered GLIBC 2.34 floor; installed-system acceptance still runs on Debian 13.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "tools"))
from resource_group import STORAGE, require_limits
sys.path.insert(0, str(ROOT / "tools/qualification"))
from prepare_soak import MAX_COPY_BYTES, source_paths

DATA_MOUNT = Path("/run/media/kmosoti/data")
TOOLCHAIN_HOME = STORAGE / "toolchain-cache/rustup"
TOOLCHAIN = TOOLCHAIN_HOME / "toolchains/1.98.0-x86_64-unknown-linux-gnu"
IMAGE_TAG = "docker.io/library/debian:12-slim"
MAX_DATA_BYTES = 100 * 1000**3
RUN_BUDGET_BYTES = 8 * 1024**3
PULL_TIMEOUT_S = 300
BUILD_TIMEOUT_S = 1500


def sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def command(args: list[str], *, timeout: int = 60,
            env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, text=True, capture_output=True, timeout=timeout,
                          check=False, env=env)


def require_data_budget() -> None:
    disk = shutil.disk_usage(DATA_MOUNT)
    project_bytes = file_tree_bytes(STORAGE)
    if project_bytes + RUN_BUDGET_BYTES > MAX_DATA_BYTES:
        raise RuntimeError(
            f"FabricO11y data tree plus {RUN_BUDGET_BYTES} byte run budget exceeds 100 GB: {project_bytes}"
        )
    if disk.free < RUN_BUDGET_BYTES:
        raise RuntimeError(
            f"data drive has {disk.free} free bytes; {RUN_BUDGET_BYTES} are required"
        )


def file_tree_bytes(root: Path) -> int:
    total = 0
    for base, directories, files in os.walk(root, followlinks=False):
        for name in list(directories):
            path = Path(base) / name
            if path.is_symlink():
                directories.remove(name)
                total += path.lstat().st_size
        for name in files:
            total += (Path(base) / name).lstat().st_size
    return total


def copy_registry(source: Path, destination: Path) -> tuple[int, str]:
    """Copy the offline Cargo cache into this run's writable, owned volume."""
    for base, directories, files in os.walk(source, followlinks=False):
        for name in directories + files:
            path = Path(base) / name
            if path.is_symlink():
                raise RuntimeError(f"Cargo cache contains a symlink: {path}")
    shutil.copytree(source, destination, symlinks=False)
    entries: list[tuple[str, int, str]] = []
    total = 0
    for base, directories, files in os.walk(destination, followlinks=False):
        directories.sort()
        for name in sorted(files):
            path = Path(base) / name
            if path.is_symlink() or not path.is_file():
                raise RuntimeError(f"unexpected non-regular Cargo cache entry: {path}")
            size = path.stat().st_size
            total += size
            entries.append((path.relative_to(destination).as_posix(), size, sha256(path)))
    digest = hashlib.sha256(json.dumps(entries, separators=(",", ":")).encode()).hexdigest()
    return total, digest


def source_inventory() -> tuple[list[str], list[str]]:
    # Match the known Rust/Cargo input allowlist used by prepare_soak, then
    # include packaging inputs and the one repository-local Cargo config.
    present = set(source_paths())
    known = set(source_paths(include_missing=True))
    packaging = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", "packaging"],
        cwd=ROOT, check=True, capture_output=True,
    ).stdout.decode().split("\0")
    for name in packaging:
        if name:
            known.add(name)
            if (ROOT / name).is_file():
                present.add(name)
    cargo_config = ".cargo/config.toml"
    known.add(cargo_config)
    if (ROOT / cargo_config).is_file():
        present.add(cargo_config)
    for required in ("Cargo.toml", "Cargo.lock", "packaging/build-deb.sh",
                     "packaging/check-glibc.sh"):
        if required not in present:
            raise RuntimeError(f"required build input is missing: {required}")
    return sorted(present), sorted(known - present)


def freeze_source(destination: Path, provenance: Path) -> dict[str, object]:
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    branch = subprocess.check_output(["git", "branch", "--show-current"], cwd=ROOT,
                                     text=True).strip()
    files, deleted = source_inventory()
    status_paths = sorted(set(files + deleted))
    status = subprocess.check_output(
        ["git", "status", "--porcelain=v1", "--", *status_paths], cwd=ROOT, text=True
    )
    diff = subprocess.run(["git", "diff", "--binary", head, "--", *status_paths],
                          cwd=ROOT, check=True, capture_output=True).stdout
    (provenance / "source-diff.patch").write_bytes(diff)

    sizes = 0
    before: dict[str, str] = {}
    for name in files:
        path = ROOT / name
        if path.is_symlink() or not path.is_file():
            raise RuntimeError(f"build input is not a regular file: {path}")
        before[name] = sha256(path)
        sizes += path.stat().st_size
    if sizes > MAX_COPY_BYTES:
        raise RuntimeError(f"allowlisted source is larger than {MAX_COPY_BYTES} bytes")

    destination.mkdir()
    members: dict[str, dict[str, int | str]] = {}
    for name in files:
        source = ROOT / name
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
        copied_hash = sha256(target)
        if copied_hash != before[name] or sha256(source) != before[name]:
            raise RuntimeError(f"source changed during private freeze: {name}")
        members[name] = {"sha256": copied_hash, "bytes": target.stat().st_size}

    after_files, after_deleted = source_inventory()
    if (files, deleted) != (after_files, after_deleted):
        raise RuntimeError("allowlisted source inventory changed during freeze")
    if subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT,
                              text=True).strip() != head:
        raise RuntimeError("main worktree HEAD changed during freeze")
    for name, digest in before.items():
        if sha256(ROOT / name) != digest or sha256(destination / name) != digest:
            raise RuntimeError(f"source changed after private freeze: {name}")

    epoch = int(subprocess.check_output(["git", "show", "-s", "--format=%ct", head],
                                        cwd=ROOT, text=True).strip())
    env = dict(os.environ, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null",
               GIT_AUTHOR_DATE=f"@{epoch}", GIT_COMMITTER_DATE=f"@{epoch}")
    subprocess.run(["git", "init", "--quiet", str(destination)], check=True, env=env)
    subprocess.run(["git", "-C", str(destination), "add", "--all"], check=True, env=env)
    subprocess.run([
        "git", "-C", str(destination), "-c", "user.name=Fabric O11y frozen build",
        "-c", "user.email=build-freeze@localhost", "commit", "--quiet",
        "--date", f"@{epoch}", "-m", "frozen package-build source",
    ], check=True, env=env)
    commit = subprocess.check_output(["git", "-C", str(destination), "rev-parse", "HEAD"],
                                     text=True).strip()
    tree = subprocess.check_output(["git", "-C", str(destination), "rev-parse", "HEAD^{tree}"],
                                   text=True).strip()
    if subprocess.check_output(["git", "-C", str(destination), "status", "--porcelain"],
                               text=True).strip():
        raise RuntimeError("private source repository is not clean")
    bundle = provenance / "frozen-source.bundle"
    subprocess.run(["git", "-C", str(destination), "bundle", "create", str(bundle), "HEAD"],
                   check=True, env=env)
    record: dict[str, object] = {
        "original_head": head,
        "original_branch": branch,
        "allowlisted_worktree_status": status,
        "deleted_allowlisted_paths": deleted,
        "source_commit": commit,
        "source_tree": tree,
        "source_date_epoch": epoch,
        "source_files": members,
        "source_bytes": sizes,
        "source_diff_sha256": hashlib.sha256(diff).hexdigest(),
        "source_diff_file": "source-diff.patch",
        "source_bundle_file": bundle.name,
        "source_bundle_sha256": sha256(bundle),
    }
    (provenance / "source-manifest.json").write_text(
        json.dumps(record, indent=2, sort_keys=True) + "\n"
    )
    record["manifest_sha256"] = sha256(provenance / "source-manifest.json")
    return record


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-id", required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", args.run_id):
        parser.error("--run-id must be lowercase letters, digits, dash or underscore")

    if not os.environ.get("FABRIC_RESOURCE_RUNTIME_SECONDS"):
        raise RuntimeError("invoke through python3 tools/resource_group.py --")
    cgroup = require_limits()
    if not DATA_MOUNT.is_mount() or not STORAGE.is_dir():
        raise RuntimeError(f"mounted data drive unavailable: {DATA_MOUNT}")
    require_data_budget()
    podman = shutil.which("podman")
    overlay = shutil.which("fuse-overlayfs")
    if not podman or not overlay:
        raise RuntimeError("rootless podman and fuse-overlayfs are required")
    cargo_home = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo"))).resolve()
    registry = cargo_home / "registry"
    rustc = TOOLCHAIN / "bin/rustc"
    cargo = TOOLCHAIN / "bin/cargo"
    if not (registry.is_dir() and rustc.is_file() and cargo.is_file()):
        raise RuntimeError(
            f"missing offline Cargo registry or pinned Rust 1.98.0 toolchain; expected {registry} and {TOOLCHAIN}"
        )
    version = command([str(rustc), "--version"])
    if version.returncode or not version.stdout.startswith("rustc 1.98.0 "):
        raise RuntimeError(f"pinned Rust compiler unavailable: {version.stdout}{version.stderr}")

    scratch_root = Path(os.environ["FABRIC_SCRATCH_ROOT"]).resolve(strict=True)
    scratch_root.relative_to((STORAGE / "scratch").resolve(strict=True))
    owned = scratch_root / ("install-package-build-" + args.run_id)
    result_dir = STORAGE / "results" / "installation-package-build" / args.run_id
    if owned.exists() or owned.is_symlink() or result_dir.exists() or result_dir.is_symlink():
        raise RuntimeError(f"refusing to reuse owned path: {owned} or {result_dir}")
    owned.mkdir(mode=0o700)
    (owned / ".fabric-package-build-owned").write_text("installation-build-v1\n")
    result_dir.mkdir(parents=True)
    provenance = result_dir / "provenance"
    provenance.mkdir()
    logs = owned / "logs"
    logs.mkdir()
    store = owned / "podman"
    for name in ("graph", "run", "tmp"):
        (store / name).mkdir(parents=True)
    source = owned / "source"
    target = owned / "target"
    output = owned / "out"
    cargo_home_guest = owned / "cargo-home"
    for path in (target, output, cargo_home_guest):
        path.mkdir()
    registry_copy = cargo_home_guest / "registry"

    podman_args = [podman, "--root", str(store / "graph"), "--runroot", str(store / "run"),
                   "--tmpdir", str(store / "tmp"), "--storage-driver", "overlay",
                   "--storage-opt", f"overlay.mount_program={overlay}"]
    env = dict(os.environ, TMPDIR=str(store / "tmp"), TMP=str(store / "tmp"),
               TEMP=str(store / "tmp"))
    receipt: dict[str, object] = {
        "run_id": args.run_id,
        "classification": "isolated package build; not acceptance or deployment qualification",
        "started_unix": time.time(),
        "data_drive": str(DATA_MOUNT),
        "scratch": str(owned),
        "storage_budget_bytes": RUN_BUDGET_BYTES,
        "host_cgroup": str(cgroup),
        "memory_max": (cgroup / "memory.max").read_text().strip(),
        "memory_swap_max": (cgroup / "memory.swap.max").read_text().strip(),
        "toolchain": str(TOOLCHAIN),
        "rustc": version.stdout.strip(),
        "cargo_registry": str(registry),
        "cargo_registry_copy_bytes": None,
        "cargo_registry_copy_sha256": None,
        "image_tag_resolved": IMAGE_TAG,
        "build_command": ["packaging/build-deb.sh", "/out"],
        "source": None,
        "commands": [],
        "state": "running",
    }
    container_name = "fabric-deb-build-" + args.run_id
    cleanup_ok = False
    cleanup_actions: list[dict[str, object]] = []

    def logged(label: str, cmd: list[str], *, timeout: int,
               run_env: dict[str, str] = env) -> subprocess.CompletedProcess[str]:
        try:
            result = command(cmd, timeout=timeout, env=run_env)
            stdout, stderr, status = result.stdout, result.stderr, result.returncode
        except subprocess.TimeoutExpired as error:
            stdout = error.stdout.decode(errors="replace") if isinstance(error.stdout, bytes) else (error.stdout or "")
            stderr = error.stderr.decode(errors="replace") if isinstance(error.stderr, bytes) else (error.stderr or "")
            (logs / f"{label}.stdout").write_text(stdout)
            (logs / f"{label}.stderr").write_text(stderr)
            receipt["commands"].append({"label": label, "command": cmd, "exit": None,
                                        "timed_out": True})
            raise RuntimeError(f"{label} timed out after {timeout}s") from error
        (logs / f"{label}.stdout").write_text(stdout)
        (logs / f"{label}.stderr").write_text(stderr)
        receipt["commands"].append({"label": label, "command": cmd, "exit": status,
                                    "timed_out": False})
        if status:
            raise RuntimeError(f"{label} failed with exit {status}: {stderr[-2000:]}")
        return result

    try:
        registry_bytes, registry_hash = copy_registry(registry, registry_copy)
        receipt["cargo_registry_copy_bytes"] = registry_bytes
        receipt["cargo_registry_copy_sha256"] = registry_hash
        if file_tree_bytes(owned) > RUN_BUDGET_BYTES:
            raise RuntimeError("Cargo registry copy exhausted the owned 8 GiB build budget")
        source_record = freeze_source(source, provenance)
        receipt["source"] = source_record
        receipt["source_commit"] = source_record["source_commit"]
        info = logged("podman-info", [*podman_args, "info", "--format",
                                      "{{.Host.Security.Rootless}}|{{.Store.GraphRoot}}|{{.Store.RunRoot}}|{{.Store.ImageCopyTmpDir}}"],
                      timeout=30)
        fields = info.stdout.strip().split("|")
        expected = ["true", str((store / "graph").resolve()), str((store / "run").resolve()),
                    str((store / "tmp").resolve())]
        if fields != expected:
            raise RuntimeError(f"Podman is not using the expected rootless data paths: {fields!r}")
        logged("podman-pull", [*podman_args, "pull", "--tls-verify=true",
                               "--policy=always", IMAGE_TAG], timeout=PULL_TIMEOUT_S)
        image = logged("podman-digest", [*podman_args, "image", "inspect", "--format",
                                         "{{range .RepoDigests}}{{println .}}{{end}}", IMAGE_TAG],
                       timeout=30).stdout.splitlines()
        digest = next((line for line in image
                       if re.fullmatch(r"docker\.io/library/debian@sha256:[0-9a-f]{64}", line)), None)
        if digest is None:
            raise RuntimeError(f"could not resolve a pinned official Debian image digest: {image!r}")
        receipt["image_digest"] = digest

        relative = "/" + str(cgroup.relative_to(Path("/sys/fs/cgroup")))
        container_script = r'''set -euo pipefail
expected=${FABRIC_EXPECTED_CGROUP:?}
actual=$(awk -F: '$1 == "0" {print $3}' /proc/self/cgroup)
if [ "$actual" != "$expected" ]; then
  echo "container cgroup $actual differs from caller cgroup $expected" >&2
  exit 70
fi
group="/sys/fs/cgroup$expected"
memory=$(cat "$group/memory.max")
swap=$(cat "$group/memory.swap.max")
if [ "$memory" = max ] || [ "$memory" -gt 21474836480 ] || [ "$swap" != 0 ]; then
  echo "unsafe build cgroup: memory.max=$memory memory.swap.max=$swap" >&2
  exit 71
fi
printf 'cgroup=%s memory.max=%s memory.swap.max=%s\n' "$actual" "$memory" "$swap"
apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
  build-essential binutils ca-certificates dpkg-dev git
export PATH=/opt/rustup/toolchains/1.98.0-x86_64-unknown-linux-gnu/bin:$PATH
export RUSTUP_HOME=/opt/rustup CARGO_HOME=/cargo CARGO_TARGET_DIR=/target
export TMPDIR=/scratch TMP=/scratch TEMP=/scratch
rustc --version | grep -q '^rustc 1\.98\.0 '
command -v git dpkg-deb dpkg-shlibdeps objdump cc >/dev/null
status=$(git -C /src status --porcelain)
[ -z "$status" ] || { echo 'private source tree is dirty' >&2; exit 72; }
SOURCE_DATE_EPOCH=${FABRIC_SOURCE_DATE_EPOCH:?} /src/packaging/build-deb.sh /out
dpkg-query -W -f='${binary:Package}\t${Version}\n' build-essential binutils ca-certificates dpkg-dev git
'''
        container = [*podman_args, "run", "--rm", "--name", container_name,
                     "--pull=never", "--cgroups=disabled", "--cgroupns=host",
                     "--systemd=false", "--network=pasta", "--security-opt=label=disable",
                     "--volume", f"{source}:/src:ro", "--volume",
                     f"{TOOLCHAIN_HOME}:/opt/rustup:ro", "--volume",
                     f"{cargo_home_guest}:/cargo:rw", "--volume", f"{target}:/target:rw",
                     "--volume", f"{output}:/out:rw",
                     "--volume", f"{store / 'tmp'}:/scratch:rw",
                     "--env", f"FABRIC_EXPECTED_CGROUP={relative}",
                     "--env", f"FABRIC_SOURCE_DATE_EPOCH={source_record['source_date_epoch']}",
                     "--env", "CARGO_NET_OFFLINE=true", digest, "bash", "-euc", container_script]
        build_env = dict(env)
        build_env["FABRIC_CONTAINER_NAME"] = container_name
        build_env["TMPDIR"] = str(store / "tmp")
        logged("package-build", container, timeout=BUILD_TIMEOUT_S, run_env=build_env)
        built = list(output.glob("fabrico11y_*.deb"))
        if len(built) != 1 or built[0].is_symlink():
            raise RuntimeError(f"expected exactly one built Debian package, found {built}")
        package_hash = sha256(built[0])
        output_bytes = file_tree_bytes(owned)
        if output_bytes > RUN_BUDGET_BYTES:
            raise RuntimeError(f"owned package-build data exceeded its 8 GiB budget: {output_bytes}")
        final_package = result_dir / built[0].name
        shutil.copy2(built[0], final_package)
        if sha256(final_package) != package_hash:
            raise RuntimeError("package hash changed while publishing build artifact")
        (result_dir / (built[0].name + ".sha256")).write_text(
            f"{package_hash}  {built[0].name}\n"
        )
        require_data_budget()
        receipt.update({"state": "built", "package": str(final_package),
                        "package_sha256": package_hash, "package_bytes": final_package.stat().st_size,
                        "owned_bytes_before_cleanup": output_bytes})
    except (OSError, RuntimeError, subprocess.SubprocessError, ValueError) as error:
        receipt.update({"state": "failed", "error": f"{type(error).__name__}: {error}"})
    finally:
        try:
            if (store / "graph").exists():
                exists = command([*podman_args, "container", "exists", container_name], timeout=20, env=env)
                if exists.returncode == 0:
                    removed = command([*podman_args, "rm", "--force", container_name], timeout=30, env=env)
                    cleanup_ok = removed.returncode == 0
                else:
                    cleanup_ok = exists.returncode == 1
        except (OSError, subprocess.SubprocessError):
            cleanup_ok = False
        receipt["container_cleanup_confirmed"] = cleanup_ok
        receipt["finished_unix"] = time.time()
        receipt["cgroup_resources"] = {
            name: (cgroup / name).read_text().strip()
            for name in ("memory.peak", "memory.events", "cpu.stat")
            if (cgroup / name).exists()
        }
        if receipt["state"] == "built" and cleanup_ok:
            for path in logs.glob("*"):
                shutil.copy2(path, result_dir / path.name)
            try:
                bundle = result_dir / "provenance" / "frozen-source.bundle"
                if (not bundle.is_file() or
                        sha256(bundle) != receipt["source"]["source_bundle_sha256"]):
                    raise RuntimeError("frozen source bundle is missing or has a hash mismatch")
                artifact = Path(str(receipt["package"]))
                if (not artifact.is_file() or
                        sha256(artifact) != receipt["package_sha256"]):
                    raise RuntimeError("built package is missing or has a hash mismatch")
                cleanup_unshare = command(
                    [*podman_args, "image", "rm", "--all", "--force"],
                    timeout=90, env=env,
                )
                cleanup_actions.append({"action": "remove-all-run-images",
                                        "exit": cleanup_unshare.returncode,
                                        "stderr": cleanup_unshare.stderr[-4000:]})
                if cleanup_unshare.returncode != 0:
                    raise RuntimeError(
                        f"Podman image cleanup exit {cleanup_unshare.returncode}: "
                        f"{cleanup_unshare.stderr[-1000:]}"
                    )

                # Only remove build payloads in the user namespace. Keep Podman's
                # graph/runroot alive until every Podman command has exited.
                payloads = [source, target, output, cargo_home_guest, logs]
                cleanup_payloads = command(
                    [*podman_args, "unshare", "rm", "-rf", "--", *map(str, payloads)],
                    timeout=90, env=env,
                )
                cleanup_actions.append({"action": "remove-build-payloads",
                                        "exit": cleanup_payloads.returncode,
                                        "stderr": cleanup_payloads.stderr[-4000:]})
                if cleanup_payloads.returncode != 0 or any(path.exists() for path in payloads):
                    raise RuntimeError(
                        f"Podman payload cleanup exit {cleanup_payloads.returncode}; "
                        f"stderr: {cleanup_payloads.stderr[-1000:]}"
                    )

                if not (owned / ".fabric-package-build-owned").is_file():
                    raise RuntimeError("owned scratch marker is missing")
                owned_mounts = []
                for line in Path("/proc/self/mountinfo").read_text().splitlines():
                    fields = line.split()
                    mountpoint = fields[4].replace("\\040", " ")
                    if mountpoint == str(owned) or mountpoint.startswith(str(owned) + "/"):
                        owned_mounts.append(mountpoint)
                if owned_mounts:
                    raise RuntimeError(f"owned scratch still has mounts: {owned_mounts}")
                unexpected_owners: list[str] = []
                def check_owner_error(error: OSError) -> None:
                    raise error
                for base, directories, files in os.walk(owned, onerror=check_owner_error,
                                                        followlinks=False):
                    for name in directories + files:
                        path = Path(base) / name
                        if path.lstat().st_uid != os.getuid():
                            unexpected_owners.append(str(path))
                            if len(unexpected_owners) == 8:
                                break
                    if unexpected_owners:
                        break
                if unexpected_owners:
                    raise RuntimeError(
                        f"Podman left non-user-owned paths; retaining scratch: {unexpected_owners}"
                    )
                shutil.rmtree(owned)
                cleanup_ok = not owned.exists()
                cleanup_actions.append({"action": "remove-host-owned-scratch",
                                        "exit": 0 if cleanup_ok else 1,
                                        "postcondition_absent": cleanup_ok})
                if not cleanup_ok:
                    raise RuntimeError("owned scratch still exists after host cleanup")
                receipt["cleanup"] = (
                    "Podman images/payloads removed; verified host-owned remainder removed after Podman exited"
                )
            except (OSError, RuntimeError, subprocess.SubprocessError) as error:
                receipt.update({"state": "failed", "cleanup": "owned scratch cleanup failed",
                                "cleanup_error": str(error), "scratch_retained": str(owned)})
                cleanup_ok = False
            receipt["cleanup_actions"] = cleanup_actions
            (result_dir / "receipt.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
        else:
            receipt["cleanup"] = "scratch retained for failure evidence"
            receipt["scratch_retained"] = str(owned)
            (result_dir / "receipt.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
            for path in logs.glob("*"):
                shutil.copy2(path, result_dir / path.name)

    print(f"state={receipt['state']} receipt={result_dir / 'receipt.json'}")
    if receipt.get("package"):
        print(f"package={receipt['package']} sha256={receipt['package_sha256']}")
    if receipt.get("scratch_retained"):
        print(f"scratch_retained={receipt['scratch_retained']}")
    return 0 if receipt["state"] == "built" and cleanup_ok else 2


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError, subprocess.SubprocessError, ValueError) as error:
        print(f"isolated package build: NOT RUN: {error}", file=sys.stderr)
        sys.exit(2)
