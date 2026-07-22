#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["jsonschema==4.26.0"]
# ///
"""Validate every public schema and declared positive/negative fixture."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator, FormatChecker


ROOT = Path(__file__).resolve().parents[1]
MANIFESTS = (
    ROOT / "fixtures/golden/contracts/manifest.json",
    ROOT / "fixtures/segment-format/manifest.json",
)
LOCAL_LINK = re.compile(r"\[[^]]+\]\((?!https?://|#)([^)#]+)(?:#[^)]+)?\)")


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"{path.relative_to(ROOT)}: {exc}") from exc


def pointer(parts: Any) -> str:
    encoded = [str(part).replace("~", "~0").replace("/", "~1") for part in parts]
    return "/" + "/".join(encoded) if encoded else "/"


def validate_schema_files(errors: list[str]) -> dict[str, dict[str, Any]]:
    schemas: dict[str, dict[str, Any]] = {}
    for path in sorted((ROOT / "schemas").glob("fabric.*.v1.json")):
        relative = path.relative_to(ROOT).as_posix()
        try:
            schema = load_json(path)
            Draft202012Validator.check_schema(schema)
        except Exception as exc:  # jsonschema raises several typed schema errors
            errors.append(f"{relative}: invalid schema: {exc}")
            continue
        expected_id = f"https://fabrico11y.dev/schemas/{path.name}"
        if schema.get("$schema") != "https://json-schema.org/draft/2020-12/schema":
            errors.append(f"{relative}: must declare JSON Schema Draft 2020-12")
        if schema.get("$id") != expected_id:
            errors.append(f"{relative}: expected stable $id {expected_id!r}")
        schemas[relative] = schema
    if not schemas:
        errors.append("schemas: no fabric.*.v1.json contracts found")
    return schemas


def validate_manifest(
    manifest_path: Path, schemas: dict[str, dict[str, Any]], errors: list[str]
) -> tuple[int, int, int]:
    if not manifest_path.exists():
        return (0, 0, 0)
    manifest = load_json(manifest_path)
    positive_count = 0
    negative_count = 0
    binary_count = 0
    for case in manifest.get("valid", []):
        schema_name = case["schema"]
        fixture_name = case["fixture"]
        schema = schemas.get(schema_name)
        if schema is None:
            errors.append(f"{manifest_path.relative_to(ROOT)}: unknown schema {schema_name}")
            continue
        instance = load_json(ROOT / fixture_name)
        case_errors = sorted(
            Draft202012Validator(schema, format_checker=FormatChecker()).iter_errors(instance),
            key=lambda item: (list(item.absolute_path), item.message),
        )
        if case_errors:
            first = case_errors[0]
            errors.append(
                f"{fixture_name}: expected valid; {pointer(first.absolute_path)}: {first.message}"
            )
        positive_count += 1

    for case in manifest.get("invalid", []):
        schema_name = case["schema"]
        fixture_name = case["fixture"]
        schema = schemas.get(schema_name)
        if schema is None:
            errors.append(f"{manifest_path.relative_to(ROOT)}: unknown schema {schema_name}")
            continue
        instance = load_json(ROOT / fixture_name)
        case_errors = list(
            Draft202012Validator(schema, format_checker=FormatChecker()).iter_errors(instance)
        )
        expected_validator = case["expected_validator"]
        expected_path = case["expected_path"]
        matched = any(
            error.validator == expected_validator
            and pointer(error.absolute_path) == expected_path
            for error in case_errors
        )
        if not matched:
            actual = ", ".join(
                sorted({f"{error.validator}@{pointer(error.absolute_path)}" for error in case_errors})
            ) or "no validation error"
            errors.append(
                f"{fixture_name}: expected {expected_validator}@{expected_path}; got {actual}"
            )
        negative_count += 1
    for case in manifest.get("binary_cases", []):
        fixture_name = case["fixture"]
        path = ROOT / fixture_name
        try:
            encoded = path.read_text(encoding="ascii").strip()
            payload = bytes.fromhex(encoded)
        except (OSError, UnicodeError, ValueError) as exc:
            errors.append(f"{fixture_name}: invalid sealed hexadecimal fixture: {exc}")
            continue
        if not payload.startswith(b"FABSEG01"):
            errors.append(f"{fixture_name}: byte fixture does not start with FABSEG01")
        if case.get("expected") == "valid" and (
            len(payload) < 96 or payload[-72:-64] != b"FABEND01"
        ):
            errors.append(f"{fixture_name}: valid fixture has no complete format-v1 trailer")
        binary_count += 1
    return (positive_count, negative_count, binary_count)


def validate_local_links(errors: list[str]) -> int:
    checked = 0
    paths = list(ROOT.glob("*.md"))
    for directory in ("benchmarks", "crates", "docs", "fixtures", "python", "schemas"):
        paths.extend((ROOT / directory).rglob("*.md"))
    for path in sorted(set(paths)):
        text = path.read_text(encoding="utf-8")
        for match in LOCAL_LINK.finditer(text):
            target = (path.parent / match.group(1)).resolve()
            checked += 1
            if not target.exists():
                errors.append(
                    f"{path.relative_to(ROOT)}: broken local link {match.group(1)!r}"
                )
    return checked


def main() -> int:
    errors: list[str] = []
    schemas = validate_schema_files(errors)
    positive = 0
    negative = 0
    binary = 0
    try:
        for manifest in MANIFESTS:
            valid_count, invalid_count, binary_count = validate_manifest(
                manifest, schemas, errors
            )
            positive += valid_count
            negative += invalid_count
            binary += binary_count
        links = validate_local_links(errors)
    except (KeyError, TypeError, ValueError) as exc:
        errors.append(str(exc))
        links = 0

    report = {
        "schemas": len(schemas),
        "valid_fixtures": positive,
        "invalid_fixtures": negative,
        "binary_fixtures": binary,
        "local_links": links,
        "errors": errors,
    }
    print(json.dumps(report, indent=2, sort_keys=True))
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
