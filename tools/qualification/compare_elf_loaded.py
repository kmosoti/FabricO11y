#!/usr/bin/env python3
"""Compare ELF64 loaded content while recording non-runtime file differences.

The only normalized bytes are ELF e_shoff and the descriptor of a validated
GNU build-id note. Full-file SHA-256 values are always reported separately.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import tempfile

MAX_ELF_BYTES = 128 * 1024 * 1024
MAX_PROGRAM_HEADERS = 1024
MAX_SECTIONS = 16384
ELF_HEADER = struct.Struct("<16sHHIQQQIHHHHHH")
PROGRAM_HEADER = struct.Struct("<IIQQQQQQ")
SECTION_HEADER = struct.Struct("<IIQQQQIIQQ")
SHF_ALLOC = 0x2
SHT_NOBITS = 8
SHT_NOTE = 7
SHT_STRTAB = 3
SHT_PROGBITS = 1
PT_LOAD = 1
PT_NOTE = 4
PT_PHDR = 6
NT_GNU_BUILD_ID = 3
BUILD_ID_NAME = b"GNU\0"


class ComparisonError(ValueError):
    pass


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def checked_range(offset, size, total, label):
    if type(offset) is not int or type(size) is not int or offset < 0 or size < 0 or offset + size > total:
        raise ComparisonError(f"{label} lies outside file bounds")
    return offset, offset + size


def cstring(table, offset, label):
    checked_range(offset, 1, len(table), label)
    end = table.find(b"\0", offset)
    if end < 0:
        raise ComparisonError(f"unterminated {label}")
    try:
        return table[offset:end].decode("utf-8")
    except UnicodeDecodeError as error:
        raise ComparisonError(f"invalid UTF-8 in {label}") from error


def parse_elf(path):
    path = Path(path)
    info = path.stat()
    if not path.is_file() or path.is_symlink() or info.st_nlink != 1:
        raise ComparisonError("ELF input must be a regular, unlinked file")
    if not 64 <= info.st_size <= MAX_ELF_BYTES:
        raise ComparisonError(f"ELF size outside bounded range: {info.st_size}")
    data = path.read_bytes()
    if len(data) != info.st_size or path.stat().st_size != info.st_size:
        raise ComparisonError("ELF input changed while reading")
    fields = ELF_HEADER.unpack_from(data)
    ident = fields[0]
    if ident[:4] != b"\x7fELF" or ident[4] != 2 or ident[5] != 1 or ident[6] != 1:
        raise ComparisonError("expected ELF64 little-endian version 1")
    (etype, machine, version, entry, phoff, shoff, flags, ehsize,
     phentsize, phnum, shentsize, shnum, shstrndx) = fields[1:]
    if etype not in (2, 3) or machine != 62 or version != 1:
        raise ComparisonError("expected x86_64 ET_EXEC or ET_DYN ELF")
    if (ehsize != ELF_HEADER.size or phentsize != PROGRAM_HEADER.size
            or shentsize != SECTION_HEADER.size or not phnum or not shnum
            or phnum == 0xFFFF or shnum == 0 or shstrndx >= shnum
            or phnum > MAX_PROGRAM_HEADERS or shnum > MAX_SECTIONS):
        raise ComparisonError("unsupported or malformed ELF table dimensions")
    checked_range(phoff, phnum * phentsize, len(data), "program header table")
    checked_range(shoff, shnum * shentsize, len(data), "section header table")

    programs = []
    for index in range(phnum):
        offset = phoff + index * phentsize
        record = PROGRAM_HEADER.unpack_from(data, offset)
        p_type, p_flags, p_offset, p_vaddr, p_paddr, p_filesz, p_memsz, p_align = record
        checked_range(p_offset, p_filesz, len(data), f"program segment {index}")
        if p_filesz > p_memsz and p_type == PT_LOAD:
            raise ComparisonError(f"PT_LOAD file size exceeds memory size at {index}")
        if p_align not in (0, 1) and (p_align & (p_align - 1)):
            raise ComparisonError(f"non-power-of-two segment alignment at {index}")
        programs.append(record)

    raw_sections = []
    for index in range(shnum):
        offset = shoff + index * shentsize
        section = SECTION_HEADER.unpack_from(data, offset)
        (name_offset, section_type, section_flags, address, file_offset, size,
         link, info, alignment, entry_size) = section
        if section_type != SHT_NOBITS:
            checked_range(file_offset, size, len(data), f"section {index}")
        raw_sections.append(section)
    names_section = raw_sections[shstrndx]
    if names_section[1] != SHT_STRTAB:
        raise ComparisonError("section-name table is not a string table")
    name_table = data[names_section[4]:names_section[4] + names_section[5]]
    sections = []
    for index, section in enumerate(raw_sections):
        (name_offset, section_type, section_flags, address, file_offset, size,
         link, info, alignment, entry_size) = section
        name = cstring(name_table, name_offset, f"section name {index}")
        sections.append({"index": index, "name": name, "type": section_type,
                         "flags": section_flags, "address": address,
                         "offset": file_offset, "size": size, "link": link,
                         "info": info, "alignment": alignment,
                         "entry_size": entry_size})

    notes = [section for section in sections if section["name"] == ".note.gnu.build-id"]
    if len(notes) != 1:
        raise ComparisonError("expected exactly one .note.gnu.build-id section")
    note = notes[0]
    if note["type"] != SHT_NOTE or not note["flags"] & SHF_ALLOC:
        raise ComparisonError(".note.gnu.build-id is not an allocated note section")
    note_start, note_end = checked_range(note["offset"], note["size"], len(data), "GNU build-id note")
    if note["size"] < 12:
        raise ComparisonError("GNU build-id note is truncated")
    name_size, desc_size, note_type = struct.unpack_from("<III", data, note_start)
    name_start = note_start + 12
    desc_start = name_start + ((name_size + 3) & ~3)
    desc_end = desc_start + desc_size
    total_end = desc_start + ((desc_size + 3) & ~3)
    if (name_size != len(BUILD_ID_NAME) or note_type != NT_GNU_BUILD_ID
            or data[name_start:name_start + name_size] != BUILD_ID_NAME
            or desc_size < 1 or desc_size > 64 or total_end != note_end
            or desc_end > note_end):
        raise ComparisonError("section named .note.gnu.build-id is not exactly a GNU build-id note")
    return {"data": data, "header": fields, "programs": programs,
            "sections": sections, "build_id_descriptor": (desc_start, desc_end)}


def masked_equal(left, right, masks):
    if len(left) != len(right):
        return False
    a, b = bytearray(left), bytearray(right)
    for start, end in masks:
        lo, hi = max(start, 0), min(end, len(a))
        if lo < hi:
            a[lo:hi] = b"\0" * (hi - lo)
            b[lo:hi] = b"\0" * (hi - lo)
    return a == b


def compare_parsed(left, right):
    h1, h2 = left["header"], right["header"]
    # e_shoff is field 6 and is the only ELF header field allowed to differ.
    if h1[:6] + h1[7:] != h2[:6] + h2[7:]:
        raise ComparisonError("ELF loaded header fields differ beyond e_shoff")
    if len(left["programs"]) != len(right["programs"]):
        raise ComparisonError("program header count differs")
    if left["programs"] != right["programs"]:
        raise ComparisonError("program header fields differ")

    section_names_left = [s["name"] for s in left["sections"]]
    section_names_right = [s["name"] for s in right["sections"]]
    if section_names_left != section_names_right:
        raise ComparisonError("section name/index layout differs")
    alloc_left = [s for s in left["sections"] if s["flags"] & SHF_ALLOC]
    alloc_right = [s for s in right["sections"] if s["flags"] & SHF_ALLOC]
    if len(alloc_left) != len(alloc_right):
        raise ComparisonError("allocated section count differs")
    for first, second in zip(alloc_left, alloc_right):
        keys = ("name", "type", "flags", "address", "offset", "size", "link",
                "info", "alignment", "entry_size")
        if any(first[key] != second[key] for key in keys):
            raise ComparisonError(f"allocated section metadata differs: {first['name']}")
        if first["type"] == SHT_NOBITS:
            continue
        a0, a1 = first["offset"], first["offset"] + first["size"]
        b0, b1 = second["offset"], second["offset"] + second["size"]
        mask_left = [left["build_id_descriptor"]]
        mask_right = [right["build_id_descriptor"]]
        if first["name"] == ".note.gnu.build-id":
            if not masked_equal(left["data"][a0:a1], right["data"][b0:b1],
                                [(mask_left[0][0] - a0, mask_left[0][1] - a0)]):
                raise ComparisonError("GNU build-id note differs beyond its validated descriptor")
        elif left["data"][a0:a1] != right["data"][b0:b1]:
            raise ComparisonError(f"allocated section bytes differ: {first['name']}")

    masks_left = [(40, 48), left["build_id_descriptor"]]
    masks_right = [(40, 48), right["build_id_descriptor"]]
    for index, (first, second) in enumerate(zip(left["programs"], right["programs"])):
        p_offset, p_filesz = first[2], first[5]
        q_offset, q_filesz = second[2], second[5]
        if p_filesz != q_filesz or p_offset != q_offset:
            raise ComparisonError(f"program segment mapping differs at {index}")
        if p_filesz == 0:
            continue
        a, b = left["data"][p_offset:p_offset + p_filesz], right["data"][q_offset:q_offset + q_filesz]
        local_a = [(start - p_offset, end - p_offset) for start, end in masks_left]
        local_b = [(start - q_offset, end - q_offset) for start, end in masks_right]
        if not masked_equal(a, b, local_a + local_b):
            raise ComparisonError(f"program segment bytes differ at index {index}")
    return {"allocated_sections_compared": len(alloc_left),
            "program_headers_compared": len(left["programs"]),
            "program_segments_compared": sum(p[5] > 0 for p in left["programs"]),
            "normalizations": ["ELF64 e_shoff field after bounded section-table validation",
                              "GNU build-id descriptor after validating exact GNU NT_GNU_BUILD_ID note"]}


def loaded_equivalent(old_path, new_path):
    left, right = parse_elf(old_path), parse_elf(new_path)
    details = compare_parsed(left, right)
    return left, right, details


def negative_control(old_path, new_path, parsed_new):
    scratch = Path(os.environ["FABRIC_SCRATCH_ROOT"]).resolve(strict=True)
    data_root = Path("/run/media/kmosoti/data/FabricO11y").resolve(strict=True)
    scratch.relative_to((data_root / "scratch").resolve(strict=True))
    text = next((s for s in parsed_new["sections"]
                 if s["name"] == ".text" and s["type"] == SHT_PROGBITS
                 and s["flags"] & (SHF_ALLOC | 0x4) == (SHF_ALLOC | 0x4)), None)
    if text is None or text["size"] == 0:
        raise ComparisonError("executable allocated .text section required for negative control")
    with tempfile.TemporaryDirectory(prefix="elf-loaded-control-", dir=scratch) as directory:
        mutated = Path(directory) / "mutated-elf"
        raw = bytearray(parsed_new["data"])
        raw[text["offset"]] ^= 1
        fd = os.open(mutated, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as output:
            output.write(raw)
            output.flush()
            os.fsync(output.fileno())
        try:
            loaded_equivalent(old_path, mutated)
        except ComparisonError as error:
            return {"text_byte_flip_rejected": True, "rejection": str(error),
                    "mutated_offset": text["offset"]}
        raise ComparisonError("negative control passed after mutating .text")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--old", required=True, type=Path)
    parser.add_argument("--new", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    old_path, new_path = args.old.resolve(strict=True), args.new.resolve(strict=True)
    output = args.output.resolve(strict=False)
    data_root = Path("/run/media/kmosoti/data/FabricO11y").resolve(strict=True)
    output.relative_to(data_root)
    if output in (old_path, new_path) or output.exists() or output.is_symlink():
        raise ComparisonError("fresh output separate from binary inputs required")
    if not output.parent.is_dir():
        raise ComparisonError("output parent must already exist on the data drive")
    left, right, comparison = loaded_equivalent(old_path, new_path)
    control = negative_control(old_path, new_path, right)
    report = {"classification": "loaded_elf_equivalence_not_file_identity",
              "old": {"path": str(old_path), "bytes": len(left["data"]),
                      "sha256": sha256(left["data"])},
              "new": {"path": str(new_path), "bytes": len(right["data"]),
                      "sha256": sha256(right["data"])},
              "full_file_sha256_equal": sha256(left["data"]) == sha256(right["data"]),
              "loaded_equivalent": True, "comparison": comparison,
              "negative_control": control}
    payload = json.dumps(report, sort_keys=True, indent=2) + "\n"
    fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as destination:
        destination.write(payload)
        destination.flush()
        os.fsync(destination.fileno())
    print(json.dumps({"output": str(output), "sha256": sha256(payload.encode()),
                      "loaded_equivalent": True, "negative_control_rejected": True}))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, KeyError, struct.error) as error:
        raise SystemExit(f"ELF comparison: NOT EQUIVALENT: {error}")
