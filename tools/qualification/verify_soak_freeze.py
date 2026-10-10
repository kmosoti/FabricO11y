"""Contained pre/post verification of a prepare_soak manifest; read-only inputs."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import require_limits, STORAGE


def main():
    group = require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", args.label):
        raise ValueError("short lowercase verification label required")
    storage = STORAGE.resolve(strict=True)
    if not args.out.is_absolute() or args.out.exists() or args.out.is_symlink():
        raise ValueError("fresh absolute external receipt path required")
    out = args.out.parent.resolve(strict=True) / args.out.name
    out.relative_to(storage)
    if not args.manifest.is_absolute():
        raise ValueError("absolute manifest path required")
    owned = args.manifest.parent.parent.resolve(strict=True)
    if out.is_relative_to(owned):
        raise ValueError("verification receipt must be outside the owned freeze")
    report = {"command": sys.argv, "label": args.label, "manifest": str(args.manifest),
              "utc": datetime.now(timezone.utc).isoformat(), "cgroup": str(group),
              "members": [], "passed": False}
    try:
        owned.relative_to((storage / "scratch").resolve(strict=True))
        manifest_path = owned / "provenance/manifest.json"
        if args.manifest != manifest_path:
            raise ValueError("exact owned provenance/manifest.json path required")
        marker = owned / ".fabric-soak-freeze-owned"
        for path in (manifest_path, marker):
            if path.is_symlink() or not path.is_file() or path.parent.is_symlink() or path.stat().st_nlink != 1:
                raise ValueError("regular unlinked manifest and ownership marker required")
        if marker.read_text() != "fabric-soak-freeze-r2-v1\n":
            raise ValueError("invalid freeze ownership marker")
        manifest_bytes = manifest_path.read_bytes()
        report["manifest_sha256"] = hashlib.sha256(manifest_bytes).hexdigest()
        manifest = json.loads(manifest_bytes)
        if manifest.get("state") != "frozen" or manifest.get("frozen_root") != str(owned / "frozen"):
            raise ValueError("manifest state or frozen root differs from owned root")
        members = manifest.get("members")
        if not isinstance(members, dict) or not members:
            raise ValueError("nonempty frozen member map required")
        members = dict(members)
        diff = owned / "provenance/source-diff.patch"
        members["provenance/source-diff.patch"] = {
            "bytes": diff.stat().st_size, "sha256": manifest["source_diff_sha256"]}
        for name, expected in sorted(members.items()):
            relative = Path(name)
            if not relative.parts or relative.is_absolute() or ".." in relative.parts or relative.as_posix() != name:
                raise ValueError("unsafe or noncanonical manifest member")
            if relative.parts[0] not in ("frozen", "provenance"):
                raise ValueError("member outside declared frozen/provenance roots")
            size = expected.get("bytes")
            digest = expected.get("sha256")
            if type(size) is not int or size < 0 or not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
                raise ValueError("invalid member byte count or SHA-256")
            path = owned / relative
            observation = {"name": name, "expected_bytes": size, "expected_sha256": digest, "matches": False}
            report["members"].append(observation)
            try:
                cursor = owned
                for part in relative.parts:
                    cursor = cursor / part
                    if cursor.is_symlink():
                        raise ValueError("symlink in frozen member path")
                if not path.is_file() or path.stat().st_nlink != 1:
                    raise ValueError("regular unlinked frozen member required")
                with path.open("rb") as source:
                    actual = hashlib.file_digest(source, "sha256").hexdigest()
                observation.update(actual_bytes=path.stat().st_size, actual_sha256=actual)
                observation["matches"] = observation["actual_bytes"] == size and actual == digest
            except (OSError, ValueError) as error:
                observation["error"] = str(error)
        report["passed"] = all(row["matches"] for row in report["members"])
    except (OSError, ValueError, TypeError, KeyError, AttributeError) as error:
        report["error"] = str(error)
    with out.open("x") as destination:
        destination.write(json.dumps(report, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"label": args.label, "passed": report["passed"], "receipt": str(out)}))
    if not report["passed"]:
        raise RuntimeError("frozen soak validation failed; inspect receipt")


if __name__ == "__main__":
    main()
