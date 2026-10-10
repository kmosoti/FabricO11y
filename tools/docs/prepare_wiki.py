#!/usr/bin/env python3
"""Prepare a bounded, offline, review-pending wiki text export.

This tool never publishes, follows links, copies assets, or removes source data.
It stages only explicitly selected Markdown under the mounted project data drive.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tempfile
from urllib.parse import quote, unquote, urlsplit


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from resource_group import DATA_DRIVE, STORAGE, require_limits  # noqa: E402


MAX_FILES = 250
MAX_EXPORT_BYTES = 4 * 1024 * 1024
DATA_PARTS = {"data", "raw", "artifacts", "datasets", "telemetry"}
SKIP_DIRS = {"performance-frontier-labs"}

# Deliberately strict patterns: false positives are reported by filename and
# count only. This scan is a blocker for known credentials, not a privacy audit.
SECRET_PATTERNS = {
    "private_key": re.compile(r"-----BEGIN (?:RSA |OPENSSH |EC |DSA )?PRIVATE KEY-----"),
    "aws_access_key": re.compile(r"\bAKIA[0-9A-Z]{16}\b"),
    "github_token": re.compile(r"\bgh[pousr]_[A-Za-z0-9]{20,}\b"),
    "bearer_token": re.compile(r"(?i)\bAuthorization\s*:\s*Bearer\s+[A-Za-z0-9._~+/=-]{20,}"),
    "named_secret": re.compile(
        r"(?i)\b(?:password|passwd|secret|api[_-]?key|access[_-]?token|aws_secret_access_key)"
        r"\s*[:=]\s*[\"']?[A-Za-z0-9_+/=-]{24,}"
    ),
}
REVIEW_PATTERNS = {
    "local_path": re.compile(r"(?:/home/[^\s)\]>]+|/run/media/[^\s)\]>]+|[A-Za-z]:\\Users\\[^\s)\]>]+)"),
    "host_name": re.compile(r"(?i)\b(?:fedora-blade|digitalocean-\d+|[a-z0-9-]+\.(?:local|internal|lan))\b"),
    "ipv4_address": re.compile(r"\b(?:\d{1,3}\.){3}\d{1,3}\b"),
}

LINK_RE = re.compile(
    r"(?P<image>!?)\[(?P<label>(?:\\.|[^\]])*)\]"
    r"\((?P<dest><[^>\n]*>|(?:\\.|[^)\s])*)"
    r"(?P<tail>(?:\s+[\"'][^)]*[\"'])?)\)"
)
FENCE_RE = re.compile(r"^\s{0,3}(```+|~~~+)")


def git(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", "-C", str(ROOT), *args],
        check=check,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def has_data_part(path: PurePosixPath) -> bool:
    return any(part.casefold() in DATA_PARTS for part in path.parts)


def check_source_path(path: Path, root: Path = ROOT) -> bool:
    """Return true for a regular, non-symlink Markdown file outside data dirs."""
    try:
        rel = path.relative_to(root).as_posix()
    except ValueError:
        return False
    if has_data_part(PurePosixPath(rel)) or path.suffix.casefold() != ".md":
        return False
    current = root
    for part in PurePosixPath(rel).parts:
        current = current / part
        if current.is_symlink():
            return False
    if not path.is_file():
        return False
    with path.open("rb") as stream:
        if stream.read(256).startswith(b"<!-- canonical-wiki:"):
            return False
    return True


def selected_sources() -> list[tuple[Path, str, str]]:
    """Return (source path, category, page name) in stable repository order."""
    chosen: list[tuple[Path, str, str]] = []
    research = ROOT / "docs" / "research"
    for path in sorted(research.glob("*.md")):
        if check_source_path(path):
            chosen.append((path, "research", f"Research/{path.stem}"))

    experiments = ROOT / "docs" / "experiments"
    suffix = re.compile(r"-(?:findings|results|run-\d{2})\.md$", re.IGNORECASE)
    candidate_paths: list[Path] = []
    for directory, dirs, names in os.walk(experiments, topdown=True, followlinks=False):
        base = Path(directory)
        dirs[:] = sorted(
            name for name in dirs
            if name.casefold() not in DATA_PARTS | SKIP_DIRS
            and not (base / name).is_symlink()
        )
        candidate_paths.extend(base / name for name in names if name.endswith(".md"))
    for path in sorted(candidate_paths):
        rel = path.relative_to(experiments)
        if not suffix.search(path.name) or not check_source_path(path):
            continue
        rel_no_suffix = rel.with_suffix("")
        category = rel_no_suffix.parts[0] if len(rel_no_suffix.parts) > 1 else "misc"
        stem = "-".join(rel_no_suffix.parts[1:]) if len(rel_no_suffix.parts) > 1 else rel_no_suffix.parts[0]
        chosen.append((path, "experiment-results", f"Experiment-{category}-{stem}"))
    chosen.sort(key=lambda item: item[0].relative_to(ROOT).as_posix())
    return chosen


def wiki_slug(page_name: str) -> str:
    return "-".join(part.replace("_", "-").replace(" ", "-") for part in page_name.split("/"))


def github_repository_url() -> str:
    remote = git("remote", "get-url", "origin").stdout.strip()
    match = re.fullmatch(r"git@github\.com:([^/]+/[^/]+?)(?:\.git)?", remote)
    if not match:
        parsed = urlsplit(remote)
        if parsed.hostname != "github.com":
            raise RuntimeError("origin is not a GitHub repository; no wiki URL can be derived")
        repo_path = parsed.path.strip("/")
        if repo_path.endswith(".git"):
            repo_path = repo_path[:-4]
    else:
        repo_path = match.group(1)
    if not repo_path or "/" not in repo_path:
        raise RuntimeError("could not derive a GitHub repository URL from origin")
    return f"https://github.com/{repo_path}"


def origin_main_has(repo_path: str, oid: str, cache: dict[str, bool]) -> bool:
    if repo_path not in cache:
        result = git("cat-file", "-e", f"{oid}:{repo_path}", check=False)
        cache[repo_path] = result.returncode == 0
    return cache[repo_path]


def escaped_label(label: str) -> str:
    # Link labels are source text; avoid allowing a rewritten path to create a
    # second Markdown link or HTML tag.
    return label.replace("<", "&lt;").replace(">", "&gt;")


def split_destination(value: str) -> tuple[str, str]:
    value = value[1:-1] if value.startswith("<") and value.endswith(">") else value
    value = value.replace(r"\(", "(").replace(r"\)", ")")
    parsed = urlsplit(value)
    suffix = ("?" + parsed.query if parsed.query else "") + ("#" + parsed.fragment if parsed.fragment else "")
    return unquote(parsed.path), suffix


def render_local(label: str, phrase: str) -> str:
    text = escaped_label(label)
    return f"{text} ({phrase})" if text else phrase


def transform_link(
    match: re.Match[str],
    source: Path,
    selected: dict[Path, str],
    repo_url: str,
    origin_main_oid: str,
    cat_cache: dict[str, bool],
    link_log: list[dict[str, object]],
    line: int,
) -> str:
    label = match.group("label")
    original_dest = match.group("dest")
    raw_dest = original_dest[1:-1] if original_dest.startswith("<") and original_dest.endswith(">") else original_dest
    parsed = urlsplit(raw_dest)
    image = bool(match.group("image"))

    if parsed.scheme.casefold() in {"http", "https", "mailto"} or parsed.netloc:
        link_log.append({"line": line, "status": "external_unverified"})
        return match.group(0)
    if not parsed.scheme and not parsed.netloc and not parsed.path:
        link_log.append({"line": line, "status": "same_page_anchor"})
        return match.group(0)

    path_text, suffix = split_destination(raw_dest)
    path_obj = Path(path_text)
    if path_obj.is_absolute() or path_text.startswith(("file:", "~")):
        link_log.append({"line": line, "status": "local_absolute_path_review", "destination": "omitted"})
        return render_local(label, "local evidence omitted from stage")

    relative = PurePosixPath(os.path.normpath((source.parent.relative_to(ROOT) / path_text).as_posix()))
    if relative.is_absolute() or ".." in relative.parts:
        link_log.append({"line": line, "status": "outside_repository_local_evidence", "destination": "omitted"})
        return render_local(label, "local evidence omitted from stage")
    if has_data_part(relative):
        prefix: list[str] = []
        for part in relative.parts:
            if part.casefold() in DATA_PARTS:
                prefix.extend([part, "<excluded>"])
                break
            prefix.append(part)
        link_log.append({"line": line, "status": "data_tree_excluded", "destination": "/".join(prefix)})
        return render_local(label, "local evidence omitted from stage")

    target_path = ROOT.joinpath(*relative.parts)
    if image:
        link_log.append({"line": line, "status": "asset_not_staged_review"})
        return render_local(label, "asset not staged; see local checkout")

    candidate = target_path.resolve(strict=False)
    if candidate in selected:
        page_url = repo_url + "/wiki/" + quote(wiki_slug(selected[candidate]), safe="-") + suffix
        link_log.append({"line": line, "status": "selected_wiki_page_unpublished", "destination": selected[candidate]})
        title = match.group("tail")
        return f"[{escaped_label(label)}]({page_url}{title})"

    repo_path = relative.as_posix()
    if origin_main_has(repo_path, origin_main_oid, cat_cache):
        blob = repo_url + "/blob/" + origin_main_oid + "/" + quote(repo_path, safe="/-._~") + suffix
        link_log.append({"line": line, "status": "origin_main_link"})
        title = match.group("tail")
        return f"[{escaped_label(label)}]({blob}{title})"

    link_log.append({"line": line, "status": "unpublished_or_unresolvable_local_reference", "destination": repo_path})
    return render_local(label, f"local checkout reference `{repo_path}`; not present at origin/main")


def rewrite_links(
    text: str,
    source: Path,
    selected: dict[Path, str],
    repo_url: str,
    origin_main_oid: str,
    cat_cache: dict[str, bool],
) -> tuple[str, list[dict[str, object]]]:
    links: list[dict[str, object]] = []
    out: list[str] = []
    fence: tuple[str, int] | None = None
    pending: list[str] = []
    start_line = 1

    def flush():
        block = "".join(pending)
        out.append(LINK_RE.sub(
            lambda match: transform_link(
                match, source, selected, repo_url, origin_main_oid, cat_cache,
                links, start_line + block.count("\n", 0, match.start()),
            ), block))
        pending.clear()

    for line_num, line in enumerate(text.splitlines(keepends=True), 1):
        fence_match = FENCE_RE.match(line)
        if fence is not None:
            out.append(line)
            if fence_match and fence_match.group(1)[0] == fence[0] and len(fence_match.group(1)) >= fence[1]:
                fence = None
            continue
        if fence_match:
            flush()
            marker = fence_match.group(1)
            fence = (marker[0], len(marker))
            out.append(line)
            continue
        if not pending:
            start_line = line_num
        pending.append(line)
    flush()
    return "".join(out), links


def scan_secrets(relative_path: str, text: str) -> dict[str, int]:
    counts = {name: len(pattern.findall(text)) for name, pattern in SECRET_PATTERNS.items()}
    return {name: count for name, count in counts.items() if count}


def review_flags(text: str) -> dict[str, int]:
    return {name: len(pattern.findall(text)) for name, pattern in REVIEW_PATTERNS.items()}


def self_check() -> None:
    """Exercise the export boundary without creating a wiki stage."""
    require_limits()
    if not DATA_DRIVE.is_mount():
        raise RuntimeError(f"data drive is not mounted: {DATA_DRIVE}")
    if not has_data_part(PurePosixPath("docs/experiments/DATA/sample.md")):
        raise AssertionError("data-directory deny rule failed")

    scratch = STORAGE / "scratch"
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="wiki-stage-check-", dir=scratch) as temporary:
        root = Path(temporary)
        (root / "research").mkdir()
        allowed = root / "research" / "allowed.md"
        allowed.write_text("# fixture\n", encoding="utf-8")
        (root / "research" / "linked.md").symlink_to(allowed)
        (root / "research" / "data").mkdir()
        (root / "research" / "data" / "blocked.md").write_text("x", encoding="utf-8")
        if not check_source_path(allowed, root):
            raise AssertionError("ordinary Markdown source was rejected")
        if check_source_path(root / "research" / "linked.md", root):
            raise AssertionError("symlinked Markdown source was admitted")
        if check_source_path(root / "research" / "data" / "blocked.md", root):
            raise AssertionError("Markdown below data/ was admitted")

    token = "ghp_" + "A" * 36
    if not scan_secrets("synthetic.md", f"token={token}"):
        raise AssertionError("known token pattern was not blocked")

    source = ROOT / "docs" / "research" / "application-use-cases.md"
    target = ROOT / "docs" / "research" / "storage-direction.md"
    fixture = (
        "[selected](storage-direction.md)\n"
        "[wrapped\nlabel](storage-direction.md)\n"
        "```md\n[selected](storage-direction.md)\n```\n"
        "[raw](../experiments/data/fake.json)\n"
    )
    rewritten, links = rewrite_links(
        fixture,
        source,
        {target.resolve(): "Research-storage-direction"},
        "https://github.com/example/repo",
        "0" * 40,
        {},
    )
    if "https://github.com/example/repo/wiki/Research-storage-direction" not in rewritten:
        raise AssertionError("selected page link was not rewritten")
    if "[wrapped\nlabel](https://github.com/example/repo/wiki/Research-storage-direction)" not in rewritten:
        raise AssertionError("multiline link label was not rewritten (migration counterexample)")
    if "```md\n[selected](storage-direction.md)\n```" not in rewritten:
        raise AssertionError("fenced example was rewritten")
    if "local evidence omitted from stage" not in rewritten:
        raise AssertionError("data link did not become a local evidence note")
    if not any(item["status"] == "data_tree_excluded" for item in links):
        raise AssertionError("data link exclusion was not recorded")


def output_header(relative_path: str, source_hash: str) -> str:
    return (
        "> Historical snapshot prepared for review; publication is pending.\n"
        f"> Original repository path: `{relative_path}`\n"
        f"> Original file SHA-256: `{source_hash}`\n"
        "> The report's stated measurement revision and results remain unchanged.\n"
        "> Only this wrapper and relative link destinations were changed; inspect\n"
        "> the export before publication.\n\n"
    )


def prepare(out_arg: str) -> Path:
    require_limits()
    if not DATA_DRIVE.is_mount():
        raise RuntimeError(f"data drive is not mounted: {DATA_DRIVE}")
    storage = STORAGE.resolve(strict=True)
    supplied = Path(out_arg).expanduser()
    if not supplied.is_absolute():
        raise RuntimeError("--out must be an absolute path under the mounted project data drive")
    out = Path(os.path.abspath(supplied))
    if out.exists():
        raise FileExistsError("--out already exists; choose a fresh output directory")
    lexical_parent = out.parent
    parent = lexical_parent.resolve(strict=True)
    if parent != storage and storage not in parent.parents:
        raise RuntimeError(f"--out parent must be inside {storage}")
    try:
        relative_parent = lexical_parent.relative_to(storage)
    except ValueError as error:
        raise RuntimeError(f"--out parent must be lexically inside {storage}") from error
    cursor = storage
    for part in relative_parent.parts:
        cursor = cursor / part
        if cursor.is_symlink():
            raise RuntimeError("--out parent path must not contain symlinks")
    cursor = parent
    while cursor != storage:
        if cursor.is_symlink():
            raise RuntimeError("--out parent path must not contain symlinks")
        cursor = cursor.parent
    if out.name in {"", ".", ".."}:
        raise RuntimeError("invalid --out directory name")

    sources = selected_sources()
    if not sources or len(sources) > MAX_FILES:
        raise RuntimeError(f"selected page count {len(sources)} outside 1..{MAX_FILES}")
    source_map: dict[Path, str] = {}
    page_paths: dict[Path, str] = {}
    for source, _category, page_name in sources:
        resolved = source.resolve(strict=True)
        if not check_source_path(source):
            raise RuntimeError(f"source is not a regular non-symlink Markdown file: {source.relative_to(ROOT)}")
        if resolved in page_paths:
            raise RuntimeError("two input files resolve to the same source page")
        source_map[resolved] = page_name
        page_paths[resolved] = page_name
    repo_url = github_repository_url()
    origin_main_oid = git("rev-parse", "origin/main").stdout.strip()

    head = git("rev-parse", "HEAD").stdout.strip()
    dirty = bool(git("status", "--porcelain", "--untracked-files=normal").stdout)
    cat_cache: dict[str, bool] = {}
    page_records: list[dict[str, object]] = []
    pages_to_write: list[tuple[Path, bytes]] = []
    global_secrets: list[tuple[str, dict[str, int]]] = []
    export_size = 0

    for source, category, page_name in sources:
        relative_path = source.relative_to(ROOT).as_posix()
        raw = source.read_bytes()
        try:
            original_text = raw.decode("utf-8")
        except UnicodeDecodeError as error:
            raise RuntimeError(f"selected Markdown is not UTF-8: {relative_path}") from error
        secret_counts = scan_secrets(relative_path, original_text)
        if secret_counts:
            global_secrets.append((relative_path, secret_counts))
        transformed, link_records = rewrite_links(
            original_text, source.resolve(strict=True), source_map, repo_url, origin_main_oid, cat_cache
        )
        rendered = output_header(relative_path, sha256(raw)) + transformed
        rendered_bytes = rendered.encode("utf-8")
        output_rel = Path("pages") / category / source.relative_to(
            ROOT / "docs" / ("research" if category == "research" else "experiments")
        )
        output_rel = output_rel.with_suffix(".md")
        output_rel_text = output_rel.as_posix()
        pages_to_write.append((output_rel, rendered_bytes))
        export_size += len(rendered_bytes)
        flag_counts = review_flags(original_text)
        page_records.append(
            {
                "source": relative_path,
                "source_sha256": sha256(raw),
                "category": category,
                "page_name": page_name,
                "staged_path": output_rel_text,
                "staged_sha256": sha256(rendered_bytes),
                "review_flags": flag_counts,
                "manual_review_required": any(flag_counts.values()),
                "links": link_records,
            }
        )

    if global_secrets:
        for filename, counts in global_secrets:
            print(f"credential scan: blocked {filename}; pattern_counts={counts}", file=sys.stderr)
        raise RuntimeError("credential scan blocked staging; no output directory created")

    index_lines = [
        "# Prepared research and experiment reports",
        "",
        "> Historical text export for review. Nothing here has been published.",
        "",
        f"Base commit: `{head}`; working tree dirty: `{str(dirty).lower()}`.",
        "",
    ]
    for title, category in (("Research", "research"), ("Experiment results", "experiment-results")):
        matching = [record for record in page_records if record["category"] == category]
        index_lines.extend([f"## {title}", ""])
        for record in matching:
            relative = Path(str(record["staged_path"]))
            index_lines.append(f"- [{record['page_name']}]({relative.as_posix()})")
        index_lines.append("")
    index_bytes = ("\n".join(index_lines).rstrip() + "\n").encode("utf-8")
    export_size += len(index_bytes)

    # Manifest is included in the same strict byte budget. Data destinations
    # are truncated at their excluded directory; absolute local destinations
    # are omitted.
    manifest = {
        "format": "fabric-wiki-stage-v1",
        "status": "prepared_review_pending_not_published",
        "base_head": head,
        "working_tree_dirty": dirty,
        "origin_main_reference_check": "local_git_cat_file_only_no_network",
        "origin_main_oid": origin_main_oid,
        "wiki_base": repo_url + "/wiki",
        "limits": {"max_pages": MAX_FILES, "max_export_bytes": MAX_EXPORT_BYTES},
        "selected_page_count": len(page_records),
        "review_flags_note": (
            "Counts are review signals only. Scanners do not establish that text is safe for publication."
        ),
        "credential_scan": {"known_pattern_matches": 0, "known_patterns": sorted(SECRET_PATTERNS)},
        "pages": page_records,
    }
    manifest_bytes = (json.dumps(manifest, indent=2, ensure_ascii=False) + "\n").encode("utf-8")
    export_size += len(manifest_bytes)
    if export_size > MAX_EXPORT_BYTES:
        raise RuntimeError(f"prepared text would be {export_size} bytes; limit is {MAX_EXPORT_BYTES}")

    # Exclusive create prevents an existing directory from being overwritten.
    out.mkdir(mode=0o700, parents=False, exist_ok=False)
    try:
        for relative, payload in pages_to_write:
            destination = out / relative
            destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            if any(parent.is_symlink() for parent in [destination.parent, *destination.parent.parents] if parent != out.parent):
                raise RuntimeError("symlink appeared in staging destination")
            with destination.open("xb") as stream:
                stream.write(payload)
                stream.flush()
                os.fsync(stream.fileno())
        with (out / "Index.md").open("xb") as stream:
            stream.write(index_bytes)
            stream.flush()
            os.fsync(stream.fileno())
        with (out / "manifest.json").open("xb") as stream:
            stream.write(manifest_bytes)
            stream.flush()
            os.fsync(stream.fileno())
        directory_fd = os.open(out, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    except Exception:
        # Keep any partial owned stage for inspection; never delete evidence.
        raise
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out",
        help="fresh absolute output directory under /run/media/kmosoti/data/FabricO11y",
    )
    parser.add_argument(
        "--self-check",
        action="store_true",
        help="run staging-policy checks in bounded, data-drive scratch",
    )
    args = parser.parse_args()
    require_limits()
    if args.self_check:
        self_check()
        print("wiki staging self-check: passed")
        return 0
    if not args.out:
        parser.error("--out is required")
    result = prepare(args.out)
    print(f"prepared review stage: {result}")
    print(f"pages: {len(selected_sources())}; published: no")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"wiki staging: NOT PREPARED: {error}", file=sys.stderr)
        raise SystemExit(2)
