"""Byte-preserving guide inputs for the fresh-owner walkthrough.

This module extracts commands; it neither executes them nor chooses conditions,
roles or linked callpoints. The walkthrough must record those decisions and its
actual actions separately. Extracted commands are not acceptance evidence.
"""
from dataclasses import dataclass
import hashlib
from pathlib import Path
import re


@dataclass(frozen=True)
class Block:
    document: str
    heading: str
    opening_line: int
    closing_line: int
    command: bytes

    def record(self):
        return {
            "document": self.document, "heading": self.heading,
            "opening_line": self.opening_line, "closing_line": self.closing_line,
            "command_sha256": hashlib.sha256(self.command).hexdigest(),
            "command": self.command.decode("utf-8"),
        }


class Guide:
    """One exact archived document, with one-based inclusive fence spans."""
    def __init__(self, document, raw, expected_sha256):
        if document not in ("docs/operations.md", "docs/access-operations.md"):
            raise ValueError("walkthrough accepts only the two registered guides")
        if not isinstance(raw, bytes) or len(raw) > 1024 * 1024:
            raise ValueError("bounded guide bytes required")
        if hashlib.sha256(raw).hexdigest() != expected_sha256:
            raise ValueError("guide digest differs from admitted merged guide")
        raw.decode("utf-8")
        self.document, self.raw, self.sha256 = document, raw, expected_sha256
        self.blocks, self.headings = [], []
        lines = raw.splitlines(keepends=True)
        opening = None
        heading = ""
        for number, line in enumerate(lines, 1):
            text = line.rstrip(b"\r\n")
            if opening is not None:
                # A shell heredoc is content, not a Markdown heading or link.
                if text == b"```":
                    start, language = opening
                    if language == b"sh":
                        self.blocks.append(Block(document, heading, start, number,
                                                 b"".join(lines[start:number - 1])))
                    opening = None
                continue
            if text.startswith(b"```"):
                opening = (number, text[3:])
                continue
            match = re.fullmatch(rb"(#{1,6}) (.+)", text)
            if match:
                title = match[2].decode("utf-8")
                heading = re.sub(r"[^\w\- ]", "", title.lower()).replace(" ", "-")
                if any(item["anchor"] == heading for item in self.headings):
                    raise ValueError("ambiguous duplicate guide heading")
                self.headings.append({"anchor": heading, "level": len(match[1]), "line": number})
        if opening is not None:
            raise ValueError("unterminated guide fence")
        self.line_count = len(lines)

    def section(self, anchor):
        """Return fences through the next same-or-higher heading, in source order."""
        selected = next((i for i, value in enumerate(self.headings)
                         if value["anchor"] == anchor), None)
        if selected is None:
            raise ValueError("linked guide section does not exist")
        start = self.headings[selected]
        end = next((value["line"] for value in self.headings[selected + 1:]
                    if value["level"] <= start["level"]), self.line_count + 1)
        return tuple(block for block in self.blocks if start["line"] < block.opening_line < end)

    def archive(self, directory):
        """Archive exact input once; refuse overwriting any earlier evidence."""
        destination = Path(directory) / Path(self.document).name
        with destination.open("xb") as stream:
            stream.write(self.raw)
        return {"document": self.document, "sha256": self.sha256,
                "archive": str(destination), "blocks": [block.record() for block in self.blocks]}
