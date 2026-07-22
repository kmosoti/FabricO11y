"""Typed conversion-only Python facade over the embedded Rust engine."""

from __future__ import annotations

import json
from dataclasses import dataclass
from os import PathLike
from typing import Any, Mapping, TypeAlias

from ._fabrico11y import (
    AdmissionError,
    Engine as _NativeEngine,
    FabricError,
    IntegrityError,
    SemanticError,
    StorageError,
    __version__,
)

JSONScalar: TypeAlias = None | bool | int | float | str
JSONValue: TypeAlias = JSONScalar | list["JSONValue"] | dict[str, "JSONValue"]


@dataclass(frozen=True, slots=True)
class AdmissionReceipt:
    record_id: str
    recorded_at: str
    pending_count: int


@dataclass(frozen=True, slots=True)
class SealReceipt:
    segment_id: str
    row_count: int
    content_sha256: str


@dataclass(frozen=True, slots=True)
class RecoveryReport:
    removed_partial_files: int
    registered_orphan_segments: int
    cleared_redundant_staging: bool


@dataclass(frozen=True, slots=True)
class ValidationReport:
    segment_count: int
    archived_record_count: int
    pending_record_count: int
    observation_count: int
    correction_count: int
    frontier_count: int


@dataclass(frozen=True, slots=True)
class RecordLocation:
    record_id: str
    record_kind: str
    segment_id: str
    row_index: int
    producer_id: str
    stream_id: str
    recorded_at: str
    classification: str


class Engine:
    """Single-owner embedded engine; externally serialize processes sharing one root."""

    __slots__ = ("_native",)

    def __init__(self, root: str | PathLike[str]) -> None:
        self._native = _NativeEngine(str(root))

    @property
    def pending_count(self) -> int:
        return self._native.pending_count()

    @property
    def recovery(self) -> RecoveryReport:
        return RecoveryReport(**_decode_object(self._native.recovery_json()))

    def admit(self, record: Mapping[str, JSONValue]) -> AdmissionReceipt:
        encoded = json.dumps(
            record,
            ensure_ascii=False,
            allow_nan=False,
            sort_keys=True,
            separators=(",", ":"),
        )
        return AdmissionReceipt(**_decode_object(self._native.admit_json(encoded)))

    def seal(self) -> SealReceipt:
        return SealReceipt(**_decode_object(self._native.seal_json()))

    def replay(self) -> dict[str, Any]:
        return _decode_object(self._native.replay_json())

    def validate(self) -> ValidationReport:
        return ValidationReport(**_decode_object(self._native.validate_json()))

    def locate(self, record_id: str) -> RecordLocation | None:
        decoded = json.loads(self._native.locate_json(record_id))
        return None if decoded is None else RecordLocation(**decoded)


def _decode_object(encoded: str) -> dict[str, Any]:
    decoded = json.loads(encoded)
    if not isinstance(decoded, dict):
        raise TypeError("native boundary returned a non-object JSON value")
    return decoded


__all__ = [
    "AdmissionError",
    "AdmissionReceipt",
    "Engine",
    "FabricError",
    "IntegrityError",
    "RecordLocation",
    "RecoveryReport",
    "SealReceipt",
    "SemanticError",
    "StorageError",
    "ValidationReport",
    "__version__",
]
