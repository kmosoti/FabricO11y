"""Seeded, streaming alpha offer source; no application sink is implied.

One logical record per line, with exactly 512 ASCII body bytes per log. This
module produces offers at registered schedule coordinates. A future fleet driver
must honor those coordinates with an open-loop, byte-bounded dispatch queue.
"""

import argparse
import base64
import csv
import hashlib
import json
from pathlib import Path
import time

SEEDS = (0xA11FA001, 0xA11FA002, 0xA11FA003)
TIERS = (10, 100, 1000)


def entropy_body(seed: int, node: int, tick: int) -> str:
    source = f"fabric-alpha-v1:{seed}:{node}:{tick}".encode()
    raw = b"".join(hashlib.sha256(source + b":" + i.to_bytes(2, "big")).digest()
                   for i in range(13))
    return base64.b85encode(raw)[:512].decode("ascii")


def records_at(tier: int, second: int, seed: int):
    if tier not in TIERS or seed not in SEEDS or not isinstance(second, int) or isinstance(second, bool) or second < 0:
        raise ValueError("unregistered workload coordinate")
    for half in range(2):
        tick = second * 2 + half
        for node in range(tier):
            yield {"kind": "log", "node": node, "scheduled_ms": tick * 500,
                   "body": "R" * 512 if tick % 2 == 0 else entropy_body(seed, node, tick)}
    if second % 15 == 0:
        for node in range(tier):
            for point in range(32):
                digest = hashlib.sha256(f"metric:{seed}:{node}:{second}:{point}".encode()).digest()
                yield {"kind": "metric", "node": node, "scheduled_ms": second * 1000,
                       "point": point, "value": int.from_bytes(digest[:8], "big")}


def emit(tier: int, seconds: int, seed: int, output: Path, byte_cap: int,
         paced: bool = False) -> dict:
    if (tier not in TIERS or seed not in SEEDS or not isinstance(seconds, int)
            or isinstance(seconds, bool) or not 0 < seconds <= 135):
        raise ValueError("unregistered tier, seed or duration")
    if (not isinstance(byte_cap, int) or isinstance(byte_cap, bool)
            or byte_cap <= 0 or byte_cap > 5 * 1024**3):
        raise ValueError("invalid source byte cap")
    # A worst-case 900 bytes/record covers every serialized field. Refuse to
    # start when that conservative bound cannot fit; check exact bytes per write.
    metric_seconds = (seconds + 14) // 15
    records = 2 * tier * seconds + 32 * tier * metric_seconds
    if records * 900 > byte_cap:
        raise ValueError("preflight source footprint exceeds byte cap")
    if output.parent.resolve() != Path.cwd().resolve() or output.name in ("", ".", ".."):
        raise ValueError("source output must be a direct child of the working directory")
    if output.exists() or output.is_symlink():
        raise ValueError("source output must be new")
    rate_path = output.with_name("offered.csv")
    if rate_path.exists() or rate_path.is_symlink():
        raise ValueError("rate output must be new")
    written = 0
    source_hash = hashlib.sha256()
    max_lag_ms = 0.0
    started = time.monotonic()
    with output.open("xb") as sink, rate_path.open("x", newline="") as rates:
        writer = csv.writer(rates)
        writer.writerow(("second", "offered_logs", "offered_metrics", "admitted", "committed", "backlog", "gaps"))
        backlog = 0
        for second in range(seconds):
            count = 0
            offers = records_at(tier, second, seed)
            if paced:
                offers = sorted(offers, key=lambda item: item["scheduled_ms"])
            for record in offers:
                if paced:
                    deadline = started + record["scheduled_ms"] / 1000
                    time.sleep(max(0.0, deadline - time.monotonic()))
                    emitted_ms = (time.monotonic() - started) * 1000
                    max_lag_ms = max(max_lag_ms, emitted_ms - record["scheduled_ms"])
                    record["emitted_ms"] = round(emitted_ms, 3)
                line = (json.dumps(record, separators=(",", ":"), sort_keys=True) + "\n").encode()
                if written + len(line) > byte_cap:
                    raise ValueError("source byte cap reached before write")
                sink.write(line)
                source_hash.update(line)
                written += len(line)
                count += 1
            logs = 2 * tier
            metrics = 32 * tier if second % 15 == 0 else 0
            assert count == logs + metrics
            backlog += count
            writer.writerow((second, logs, metrics, count, 0, backlog, 0))
    return {"tier": tier, "seconds": seconds, "seed": seed, "records": records,
            "source_bytes": written, "source_sha256": source_hash.hexdigest(),
            "rate_file": rate_path.name, "sink": "none", "committed": 0,
            "paced": paced, "max_lag_ms": round(max_lag_ms, 3) if paced else None}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tier", type=int, required=True)
    parser.add_argument("--seconds", type=int, required=True)
    parser.add_argument("--seed", type=lambda s: int(s, 0), required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--byte-cap", type=int, required=True)
    parser.add_argument("--paced", action="store_true")
    args = parser.parse_args()
    print(json.dumps(emit(args.tier, args.seconds, args.seed, args.out, args.byte_cap,
                          args.paced), sort_keys=True))


if __name__ == "__main__":
    main()
