"""Pure logical model for the registered E2R group-seal framing."""

from __future__ import annotations

from hashlib import sha256
import struct

_SECTORS = (512, 4096)
_DOMAIN = b"Fabric-E2R-seal-v1\0"


def _check_sector_size(sector_size: int) -> None:
    if type(sector_size) is not int or sector_size not in _SECTORS:
        raise ValueError("unsupported sector size")


def _check_body_tuple(bodies: tuple[bytes, ...]) -> None:
    if not isinstance(bodies, tuple) or not 1 <= len(bodies) <= 6:
        raise ValueError("a group must contain one to six bodies")
    for body in bodies:
        if not isinstance(body, bytes) or not 57 <= len(body) <= 4096:
            raise ValueError("body length outside registered range")


def _encode_group(group_id: int, bodies: tuple[bytes, ...], sector_size: int) -> bytes:
    if type(group_id) is not int or not 0 <= group_id <= 0xFFFFFFFFFFFFFFFF:
        raise ValueError("group id must fit u64")
    _check_body_tuple(bodies)

    lengths = tuple(len(body) for body in bodies)
    descriptor = (
        b"FGD1"
        + struct.pack("<QI", group_id, len(bodies))
        + struct.pack("<6I", *(lengths + (0,) * (6 - len(lengths))))
    )
    descriptor += bytes(sector_size - len(descriptor))

    frames = bytearray()
    for index, body in enumerate(bodies):
        raw = b"FGF1" + struct.pack("<II", index, len(body)) + body
        frames.extend(raw)
        frames.extend(bytes((-len(raw)) % sector_size))

    seal_head = b"FGS1" + struct.pack("<QI", group_id, len(bodies))
    digest = sha256(_DOMAIN + seal_head + descriptor + bytes(frames)).digest()
    seal = seal_head + digest
    seal += bytes(sector_size - len(seal))
    return descriptor + bytes(frames) + seal


def encode_groups(groups: tuple[tuple[bytes, ...], ...], sector_size: int) -> bytes:
    """Encode sequential groups using the frozen sector-aligned representation."""
    _check_sector_size(sector_size)
    if not isinstance(groups, tuple):
        raise ValueError("groups must be a tuple")
    return b"".join(_encode_group(group_id, group, sector_size)
                     for group_id, group in enumerate(groups))


def _all_zero(data: bytes) -> bool:
    return not any(data)


def recover(image: bytes, sector_size: int) -> tuple:
    """Return the fully validated committed prefix, or the untouched image on error."""
    _check_sector_size(sector_size)
    if not isinstance(image, bytes) or len(image) % sector_size:
        return ("error", image)

    groups: list[tuple[bytes, ...]] = []
    offset = 0
    group_id = 0
    image_len = len(image)

    while offset < image_len:
        # A missing descriptor gives no trustworthy extent. Accept only an
        # entirely zero tail; any later data could include a complete seal.
        if image_len - offset < sector_size:
            return ("error", image)
        descriptor = image[offset:offset + sector_size]
        if _all_zero(descriptor):
            if _all_zero(image[offset:]):
                return ("ok", tuple(groups), offset)
            return ("error", image)
        if descriptor[:4] != b"FGD1":
            return ("error", image)

        parsed_group_id = struct.unpack_from("<Q", descriptor, 4)[0]
        frame_count = struct.unpack_from("<I", descriptor, 12)[0]
        lengths = struct.unpack_from("<6I", descriptor, 16)
        if parsed_group_id != group_id or not 1 <= frame_count <= 6:
            return ("error", image)
        if any(not 57 <= n <= 4096 for n in lengths[:frame_count]):
            return ("error", image)
        if any(lengths[frame_count:]):
            return ("error", image)
        if not _all_zero(descriptor[40:]):
            return ("error", image)

        frame_offset = offset + sector_size
        bodies: list[bytes] = []
        frame_sectors: list[bytes] = []
        for index, body_len in enumerate(lengths[:frame_count]):
            raw_len = 12 + body_len
            span = ((raw_len + sector_size - 1) // sector_size) * sector_size
            end = frame_offset + span
            if end > image_len:
                return ("error", image)
            frame_region = image[frame_offset:end]
            if frame_region[:4] != b"FGF1":
                return ("error", image)
            stored_index, stored_length = struct.unpack_from("<II", frame_region, 4)
            if stored_index != index or stored_length != body_len:
                return ("error", image)
            if not _all_zero(frame_region[raw_len:]):
                return ("error", image)
            bodies.append(frame_region[12:raw_len])
            frame_sectors.append(frame_region)
            frame_offset = end

        seal_end = frame_offset + sector_size
        if seal_end > image_len:
            return ("error", image)
        seal = image[frame_offset:seal_end]
        # A zero seal is the only absent seal representation. The candidate
        # group is discarded, while nonzero trailing bytes fail closed.
        if _all_zero(seal):
            if _all_zero(image[frame_offset:]):
                return ("ok", tuple(groups), offset)
            return ("error", image)

        if seal[:4] != b"FGS1":
            return ("error", image)
        seal_group_id = struct.unpack_from("<Q", seal, 4)[0]
        seal_count = struct.unpack_from("<I", seal, 12)[0]
        if seal_group_id != group_id or seal_count != frame_count:
            return ("error", image)
        if not _all_zero(seal[48:]):
            return ("error", image)
        seal_head = seal[:16]
        covered = descriptor + b"".join(frame_sectors)
        expected_digest = sha256(_DOMAIN + seal_head + covered).digest()
        if seal[16:48] != expected_digest:
            return ("error", image)

        groups.append(tuple(bodies))
        group_id += 1
        offset = seal_end

    return ("ok", tuple(groups), offset)


def supervise(external_io_error: bool, recovery_result: tuple) -> str:
    if type(external_io_error) is not bool:
        raise ValueError("external_io_error must be bool")
    if (not isinstance(recovery_result, tuple) or not recovery_result
            or recovery_result[0] != "ok") or external_io_error:
        return "rebuild_from_trusted_source"
    return "resume_allowed"


def append_logical(previous: bytes, group_id: int, bodies: tuple[bytes, ...],
                   sector_size: int, fail_at: str | None) -> tuple:
    """Model writes plus one sync; a failed operation poisons without ACK."""
    _check_sector_size(sector_size)
    if not isinstance(previous, bytes) or len(previous) % sector_size:
        raise ValueError("previous image must be sector aligned bytes")
    committed = recover(previous, sector_size)
    if (committed[0] != "ok" or committed[2] != len(previous)
            or group_id != len(committed[1])):
        raise ValueError("append requires a complete prefix and its next group id")
    group = _encode_group(group_id, bodies, sector_size)
    operations = tuple(f"write:{i}" for i in range(len(group) // sector_size)) + ("sync",)
    if fail_at is not None and (not isinstance(fail_at, str) or fail_at not in operations):
        raise ValueError("unknown boundary")

    image = bytearray(previous + bytes(len(group)))
    start = len(previous)
    for index, operation in enumerate(operations):
        if operation.startswith("write:"):
            sector_index = int(operation[6:])
            if operation == fail_at:
                left = start + sector_index * sector_size
                image[left:left + sector_size] = group[sector_index * sector_size:
                                                        (sector_index + 1) * sector_size]
                return (bytes(image), False, True, True, operations)
            left = start + sector_index * sector_size
            image[left:left + sector_size] = group[sector_index * sector_size:
                                                    (sector_index + 1) * sector_size]
        elif operation == fail_at:
            return (bytes(image), False, True, True, operations)

    return (bytes(image), True, False, False, operations)
