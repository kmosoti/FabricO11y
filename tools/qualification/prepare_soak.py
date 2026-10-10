"""Freeze a fresh R2 mini-tree on owned data-drive scratch; run via launcher."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from resource_group import require_limits, STORAGE

MAX_COPY_BYTES = 1024 ** 3
BINARY_NAMES = ("fabric-server", "fabric-node", "examples/spindle_sim",
                "examples/server_dump", "examples/spool_dump")
PROTOCOL_NAMES = ("docs/experiments/benchmarks/soak-protocol.md",
                  "docs/experiments/benchmarks/soak-protocol-r2.md",
                  "docs/experiments/formal/readiness-continuation-protocol.md")


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def git(*arguments):
    return subprocess.check_output(["git", *arguments], cwd=ROOT)


def source_paths(*, include_missing=False):
    # Git-listed compilation and package inputs, including dirty new modules,
    # patched vendor crates and console assets. No run archives, arbitrary
    # operator configuration, credentials or ignored files. A snapshot remains
    # provenance, not a claim that supplied binaries were built from its bytes.
    names = git("ls-files", "--cached", "--others", "--exclude-standard", "-z")
    selected = []
    for name in names.decode().split("\0"):
        if not name:
            continue
        path = Path(name)
        if path.is_absolute() or ".." in path.parts:
            raise ValueError("unsafe source path")
        root_input = name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml",
                              "build.rs", ".cargo/config.toml", "LICENSE", "NOTICE")
        rust_input = path.parts[0] in ("src", "crates", "examples", "tests", "xtask") and (
            path.suffix in (".rs", ".proto") or path.name in ("Cargo.toml", "Cargo.lock"))
        proto_input = path.parts[0] == "proto" and path.suffix == ".proto"
        vendor_input = path.parts[0] == "vendor"
        console_input = path.parts[:2] == ("crates", "fabric-ui")
        package_input = path.parts[0] == "packaging"
        helper_input = name in ("tools/resource_group.py", "tools/ui/build.py") or (
            path.parts[:2] == ("tools", "packaging") and path.suffix == ".py")
        if (root_input or rust_input or proto_input or vendor_input or console_input
                or package_input or helper_input) and (include_missing or (ROOT / path).exists()):
            selected.append(name)
    return sorted(set(selected))


def main():
    group = require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--id", required=True)
    parser.add_argument("--bin-dir", type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", args.id):
        raise ValueError("id must be a short lowercase identifier")
    if args.bin_dir is None:
        target = os.environ.get("CARGO_TARGET_DIR")
        if not target:
            raise ValueError("CARGO_TARGET_DIR or --bin-dir required")
        bins = Path(target) / "release"
    else:
        bins = args.bin_dir
    if not bins.is_absolute():
        raise ValueError("binary directory must be absolute")
    bins = bins.resolve(strict=True)
    scratch = Path(os.environ["FABRIC_SCRATCH_ROOT"]).resolve(strict=True)
    scratch.relative_to((STORAGE / "scratch").resolve(strict=True))
    owned = scratch / ("soak-r2-" + args.id)
    if owned.exists() or owned.is_symlink():
        raise ValueError("freeze destination already exists")
    head = git("rev-parse", "HEAD").decode().strip()
    branch = git("branch", "--show-current").decode().strip()
    source_scope = source_paths(include_missing=True)
    sources = source_paths()
    qualification = sorted((ROOT / "tools/qualification").glob("*.py"))
    copies = [(bins / name, Path("frozen/bin") / name) for name in BINARY_NAMES]
    copies += [(path, Path("frozen/tools/qualification") / path.name) for path in qualification]
    copies += [(ROOT / "tools/bench/labs/completion" / name,
                Path("frozen/tools/qualification") / name)
               for name in ("cgroups.py", "enter_group.py")]
    copies += [(ROOT / name, Path("provenance/source") / name) for name in sources]
    copies += [(ROOT / name, Path("provenance/protocols") / name) for name in PROTOCOL_NAMES]
    total = 0
    for source, _ in copies:
        if source.is_symlink() or not source.is_file():
            raise ValueError(f"freeze requires a regular source file: {source}")
        total += source.stat().st_size
    if total > MAX_COPY_BYTES:
        raise ValueError("freeze exceeds 1 GiB known-file copy budget")
    for name in BINARY_NAMES:
        if not os.access(bins / name, os.X_OK):
            raise ValueError(f"binary is not executable: {name}")
    owned.mkdir()
    (owned / ".fabric-soak-freeze-owned").write_text("fabric-soak-freeze-r2-v1\n")
    try:
        members = {}
        for source, destination in copies:
            destination = owned / destination
            destination.parent.mkdir(parents=True, exist_ok=True)
            before = digest(source)
            shutil.copy2(source, destination)
            after = digest(source)
            copied = digest(destination)
            if before != after or copied != before:
                raise ValueError(f"source changed during freeze: {source}")
            members[str(destination.relative_to(owned))] = {"sha256": copied,
                                                           "bytes": destination.stat().st_size}
        provenance = owned / "provenance"
        # Scope the binary diff to the allowlisted inputs, never arbitrary dirty
        # configs. Untracked inputs are preserved in source/ and its hash map.
        (provenance / "source-diff.patch").write_bytes(git("diff", head, "--binary", "--", *source_scope))
        status = git("status", "--porcelain=v1", "--", *source_scope).decode()
        # A second complete source census/hash check rejects concurrent edits,
        # additions or deletions while this independent snapshot was copied.
        if source_paths() != sources or source_paths(include_missing=True) != source_scope:
            raise ValueError("source set changed during freeze")
        for name in sources:
            if digest(ROOT / name) != members["provenance/source/" + name]["sha256"]:
                raise ValueError(f"source changed after copy: {name}")
        for source, destination in copies:
            if digest(source) != members[str(destination)]["sha256"]:
                raise ValueError(f"frozen input changed after copy: {source}")
        if git("rev-parse", "HEAD").decode().strip() != head:
            raise ValueError("Git revision changed during freeze")
        frozen = owned / "frozen"
        (frozen / "target").mkdir()
        manifest = {"state": "frozen", "frozen_root": str(frozen), "source_root": str(ROOT),
                    "git_head": head, "git_branch": branch,
                    "source_status": status, "source_names": sources,
                    "deleted_source_names": sorted(set(source_scope) - set(sources)),
                    "members": members, "total_copy_bytes": total,
                    "copy_budget_bytes": MAX_COPY_BYTES, "cgroup": str(group),
                    "build_provenance_limit": "binary hashes freeze artifacts; no inferred build flags or source-to-binary attestation",
                    "source_diff_sha256": digest(provenance / "source-diff.patch")}
        (provenance / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        print(json.dumps({"frozen_root": str(frozen), "manifest": str(provenance / "manifest.json")}, sort_keys=True))
    except BaseException as error:
        (owned / "freeze-failure.json").write_text(json.dumps({"state":"failed", "error":str(error)}) + "\n")
        raise


if __name__ == "__main__":
    main()
