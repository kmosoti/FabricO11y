import pyarrow.parquet as pq, json, sys, os, collections
from pathlib import Path
import research_paths
out = {}
for name in sys.argv[1:] or ["seg-steady64", "seg-adversarial64"]:
    root = ((research_paths.DATA / "corpus-seal/states") if name == "seg-corpus64" else (research_paths.DATA / "topk/states")) / name / "segments"
    tables = collections.defaultdict(lambda: collections.defaultdict(lambda: {"comp": 0, "uncomp": 0, "rows": 0, "enc": set(), "type": None}))
    files = collections.Counter(); rows = collections.Counter(); groups = collections.Counter(); segs = 0; journal_bytes = 0
    for seg in sorted(root.glob("seg-*")):
        segs += 1; m = json.load(open(seg / "manifest.json"))
        for t in ["batches", "logs", "metrics", "gaps"]:
            f = seg / f"{t}.parquet"; files[t] += f.stat().st_size
            md = pq.read_metadata(f); rows[t] += md.num_rows; groups[t] += md.num_row_groups
            for rg in range(md.num_row_groups):
                g = md.row_group(rg)
                for c in range(g.num_columns):
                    col = g.column(c); d = tables[t][col.path_in_schema]
                    d["comp"] += col.total_compressed_size; d["uncomp"] += col.total_uncompressed_size; d["rows"] += col.num_values
                    d["enc"] |= set(col.encodings); d["type"] = col.physical_type
        files["manifest"] += (seg / "manifest.json").stat().st_size
    tot = sum(files.values())
    r = {"segments": segs, "total_bytes": tot, "per_table_bytes": dict(files), "rows": dict(rows), "row_groups": dict(groups), "columns": {}}
    for t, cols in tables.items():
        r["columns"][t] = {c: {"compressed": v["comp"], "uncompressed": v["uncomp"], "values": v["rows"], "type": v["type"], "encodings": sorted(v["enc"])} for c, v in cols.items()}
    out[name] = r
    print(f"== {name}: {segs} Segments, {tot/1048576:.1f} MiB")
    for t in ["batches", "logs", "metrics", "gaps", "manifest"]:
        print(f"  {t:9} {files[t]/1048576:7.2f} MiB {100*files[t]/tot:5.1f}%  rows {rows.get(t,0):8}  groups {groups.get(t,0)}")
    for t, cols in tables.items():
        print(f"  -- {t}")
        for c, v in sorted(cols.items(), key=lambda kv: -kv[1]["comp"]):
            print(f"     {c:22} {v['type']:12} comp {v['comp']/1048576:7.2f} MiB  uncomp {v['uncomp']/1048576:7.2f} MiB  ratio {v['uncomp']/max(v['comp'],1):5.1f}  B/val {v['comp']/max(v['rows'],1):6.1f}  {sorted(v['enc'])}")
tag = "-".join(sys.argv[1:]) or "default"; json.dump(out, open(fstr(research_paths.DATA / "topk/segstat-{tag}.json"), "w"), indent=1)
# journal input size for comparison
for name, gen in ([("seg-corpus64", str(research_paths.DATA / "corpus-seal/gen-corpus"))] if "seg-corpus64" in sys.argv else [("seg-steady64", str(research_paths.DATA / "retscale/gen-steady")), ("seg-adversarial64", str(research_paths.DATA / "topk/gen-adversarial"))]):
    fs = sorted(Path(gen).glob("sealed-*.faj"))[:64]; print(name, "journal input", sum(f.stat().st_size for f in fs)/1048576, "MiB over", len(fs), "files")
