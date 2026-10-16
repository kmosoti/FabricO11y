"""Literal extraction controls, not native owner-walkthrough acceptance."""
import hashlib
import os
from pathlib import Path
import tempfile
import unittest

from owner_guide import Guide


def guide(raw):
    return Guide("docs/operations.md", raw, hashlib.sha256(raw).hexdigest())


class GuideTests(unittest.TestCase):
    def test_exact_bytes_source_spans_and_section_extent(self):
        raw = (b"# Guide\n## Install\n`never execute inline`\n```sh\n"
               b"echo '$HOST'\n```\n### Nested\n```sh\ncat <<'EOF'\n"
               b"# Not a heading\nEOF\n```\n## Next\n```sh\nexit 7\n```\n")
        value = guide(raw)
        selected = value.section("install")
        self.assertEqual([(b.opening_line, b.closing_line) for b in selected], [(4, 6), (8, 12)])
        self.assertEqual([b.command for b in selected], [b"echo '$HOST'\n", b"cat <<'EOF'\n# Not a heading\nEOF\n"])
        self.assertEqual(len(value.blocks), 3)
        self.assertEqual(selected[1].heading, "nested")

    def test_other_languages_and_inline_commands_are_not_shell_blocks(self):
        self.assertEqual(guide(b"# Guide\n`echo inline`\n```python\nprint(1)\n```\n").blocks, [])

    def test_crlf_command_bytes_are_preserved(self):
        self.assertEqual(guide(b"# Guide\r\n```sh\r\necho ok\r\n```\r\n").blocks[0].command,
                         b"echo ok\r\n")

    def test_changed_guide_refused(self):
        original = b"# Guide\n```sh\necho ok\n```\n"
        with self.assertRaisesRegex(ValueError, "digest differs"):
            Guide("docs/operations.md", original.replace(b"ok", b"changed"),
                  hashlib.sha256(original).hexdigest())

    def test_unterminated_fence_refused(self):
        with self.assertRaisesRegex(ValueError, "unterminated"):
            guide(b"# Guide\n```sh\ntrue\n")

    def test_ambiguous_section_refused(self):
        with self.assertRaisesRegex(ValueError, "duplicate"):
            guide(b"# Guide\n## Same\n## Same\n")

    def test_missing_linked_section_refused(self):
        with self.assertRaisesRegex(ValueError, "does not exist"):
            guide(b"# Guide\n").section("missing")

    def test_archive_preserves_bytes_and_refuses_overwrite(self):
        raw = b"# Guide\r\n```sh\r\ntrue\r\n```\r\n"
        with tempfile.TemporaryDirectory(dir=os.environ["FABRIC_SCRATCH_ROOT"]) as directory:
            value = guide(raw)
            record = value.archive(directory)
            self.assertEqual(Path(record["archive"]).read_bytes(), raw)
            self.assertEqual(record["blocks"][0]["command"], "true\r\n")
            with self.assertRaises(FileExistsError):
                value.archive(directory)
            self.assertEqual(Path(record["archive"]).read_bytes(), raw)


if __name__ == "__main__":
    unittest.main()
