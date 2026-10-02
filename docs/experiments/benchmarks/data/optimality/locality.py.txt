"""Hypothesis B3 and the text-search floor: real log streams in their own order
against a random draw, per 1 MiB-class block (1,000 lines): compression, and
the fraction of blocks a token touches (what any block-level text index could
skip). Standard library plus zstandard."""
import json, random, sys, zstandard as zstd
from pathlib import Path
import research_paths
C = research_paths.CORPUS
BLOCK = int(sys.argv[1]) if len(sys.argv) > 1 else 1000
TOKENS = ["INFO", "WARN", "ERROR", "Exception", "blk_", "session", "kernel", "closed", "Failed", "timeout"]
rng = random.Random(7)
out = {}
for f in sorted(C.glob("*_2k.log")):
    lines = f.read_text(errors="replace").splitlines()
    shuffled = lines[:]; rng.shuffle(shuffled)
    def blocks(ls): return [ls[i:i + BLOCK] for i in range(0, len(ls), BLOCK)]
    def bytes_per_line(ls, level=3):
        return sum(len(zstd.ZstdCompressor(level=level).compress("\n".join(b).encode())) for b in blocks(ls)) / len(ls)
    r = {"lines": len(lines), "zstd3_stream_order": round(bytes_per_line(lines), 2), "zstd3_shuffled": round(bytes_per_line(shuffled), 2),
         "zstd19_stream_order": round(bytes_per_line(lines, 19), 2), "zstd19_shuffled": round(bytes_per_line(shuffled, 19), 2), "tokens": {}}
    for tok in TOKENS:
        total = sum(1 for l in lines if tok in l)
        if total == 0: continue
        bs = blocks(lines); bsh = blocks(shuffled)
        r["tokens"][tok] = {"lines_pct": round(100 * total / len(lines), 2),
                            "blocks_touched_stream_pct": round(100 * sum(1 for b in bs if any(tok in l for l in b)) / len(bs), 1),
                            "blocks_touched_shuffled_pct": round(100 * sum(1 for b in bsh if any(tok in l for l in b)) / len(bsh), 1)}
    out[f.stem] = r
    print(f"{f.stem:16} zstd3 stream {r['zstd3_stream_order']:5.1f} shuffled {r['zstd3_shuffled']:5.1f} B/line  (gain {r['zstd3_shuffled']/r['zstd3_stream_order']:.2f}x)  zstd19 {r['zstd19_stream_order']:5.1f}/{r['zstd19_shuffled']:5.1f}")
    for tok, t in r["tokens"].items():
        print(f"     {tok:10} {t['lines_pct']:6.2f}% of lines  blocks touched: stream {t['blocks_touched_stream_pct']:5.1f}%  shuffled {t['blocks_touched_shuffled_pct']:5.1f}%")
json.dump(out, open(research_paths.DATA / "locality.json", "w"), indent=1)
