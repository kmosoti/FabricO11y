"""Independent E2R fixture encoder and logical failure model. No candidate imports."""

from __future__ import annotations

from dataclasses import dataclass
from hashlib import sha256
from itertools import combinations
import random
import struct

LENGTHS = (57, 127, 509, 513, 1023, 4096)
SECTORS = (512, 4096)
SEEDS = (11, 12, 13)


def bodies(size: int, seed: int) -> tuple[bytes, ...]:
    """Pinned, deterministic Log-shaped opaque bodies, including both end lengths."""
    rng = random.Random(seed)
    return tuple(bytes(rng.randrange(256) for _ in range(n)) for n in LENGTHS[:size])


def frame(body: bytes, index: int, sector: int) -> bytes:
    raw = b"FGF1" + struct.pack("<II", index, len(body)) + body
    return raw + bytes((-len(raw)) % sector)


def group_bytes(group_id: int, group_bodies: tuple[bytes, ...], sector: int) -> bytes:
    if sector not in SECTORS or not 1 <= len(group_bodies) <= 6:
        raise ValueError("unsupported model parameters")
    if any(not 57 <= len(body) <= 4096 for body in group_bodies):
        raise ValueError("body length outside registered range")
    lengths = tuple(map(len, group_bodies))
    descriptor = (b"FGD1" + struct.pack("<QI", group_id, len(lengths))
                  + struct.pack("<6I", *(lengths + (0,) * (6 - len(lengths)))))
    descriptor += bytes(sector - len(descriptor))
    frames = b"".join(frame(body, i, sector) for i, body in enumerate(group_bodies))
    seal_head = b"FGS1" + struct.pack("<QI", group_id, len(lengths))
    digest = sha256(b"Fabric-E2R-seal-v1\0" + seal_head + descriptor + frames).digest()
    seal = seal_head + digest + bytes(sector - len(seal_head) - len(digest))
    return descriptor + frames + seal


def fixture(groups: tuple[tuple[bytes, ...], ...], sector: int) -> bytes:
    return b"".join(group_bytes(i, group, sector) for i, group in enumerate(groups))


def sector_subsets(group: bytes, sector: int):
    """All subsets at <=16 sectors; 100,000 sampled masks otherwise, seed 5."""
    count = len(group) // sector
    if count <= 16:
        yield from range(1 << count)
        return
    rng = random.Random(5)
    # Sample with replacement as the registration says 100,000 subsets, not
    # 100,000 distinct subsets. Include adversarial controls explicitly.
    yield 0
    yield (1 << count) - 1
    yield 1 << (count - 1)  # seal only
    for _ in range(100_000 - 3):
        yield rng.getrandbits(count)


def persist_subset(group: bytes, sector: int, mask: int) -> bytes:
    return b"".join(group[i:i + sector] if mask & (1 << (i // sector)) else bytes(sector)
                    for i in range(0, len(group), sector))


def covered_byte_flips(group: bytes, sector: int, seed: int):
    """1,000 deterministic flips/seed, spread over descriptor, payload and seal."""
    rng = random.Random(seed)
    count = len(group) // sector
    regions = ((0, sector), (sector, (count - 1) * sector),
               ((count - 1) * sector, count * sector))
    for i in range(1000):
        lo, hi = regions[i % 3]
        offset = rng.randrange(lo, hi)
        changed = bytearray(group)
        changed[offset] ^= 1 << rng.randrange(8)
        yield bytes(changed), offset


@dataclass(frozen=True)
class LogicalHistory:
    image: bytes
    acked: bool
    poisoned: bool
    external_io_error: bool
    operations: tuple[str, ...]


def logical_append(previous: bytes, group: bytes, sector: int,
                   fail_at: str | None = None) -> LogicalHistory:
    """Sector-atomic logical write sequence; an EIO may leave its write visible."""
    operations = tuple(f"write:{i}" for i in range(len(group) // sector)) + ("sync",)
    if fail_at is not None and fail_at not in operations:
        raise ValueError("unknown boundary")
    image = previous + bytes(len(group))
    for i, op in enumerate(operations):
        if op == fail_at:
            # A failed write may have persisted that sector; a failed sync has
            # every byte visible yet gives no durability/ACK evidence.
            if op.startswith("write:"):
                slot = int(op.split(":")[1])
                image = image[:len(previous) + slot * sector] + group[slot * sector:(slot + 1) * sector] + image[len(previous) + (slot + 1) * sector:]
            return LogicalHistory(image, False, True, True, operations)
        if op.startswith("write:"):
            slot = int(op.split(":")[1])
            image = image[:len(previous) + slot * sector] + group[slot * sector:(slot + 1) * sector] + image[len(previous) + (slot + 1) * sector:]
    return LogicalHistory(image, True, False, False, operations)


def supervisor_decision(external_io_error: bool, scan_is_ok: bool) -> str:
    if external_io_error or not scan_is_ok:
        return "rebuild_from_trusted_source"
    return "resume_allowed"
