#!/usr/bin/env python3
"""A cost model of the Observation record and its FOB1 block encoding.

Closed forms for bytes per record derived from the block layout
(crates/fabric-observation/src/block.rs), a resource model for CPU, disk and
memory fitted to measured points, and a calibration against those points.
Pure Python, standard library only. Run it for the capability tables; run
test_observation_model.py for the calibration bounds.

    python3 -B tools/model/observation_model.py            # capability tables
    python3 -B tools/model/observation_model.py calibrate  # predicted against measured
"""
from __future__ import annotations

import csv
import json
import math
import sys
from dataclasses import dataclass, field, replace
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DATA = ROOT / "docs" / "experiments" / "benchmarks" / "data"

# ---- level 2: varint length ------------------------------------------------

def varint_len(v: int) -> int:
    """Bytes of the shortest LEB128 form: ceil(bits / 7), at least 1."""
    if v < 0:
        raise ValueError("varint takes unsigned values")
    return max(1, math.ceil(max(1, v.bit_length()) / 7))


def zigzag(v: int) -> int:
    return (v << 1) if v >= 0 else ((-v << 1) - 1)


def delta_len(magnitude: int) -> int:
    """Bytes of a zigzagged step of the given magnitude."""
    return varint_len(zigzag(magnitude))


# ---- the structure: a workload described by its parameters -------------------

@dataclass
class Workload:
    """What a block holds. Per record unless stated."""
    records: int = 4096                 # N, records per block
    nodes_per_block: int = 1            # distinct node ids
    share_lines: float = 0.5            # fraction of records that are log lines
    share_points: float = 0.5           # metric points
    share_spans: float = 0.0            # spans (out of contract; the model still prices them)
    body_bytes: float = 130.0           # mean log body length
    time_jitter_ns: int = 1_000         # mean |second-order time step| (0 for a perfectly regular series)
    attrs: int = 1                      # attributes per record
    attr_key_bytes: int = 14            # mean key length (paid once per distinct key)
    attr_value_bytes: int = 12          # mean string value length (paid once per distinct value)
    attr_distinct_values: int = 16      # distinct string values per key in a block
    attr_distinct_keys: int = 6         # distinct keys in a block
    attr_distinct_values_total: int | None = None  # override: distinct string values in the whole block (default keys × values)
    point_value_bytes: float = 9.5      # varint bytes of a point's value (9.5 for random 63-bit ints; 1 to 3 for real gauges)
    point_names: int = 32               # distinct metric names in a block
    name_bytes: int = 12
    locators_share: float = 0.0         # fraction of records carrying trace locators
    sequence_step: int = 1              # usual step of the Batch sequence between records (0 inside one Batch)
    index_step: int = 1
    first_sequence_bytes: int = 5       # varint of the first record's absolute sequence
    first_time_bytes: int = 10          # varint of the first absolute time (1.8e18 ns needs 9 bytes; 10 covers u64)


# ---- level 8: bytes per block from the layout --------------------------------

@dataclass
class Bytes:
    header: float
    node_table: float
    string_table: float
    times: float
    strands: float
    tags: float
    locators: float
    attributes: float
    payloads: float
    crc: float = 4.0

    @property
    def total(self) -> float:
        return sum(getattr(self, f) for f in self.__dataclass_fields__)


def block_bytes(w: Workload) -> Bytes:
    n = w.records
    # A block can only reference as many distinct strings as its records carry.
    references = n * w.attrs
    keys = min(w.attr_distinct_keys, references)
    values_total = w.attr_distinct_values_total if w.attr_distinct_values_total is not None else w.attr_distinct_keys * w.attr_distinct_values
    values_total = min(values_total, references)
    names = min(w.point_names, math.ceil(n * w.share_points)) if w.share_points else 0
    distinct_strings = keys + values_total + names + 1  # unit "1" or "", event ""
    string_table = varint_len(distinct_strings) + keys * (1 + w.attr_key_bytes) \
        + values_total * (1 + w.attr_value_bytes) \
        + names * (1 + w.name_bytes) + 1
    header = 4 + varint_len(n)
    node_table = varint_len(w.nodes_per_block) + 16 * w.nodes_per_block
    times = w.first_time_bytes + (n - 1) * delta_len(w.time_jitter_ns)
    # strands: node id (1), Δgeneration (1, constant), Δsequence, Δindex; the first record pays the absolute values
    strands = (varint_len(w.nodes_per_block) + 1 + delta_len(w.sequence_step) + delta_len(w.index_step)) * (n - 1) \
        + (1 + 1 + w.first_sequence_bytes + 2)
    tags = n
    locators = n * w.locators_share * 24
    # attributes: count, then key id (1 or 2 bytes), tag, value id
    id_bytes = varint_len(max(distinct_strings - 1, 0))  # the largest id in the table
    attributes = n * (1 + w.attrs * (id_bytes + 1 + id_bytes))
    lines = n * w.share_lines
    points = n * w.share_points
    spans = n * w.share_spans
    payloads = lines * (1 + 1 + varint_len(int(w.body_bytes)) + w.body_bytes) \
        + points * (id_bytes + 1 + 1 + 1 + w.point_value_bytes) \
        + spans * (id_bytes + delta_len(1_234_567) + 2)
    return Bytes(header, node_table, string_table, times, strands, tags, locators, attributes, payloads)


def bytes_per_record(w: Workload) -> float:
    return block_bytes(w).total / w.records


# ---- resource model: fitted constants ----------------------------------------

@dataclass
class Machine:
    """What the host gives and what the codec costs on it (fitted on this
    container's CPU; see calibrate())."""
    ghz: float = 3.0
    cores_for_fabric: float = 2.0            # the registered two-CPU profile
    cpu_share_budget: float = 0.25           # share of those cores the codec may take
    # encode: base per record, per body byte, per attribute, extra per high-cardinality attribute
    # Least-squares fit on fobbench.csv (92 points; slicing-by-eight CRC, open-addressing
    # intern table, pre-sized buffers): median error about 15 % on both sides; the worst
    # points are one-record blocks. Earlier fits are kept in the model record.
    enc_per_block_ns: float = 410.0          # six column buffers, two dictionaries, the output (650 before pre-sizing)
    enc_base_ns: float = 135.0
    enc_per_byte_ns: float = 1.1             # two copies plus the CRC (3.0 with the bytewise CRC)
    enc_per_attr_ns: float = 65.0            # a hash lookup per key and per value (105 with the BTreeMap)
    enc_per_distinct_attr_ns: float = 50.0   # a table insert when the value is new (375 with the BTreeMap)
    # decode to owned records: the view below plus one String per string field
    dec_per_block_ns: float = 365.0
    dec_base_ns: float = 170.0
    dec_per_byte_ns: float = 0.9             # UTF-8 check, the CRC, one copy into a String (3.3 with the bytewise CRC)
    dec_per_attr_ns: float = 150.0           # two String clones (235 before the view)
    dec_per_distinct_attr_ns: float = 135.0
    dec_per_point_extra_ns: float = 95.0     # name and unit clones
    dec_per_span_extra_ns: float = 45.0
    # decode to the borrowing view (decode_view): no string is copied
    view_per_block_ns: float = 460.0
    view_base_ns: float = 72.0
    view_per_byte_ns: float = 0.8            # UTF-8 check and the CRC
    view_per_attr_ns: float = 45.0           # two dictionary lookups
    view_per_distinct_attr_ns: float = 115.0 # the distinctness check of a large table
    view_per_point_extra_ns: float = 55.0
    view_per_span_extra_ns: float = 55.0
    # compression stage (Zstd level 3 over a whole block of at least 1 MiB), by component.
    # Two measured points fit these (encoding run 01): they are structured, not independent.
    zstd_ratio_text: float = 9.0             # real log lines contiguous in the payload column (Parquet's body column, in smaller pages, reached 6.2)
    zstd_ratio_synthetic: float = 2.6        # the soak generator's lines: half repeated bytes (free), half random base64 (0.75)
    zstd_ratio_point_values: float = 1.0     # random 63-bit gauges do not compress; real gauges would, under the delta column
    zstd_ratio_rest: float = 4.0             # keys, times, tags, dictionary ids
    zstd_ns_per_byte: float = 4.0            # level 3 on this host, about 250 MB/s per core
    # disk and retention
    disk_write_mbs: float = 200.0
    retention_bytes: int = 20 * 1024**3
    journal_bytes: int = 4 * 1024**3
    # memory per decoded record (struct 200 B plus heap for strings and attributes)
    decoded_struct_bytes: int = 200
    decoded_per_attr_bytes: int = 80
    decoded_per_string_bytes: int = 24


def encode_ns(w: Workload, m: Machine) -> float:
    body = w.share_lines * w.body_bytes
    heavy = 1.0 if w.attr_distinct_values >= 1024 else 0.0
    return m.enc_per_block_ns / w.records + m.enc_base_ns + m.enc_per_byte_ns * body + w.attrs * (m.enc_per_attr_ns + heavy * m.enc_per_distinct_attr_ns)


def decode_ns(w: Workload, m: Machine) -> float:
    body = w.share_lines * w.body_bytes
    heavy = 1.0 if w.attr_distinct_values >= 1024 else 0.0
    return m.dec_per_block_ns / w.records + m.dec_base_ns + m.dec_per_byte_ns * body \
        + w.attrs * (m.dec_per_attr_ns + heavy * m.dec_per_distinct_attr_ns) \
        + w.share_points * m.dec_per_point_extra_ns + w.share_spans * m.dec_per_span_extra_ns


def view_ns(w: Workload, m: Machine) -> float:
    """Decode to the borrowing view, which a verifier or re-encoder needs; no copies."""
    body = w.share_lines * w.body_bytes
    heavy = 1.0 if w.attr_distinct_values >= 1024 else 0.0
    return m.view_per_block_ns / w.records + m.view_base_ns + m.view_per_byte_ns * body \
        + w.attrs * (m.view_per_attr_ns + heavy * m.view_per_distinct_attr_ns) \
        + w.share_points * m.view_per_point_extra_ns + w.share_spans * m.view_per_span_extra_ns


def compressed_bytes_per_record(w: Workload, m: Machine, text_ratio: float | None = None) -> float:
    """Three components under their own ratios: the text, the point values, the rest."""
    b = block_bytes(w)
    text = w.records * w.share_lines * w.body_bytes
    values = w.records * w.share_points * w.point_value_bytes
    rest = b.total - text - values
    ratio = text_ratio if text_ratio is not None else m.zstd_ratio_text
    return (text / ratio + values / m.zstd_ratio_point_values + rest / m.zstd_ratio_rest) / w.records


def decoded_block_bytes(w: Workload, m: Machine) -> float:
    per = m.decoded_struct_bytes + w.attrs * m.decoded_per_attr_bytes + w.share_lines * (w.body_bytes + m.decoded_per_string_bytes) \
        + w.share_points * 2 * m.decoded_per_string_bytes
    return w.records * per


# ---- capabilities ------------------------------------------------------------

@dataclass
class Fleet:
    nodes: int = 100
    lines_per_node_s: float = 2.0
    points_per_node_s: float = 32 / 15
    spans_per_node_s: float = 0.0

    @property
    def records_s(self) -> float:
        return self.nodes * (self.lines_per_node_s + self.points_per_node_s + self.spans_per_node_s)


def capabilities(w: Workload, m: Machine, f: Fleet) -> dict:
    total = f.lines_per_node_s + f.points_per_node_s + f.spans_per_node_s
    w = replace(w, share_lines=f.lines_per_node_s / total, share_points=f.points_per_node_s / total, share_spans=f.spans_per_node_s / total)
    raw = bytes_per_record(w)
    comp = compressed_bytes_per_record(w, m)
    rs = f.records_s
    cycles_budget = m.ghz * 1e9 * m.cores_for_fabric * m.cpu_share_budget
    enc = encode_ns(w, m); dec = view_ns(w, m)
    zstd = m.zstd_ns_per_byte * raw
    server_ns_per_record = dec + zstd            # the server decodes to the view once (verify) and compresses once (seal)
    return {
        "raw_B_per_record": raw,
        "compressed_B_per_record": comp,
        "records_per_s": rs,
        "journal_MB_per_s": rs * raw / 1e6,
        "segment_MB_per_s": rs * comp / 1e6,
        "retention_hours": m.retention_bytes / max(rs * comp, 1e-9) / 3600,
        "journal_cap_hours": m.journal_bytes / max(rs * raw, 1e-9) / 3600,
        "node_encode_us_per_s": rs / f.nodes * enc / 1e3,
        "server_cpu_share": rs * server_ns_per_record * 1e-9 / m.cores_for_fabric,
        "max_nodes_at_cpu_budget": cycles_budget / (m.ghz * server_ns_per_record * rs / f.nodes),
        "max_nodes_at_disk": m.disk_write_mbs * 1e6 / ((raw + comp) * rs / f.nodes),
        "decoded_block_MiB": decoded_block_bytes(w, m) / 1048576,
        "block_bytes": block_bytes(w).total,
    }


# ---- calibration against measured points --------------------------------------

def calibrate() -> list[dict]:
    """Predicted against measured, from the committed data files."""
    out = []
    m = Machine()
    # fobbench: synthetic records with controlled parameters (bytes and ns)
    bench = DATA / "observation-model" / "fobbench.csv"
    if bench.exists():
        for r in csv.DictReader(open(bench)):
            kind, n = int(r["kind"]), int(r["n"])
            dv = int(r["distinct_values"]); attrs = int(r["attrs"])
            w = Workload(records=n, nodes_per_block=1, share_lines=1.0 if kind == 0 else 0.0, share_points=1.0 if kind == 1 else 0.0,
                         share_spans=1.0 if kind == 2 else 0.0, body_bytes=float(r["body_len"]), time_jitter_ns=int(r["jitter_ns"]),
                         attrs=attrs, attr_key_bytes=10, attr_value_bytes=6 + len(str(max(min(dv, n * max(attrs, 1)) - 1, 0))), attr_distinct_values=min(dv, n), attr_distinct_keys=attrs, attr_distinct_values_total=min(dv, n * attrs) if attrs else 0,
                         point_names=min(32, n), name_bytes=12, locators_share=1.0 if kind == 2 else 0.0, sequence_step=0, index_step=1, first_sequence_bytes=2,
                         point_value_bytes=9.5)
            if kind != 0:
                w.attrs = attrs
            pb = bytes_per_record(w); mb = float(r["bytes_per_record"])
            pe = encode_ns(w, m); me = float(r["encode_ns_per_record"])
            pd = decode_ns(w, m); md = float(r["decode_ns_per_record"])
            rec = {"source": "fobbench", "case": f"kind{kind} n{n} body{r['body_len']} attrs{attrs} dv{dv} jit{r['jitter_ns']}",
                   "bytes": (pb, mb), "encode_ns": (pe, me), "decode_ns": (pd, md)}
            if "view_ns_per_record" in r:
                rec["view_ns"] = (view_ns(w, m), float(r["view_ns_per_record"]))
            out.append(rec)
    # the three journals of encoding run 01: records per file block, real and synthetic
    enc_dir = DATA / "observation-encoding"
    for name, w in [
        ("fob-gen-steady.json", Workload(records=3580, nodes_per_block=100, share_lines=110630 / 229030, share_points=118400 / 229030,
                                         body_bytes=512, time_jitter_ns=500_000_000, attrs=0.9, attr_distinct_values=4, attr_distinct_keys=6,
                                         attr_key_bytes=16, attr_value_bytes=20, point_value_bytes=9.5, sequence_step=1, index_step=1)),
        ("fob-gen-corpus.json", Workload(records=9693, nodes_per_block=100, share_lines=300020 / 620340, share_points=320320 / 620340,
                                         body_bytes=130.5, time_jitter_ns=500_000_000, attrs=0.9, attr_distinct_values=4, attr_distinct_keys=6,
                                         attr_key_bytes=16, attr_value_bytes=20, point_value_bytes=9.5, sequence_step=1, index_step=1)),
    ]:
        p = enc_dir / name
        if not p.exists():
            continue
        d = json.load(open(p))
        pb = bytes_per_record(w); mb = d["bytes_per_record"]["fob_per_file"]
        ratio = m.zstd_ratio_text if "corpus" in name else m.zstd_ratio_synthetic
        pc = compressed_bytes_per_record(w, m, ratio); mc = d["bytes_per_record"]["fob_per_file_zstd"]
        # Times of run 01 were measured on the crate before the slicing CRC and per-Batch
        # blocks of about four records; they calibrate nothing in the current fit.
        out.append({"source": "encoding run 01", "case": name.replace("fob-", "").replace(".json", "") + " per file block",
                    "bytes": (pb, mb), "compressed_bytes": (pc, mc)})
    return out


def errors(rows: list[dict]) -> dict[str, list[float]]:
    """Relative errors per metric, as fractions."""
    out: dict[str, list[float]] = {}
    for r in rows:
        for k in ("bytes", "compressed_bytes", "encode_ns", "decode_ns", "view_ns"):
            if k in r and r[k][1]:
                out.setdefault(k, []).append((r[k][0] - r[k][1]) / r[k][1])
    return out


def _fmt_cal(rows: list[dict]) -> str:
    lines = [f"{'case':52} {'metric':17} {'predicted':>10} {'measured':>10} {'error':>7}"]
    for r in rows:
        for k in ("bytes", "compressed_bytes", "encode_ns", "decode_ns", "view_ns"):
            if k in r:
                p, mv = r[k]
                err = (p - mv) / mv * 100 if mv else float("nan")
                lines.append(f"{r['case']:52} {k:17} {p:10.1f} {mv:10.1f} {err:+6.0f}%")
    for k, es in errors(rows).items():
        ae = [abs(e) for e in es]
        lines.append(f"summary {k:17} n={len(es):3} median |error| {sorted(ae)[len(ae)//2]*100:5.1f}%  max {max(ae)*100:5.1f}%")
    return "\n".join(lines)


def _fmt_caps() -> str:
    m = Machine()
    out = []
    for label, w in [("real text, 1 attr, regular times", Workload(body_bytes=130, attrs=1, time_jitter_ns=1_000, point_value_bytes=2)),
                     ("real text, 5 attrs, 1 µs jitter", Workload(body_bytes=130, attrs=5, time_jitter_ns=1_000_000, point_value_bytes=2)),
                     ("synthetic soak (512 B, random gauges)", Workload(body_bytes=512, attrs=1, time_jitter_ns=500_000_000, point_value_bytes=9.5))]:
        out.append(f"== {label}: {bytes_per_record(w):.1f} B/record raw, {compressed_bytes_per_record(w, m, m.zstd_ratio_synthetic if 'synthetic' in label else None):.1f} compressed, block of {w.records} records decodes to {decoded_block_bytes(w, m)/1048576:.1f} MiB")
        out.append(f"   encode {encode_ns(w, m):.0f} ns, decode to view {view_ns(w, m):.0f} ns, to owned records {decode_ns(w, m):.0f} ns per record")
        out.append(f"   {'nodes':>7} {'rec/s':>9} {'journal MB/s':>12} {'segment MB/s':>12} {'retention h':>11} {'journal fill h':>14} {'server CPU':>10} {'node µs/s':>9}")
        for nodes in (100, 1_000, 10_000, 100_000):
            c = capabilities(w, m, Fleet(nodes=nodes))
            out.append(f"   {nodes:7} {c['records_per_s']:9.0f} {c['journal_MB_per_s']:12.2f} {c['segment_MB_per_s']:12.2f} {c['retention_hours']:11.1f} {c['journal_cap_hours']:14.1f} {c['server_cpu_share']*100:9.1f}% {c['node_encode_us_per_s']:9.1f}")
        c = capabilities(w, m, Fleet(nodes=100))
        out.append(f"   at a {m.cpu_share_budget:.0%} share of {m.cores_for_fabric:.0f} cores the codec stage supports {c['max_nodes_at_cpu_budget']:,.0f} nodes; the disk {c['max_nodes_at_disk']:,.0f}")
    # block size sensitivity
    out.append("== block size: bytes per record for real text, 1 attr")
    for n in (1, 2, 16, 256, 4096, 65536):
        w = Workload(records=n, body_bytes=130, attrs=1, time_jitter_ns=1_000, point_value_bytes=2)
        out.append(f"   N={n:6}  {bytes_per_record(w):7.1f} B/record  (overhead {bytes_per_record(w) - 0.5*130:.1f} beyond the body share)")
    return "\n".join(out)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "calibrate":
        print(_fmt_cal(calibrate()))
    else:
        print(_fmt_caps())
