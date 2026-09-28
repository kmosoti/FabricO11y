"""Independent closed-form rate and exact seeded-source oracle."""

import base64
import csv
import hashlib
import json
import math
from pathlib import Path

FIELDS = ("second", "offered_logs", "offered_metrics", "admitted", "committed", "backlog", "gaps")
SEEDS = (0xA11FA001, 0xA11FA002, 0xA11FA003)


def _unique_pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def _expected_entropy(seed: int, node: int, tick: int) -> str:
    prefix = b"fabric-alpha-v1:" + str(seed).encode() + b":" + str(node).encode() + b":" + str(tick).encode() + b":"
    blocks = bytearray()
    for counter in range(13):
        blocks += hashlib.sha256(prefix + counter.to_bytes(2, "big")).digest()
    return base64.b85encode(blocks).decode("ascii")[:512]


def _expected_metric(seed: int, node: int, second: int, point: int) -> int:
    source = b"metric:" + str(seed).encode() + b":" + str(node).encode() + b":" + str(second).encode() + b":" + str(point).encode()
    return int.from_bytes(hashlib.sha256(source).digest()[:8], "big")


def check_rate(path: Path, tier: int, seconds: int) -> None:
    if (not isinstance(tier, int) or isinstance(tier, bool)
            or tier not in (10, 100, 1000) or not isinstance(seconds, int)
            or isinstance(seconds, bool) or not 0 < seconds <= 135):
        raise ValueError("unregistered rate contract")
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 32_000:
        raise ValueError("missing, linked or oversized rate file")
    with path.open(newline="") as source:
        reader = csv.DictReader(source)
        if tuple(reader.fieldnames or ()) != FIELDS:
            raise ValueError("rate columns mismatch")
        balance = 0
        rows = 0
        for row in reader:
            if set(row) != set(FIELDS):
                raise ValueError("unexpected rate columns")
            try:
                values = [int(row[key]) for key in FIELDS]
            except (TypeError, KeyError, ValueError) as error:
                raise ValueError("malformed rate row") from error
            second, logs, metrics, admitted, committed, backlog, gaps = values
            if (second != rows or logs != tier * 2 or
                    metrics != (tier * 32 if second % 15 == 0 else 0)):
                raise ValueError(f"offered rate mismatch at second {rows}")
            if min(admitted, committed, backlog, gaps) < 0 or admitted + gaps != logs + metrics:
                raise ValueError(f"invalid admission/gap accounting at second {rows}")
            balance += admitted - committed
            if balance < 0 or backlog != balance:
                raise ValueError(f"backlog mismatch at second {rows}")
            rows += 1
        if rows != seconds:
            raise ValueError(f"rate duration mismatch: {rows} != {seconds}")


def check_source(path: Path, tier: int, seconds: int) -> int:
    """Check exact generated contents against one registered seed; return it."""
    if (not isinstance(tier, int) or isinstance(tier, bool)
            or tier not in (10, 100, 1000) or not isinstance(seconds, int)
            or isinstance(seconds, bool) or not 0 < seconds <= 135):
        raise ValueError("unregistered source contract")
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 5 * 1024**3:
        raise ValueError("missing, linked or oversized offer source")
    seen = set()
    current_second = 0
    total = 0
    seed_candidates = set(SEEDS)
    paced = None
    with path.open("rb") as source:
        while True:
            line = source.readline(4096)
            if not line:
                break
            if not line.endswith(b"\n"):
                raise ValueError("oversized or incomplete offer line")
            try:
                item = json.loads(line, object_pairs_hook=_unique_pairs)
                if not isinstance(item, dict):
                    raise ValueError("offer must be a JSON object")
                node = item["node"]
                scheduled = item["scheduled_ms"]
                kind = item["kind"]
                if (not isinstance(node, int) or isinstance(node, bool)
                        or not 0 <= node < tier or not isinstance(scheduled, int)
                        or isinstance(scheduled, bool)):
                    raise ValueError("invalid offer identity")
                second = scheduled // 1000
                if not 0 <= second < seconds or second < current_second:
                    raise ValueError("offer outside ordered duration")
                if second != current_second:
                    if second != current_second + 1:
                        raise ValueError("missing offered second")
                    expected = 2 * tier + (32 * tier if current_second % 15 == 0 else 0)
                    if len(seen) != expected:
                        raise ValueError("missing offer in closed second")
                    seen.clear()
                    current_second = second
                if kind == "log":
                    allowed = {"kind", "node", "scheduled_ms", "body"}
                    if scheduled % 1000 not in (0, 500) or not isinstance(item["body"], str):
                        raise ValueError("invalid log schedule or body")
                    body = item["body"]
                    if len(body) != 512 or not body.isascii():
                        raise ValueError("invalid log body size")
                    if (scheduled // 500) % 2 == 0 and body != "R" * 512:
                        raise ValueError("repetitive half mismatch")
                    if (scheduled // 500) % 2 == 1:
                        seed_candidates = {seed for seed in seed_candidates
                                           if body == _expected_entropy(seed, node, scheduled // 500)}
                        if not seed_candidates:
                            raise ValueError("seeded entropy body mismatch")
                    key = ("log", scheduled, node)
                elif kind == "metric":
                    allowed = {"kind", "node", "scheduled_ms", "point", "value"}
                    point = item["point"]
                    value = item["value"]
                    if (second % 15 != 0 or scheduled % 1000 != 0
                            or not isinstance(point, int) or isinstance(point, bool)
                            or not 0 <= point < 32 or not isinstance(value, int)
                            or isinstance(value, bool) or not 0 <= value < 2**64):
                        raise ValueError("invalid metric offer")
                    seed_candidates = {seed for seed in seed_candidates
                                       if value == _expected_metric(seed, node, second, point)}
                    if not seed_candidates:
                        raise ValueError("seeded metric value mismatch")
                    key = ("metric", scheduled, node, point)
                else:
                    raise ValueError("unknown offer kind")
                if key in seen:
                    raise ValueError("duplicate offer identity")
                has_emitted = "emitted_ms" in item
                if paced is None:
                    paced = has_emitted
                elif paced != has_emitted:
                    raise ValueError("mixed paced and unpaced records")
                if set(item) != allowed | ({"emitted_ms"} if has_emitted else set()):
                    raise ValueError("unexpected offer fields")
                if "emitted_ms" in item:
                    emitted = item["emitted_ms"]
                    if (not isinstance(emitted, (int, float)) or isinstance(emitted, bool)
                            or not math.isfinite(emitted)
                            or emitted + 0.001 < scheduled):
                        raise ValueError("invalid observed offer time")
                seen.add(key)
                total += 1
            except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
                raise ValueError(f"invalid offer record {total + 1}: {error}") from error
    expected = 2 * tier + (32 * tier if current_second % 15 == 0 else 0)
    if len(seen) != expected or current_second != seconds - 1:
        raise ValueError("missing offers at end of source")
    if len(seed_candidates) != 1:
        raise ValueError("source seed is ambiguous or unregistered")
    return seed_candidates.pop()
