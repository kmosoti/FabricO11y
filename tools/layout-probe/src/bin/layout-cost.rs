#[path = "../../../storage-probe/src/bin/common/mod.rs"]
mod common;
use common::{Measure, measured};
use fabric_o11y::Event;
use layout_probe::{self, Codec, Hit, TableAnchor};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    num::NonZeroUsize,
    path::{Path, PathBuf},
};
use storage_probe::{Query, coverage, disk};
#[derive(Serialize)]
struct Sample {
    index: usize,
    family: usize,
    mode: &'static str,
    timing: Measure,
    matches: usize,
    logical_data_bytes: usize,
}
#[derive(Serialize)]
struct ResultRow {
    schema: u32,
    benchmark: &'static str,
    mode: String,
    shape: String,
    seed: u64,
    trial: i64,
    events: usize,
    query_count: usize,
    layout: String,
    source_sha256: String,
    output_sha256: String,
    answer_sha256: String,
    source_bytes: usize,
    layout_bytes: usize,
    metadata_bytes: usize,
    parquet_footer_bytes: usize,
    table_anchor_bytes: usize,
    publication: Measure,
    file_read: Measure,
    index_build: Option<Measure>,
    index_publication: Option<Measure>,
    index_bytes: usize,
    index_data_bytes: usize,
    index_digest_bytes: usize,
    samples: Vec<Sample>,
    p50_full_ns: u128,
    p99_full_ns: u128,
    p50_projected_ns: u128,
    p99_projected_ns: u128,
    calibration: Vec<Measure>,
    rss_kib: u64,
    process_cpu_ns: u128,
    kernel_io_delta: common::IoCounters,
    checks_ok: bool,
    json_validation_boundary: Option<&'static str>,
    validation_boundary: &'static str,
}
fn err(e: impl std::fmt::Debug) -> io::Error {
    io::Error::other(format!("{e:?}"))
}
fn write_sync(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}
fn parent_sync(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}
fn hits(rows: &[Event], q: &Query) -> Vec<(usize, coverage::Digest)> {
    common::expected(rows, q)
}
fn pairs(h: &[Hit]) -> Vec<(usize, coverage::Digest)> {
    h.iter().map(|x| (x.position, x.digest)).collect()
}
fn json_scan(
    dir: &Path,
    hashes: &[String],
    q: &Query,
) -> io::Result<Vec<(usize, coverage::Digest)>> {
    let mut out = Vec::new();
    for (b, h) in hashes.iter().enumerate() {
        let bytes = fs::read(dir.join(format!("block-{b:04}.json")))?;
        if common::sha_hex(&bytes) != *h {
            return Err(err("JSON block hash mismatch"));
        }
        let rows = disk::decode_events(&bytes)?;
        for (off, e) in rows.iter().enumerate() {
            if common::matches(e, q) {
                out.push((b * 64 + off, coverage::rows_digest(std::slice::from_ref(e))));
            }
        }
    }
    Ok(out)
}
fn run() -> io::Result<()> {
    let a: Vec<_> = std::env::args().collect();
    if a.len() != 9 {
        return Err(err(
            "usage: layout-cost MODE SHAPE SEED EVENTS TRIAL LAYOUT SOURCE OUTPUT_DIR",
        ));
    }
    let mode = &a[1];
    let shape = &a[2];
    let seed = a[3].parse::<u64>().map_err(err)?;
    let count = a[4].parse::<usize>().map_err(err)?;
    let trial = a[5].parse::<i64>().map_err(err)?;
    let layout = &a[6];
    let source = &a[7];
    let out = PathBuf::from(&a[8]);
    fs::create_dir(&out)?;
    // The newly created directory entry must already be durable when the
    // publication timer starts; all variants share this parent boundary.
    parent_sync(out.parent().ok_or_else(|| err("missing output parent"))?)?;
    parent_sync(&out)?;
    let cpu_start = common::cpu_ns()?;
    let io_start = common::io_counters()?;
    let calibration = common::calibration()?;
    let source_bytes = fs::read(source)?;
    let rows = disk::decode_events(&source_bytes)?;
    let (generated, base) = common::workload(shape, seed, count);
    if coverage::rows_digest(&rows) != coverage::rows_digest(&generated) {
        return Err(err("source mismatch"));
    }
    let qs = common::queries(base, count);
    let wants: Vec<_> = qs.iter().map(|q| hits(&rows, q)).collect();
    let mut hashes = Vec::new();
    let mut table = None;
    let mut layout_bytes = 0;
    let mut metadata_bytes = 0;
    let mut parquet_footer_bytes = 0;
    let mut table_anchor_bytes = 0;
    let (published, publication) = measured(|| -> io::Result<()> {
        if layout == "json64" {
            for (i, block) in rows.chunks(64).enumerate() {
                let b = disk::encode_events(block)?;
                hashes.push(common::sha_hex(&b));
                layout_bytes += b.len();
                write_sync(&out.join(format!("block-{i:04}.json")), &b)?;
            }
            let meta = serde_json::to_vec(&hashes).map_err(err)?;
            metadata_bytes = meta.len();
            write_sync(&out.join("metadata.json"), &meta)?;
        } else {
            let (group, codec) = match layout.as_str() {
                "plain64" => (64, Codec::Plain),
                "zstd64" => (64, Codec::Zstd),
                "zstd256" => (256, Codec::Zstd),
                _ => return Err(err("invalid layout")),
            };
            let (b, anchor) =
                layout_probe::encode(&rows, NonZeroUsize::new(group).unwrap(), codec)?;
            layout_bytes = b.len();
            if b.len() < 8 || &b[b.len() - 4..] != b"PAR1" {
                return Err(err("invalid Parquet footer"));
            }
            parquet_footer_bytes =
                u32::from_le_bytes(b[b.len() - 8..b.len() - 4].try_into().unwrap()) as usize;
            let anchor_wire = serde_json::to_vec(&anchor).map_err(err)?;
            table_anchor_bytes = anchor_wire.len();
            metadata_bytes = anchor_wire.len();
            write_sync(&out.join("table.parquet"), &b)?;
            write_sync(&out.join("table-anchor.json"), &anchor_wire)?;
            table = Some(anchor);
        }
        parent_sync(&out)
    })?;
    published?;
    let (read, file_read) = measured(|| -> io::Result<()> {
        if layout == "json64" {
            let _ = fs::read(out.join("metadata.json"))?;
            for i in 0..hashes.len() {
                let _ = fs::read(out.join(format!("block-{i:04}.json")))?;
            }
        } else {
            let _ = fs::read(out.join("table.parquet"))?;
            let _ = fs::read(out.join("table-anchor.json"))?;
        }
        Ok(())
    })?;
    read?;
    if let Some(original) = &table {
        let retained: TableAnchor =
            serde_json::from_slice(&fs::read(out.join("table-anchor.json"))?).map_err(err)?;
        if &retained != original {
            return Err(err("retained table anchor mismatch"));
        }
        table = Some(retained);
    }
    // Before query timing, require complete Event equality via the lossless S2 wire
    // representation, including raw float bits, attribute order and duplicates.
    let decoded = if layout == "json64" {
        let mut all = Vec::new();
        for (i, h) in hashes.iter().enumerate() {
            let b = fs::read(out.join(format!("block-{i:04}.json")))?;
            if common::sha_hex(&b) != *h {
                return Err(err("JSON block authentication failed"));
            }
            all.extend(disk::decode_events(&b)?);
        }
        all
    } else {
        let b = fs::read(out.join("table.parquet"))?;
        layout_probe::decode(&b, table.as_ref().unwrap())?
    };
    if disk::encode_events(&decoded)? != source_bytes {
        return Err(err("full decoded Event equality failed"));
    }
    let mut samples = Vec::new();
    let mut full_times = Vec::new();
    let mut projected_times = Vec::new();
    let mut index_build = None;
    let mut index_publication = None;
    let mut index_bytes = 0;
    let mut index_data_bytes = 0;
    let mut index_digest_bytes = 0;
    let mut index = None;
    if let Some(anchor) = &table {
        let (built, t) =
            measured(|| layout_probe::build_postings(&rows, anchor, &["rare".into()]))?;
        let (bytes, digest) = built.map_err(err)?;
        let rebuilt = layout_probe::build_postings(&rows, anchor, &["rare".into()])?;
        if rebuilt != (bytes.clone(), digest) {
            return Err(err("postings rebuild mismatch"));
        }
        let digest_wire = common::sha_hex(&bytes).into_bytes();
        let computed_digest: coverage::Digest = Sha256::digest(&bytes).into();
        if digest_wire.len() != 64 || digest != computed_digest {
            return Err(err("index digest encoding mismatch"));
        }
        index_data_bytes = bytes.len();
        index_digest_bytes = digest_wire.len();
        index_bytes = index_data_bytes + index_digest_bytes;
        let (published, publication_time) = measured(|| -> io::Result<()> {
            write_sync(&out.join("postings.json"), &bytes)?;
            write_sync(&out.join("postings-digest.txt"), &digest_wire)?;
            parent_sync(&out)
        })?;
        published?;
        index_build = Some(t);
        index_publication = Some(publication_time);
        let retained_bytes = fs::read(out.join("postings.json"))?;
        let retained_digest_wire = fs::read(out.join("postings-digest.txt"))?;
        if retained_bytes != bytes || retained_digest_wire != digest_wire {
            return Err(err("retained index or digest mismatch"));
        }
        index = Some((retained_bytes, digest));
    }
    for (i, (q, want)) in qs.iter().zip(&wants).enumerate() {
        for kind in ["full", "projected", "postings"] {
            if layout == "json64" && kind == "postings" {
                continue;
            }
            let (actual, t) = measured(|| -> io::Result<_> {
                if layout == "json64" {
                    json_scan(&out, &hashes, q)
                } else {
                    let b = fs::read(out.join("table.parquet"))?;
                    let anchor = table.as_ref().unwrap();
                    let h = match kind {
                        "full" => layout_probe::query_full(&b, anchor, q)?,
                        "projected" => layout_probe::query_projected(&b, anchor, q)?,
                        _ => {
                            let idx = index.as_ref().unwrap();
                            layout_probe::query_postings(&b, anchor, q, Some((&idx.0, idx.1)))?
                        }
                    };
                    Ok(pairs(&h))
                }
            })?;
            let actual = actual?;
            common::check(&actual, want)?;
            if kind == "full" {
                full_times.push(t.wall_ns);
            }
            if kind == "projected" {
                projected_times.push(t.wall_ns);
            }
            samples.push(Sample {
                index: i,
                family: i % 8,
                mode: kind,
                timing: t,
                matches: actual.len(),
                logical_data_bytes: layout_bytes,
            });
        }
    }
    if let Some(anchor) = &table {
        let b = fs::read(out.join("table.parquet"))?;
        let (idx, digest) = index.as_ref().unwrap();
        let q = Query {
            start_ns: i64::MIN,
            end_ns: i64::MAX,
            tenant: None,
            token: Some("rare".into()),
        };
        let want = hits(&rows, &q);
        for bad in [
            None,
            Some((&idx[..idx.len() - 1], *digest)),
            Some((&idx[..], [0; 32])),
        ] {
            let got = layout_probe::query_postings(&b, anchor, &q, bad)?;
            common::check(&pairs(&got), &want)?;
        }
        let wrong = TableAnchor {
            sha256: [0; 32],
            rows: anchor.rows,
            version: anchor.version,
        };
        let mut value: serde_json::Value = serde_json::from_slice(idx).map_err(err)?;
        value["table"] = serde_json::to_value(wrong).map_err(err)?;
        let bad = serde_json::to_vec(&value).map_err(err)?;
        let bad_hash: coverage::Digest = Sha256::digest(&bad).into();
        let got = layout_probe::query_postings(&b, anchor, &q, Some((&bad, bad_hash)))?;
        common::check(&pairs(&got), &want)?;
    }
    let output_sha256 = if layout == "json64" {
        let mut blob = Vec::new();
        for h in &hashes {
            blob.extend(h.as_bytes());
        }
        common::sha_hex(&blob)
    } else {
        common::sha_hex(&fs::read(out.join("table.parquet"))?)
    };
    let rec = ResultRow {
        schema: 1, benchmark: "layout", mode: mode.clone(), shape: shape.clone(), seed, trial,
        events: count, query_count: qs.len(), layout: layout.clone(),
        source_sha256: common::sha_hex(&source_bytes), output_sha256,
        answer_sha256: common::sha_hex(&serde_json::to_vec(&wants).map_err(err)?),
        source_bytes: source_bytes.len(), layout_bytes, metadata_bytes, parquet_footer_bytes,
        table_anchor_bytes, publication, file_read, index_build, index_publication,
        index_bytes, index_data_bytes, index_digest_bytes, samples,
        p50_full_ns: common::rank(&full_times, 50), p99_full_ns: common::rank(&full_times, 99),
        p50_projected_ns: common::rank(&projected_times, 50), p99_projected_ns: common::rank(&projected_times, 99),
        calibration, rss_kib: common::rss_kib()?, process_cpu_ns: common::cpu_ns()? - cpu_start,
        kernel_io_delta: common::io_counters()?.since(io_start), checks_ok: true,
        json_validation_boundary: (layout == "json64").then_some("Each query reads every block, compares SHA-256 to hashes retained from encoding, decodes rows, evaluates an independent predicate, and computes full matched-row digests; hash metadata is retained outside query data."),
        validation_boundary: "Before query timing, every decoded Event is compared to retained source by lossless S2 wire encoding. Each timed query reads the table file and includes full-file authentication for Parquet, or block read and per-block SHA-256 for JSON; all returned positions and complete-event row digests are checked against the independent scalar oracle. The trusted table anchor, postings and external index digest are persisted, then resident during query timing; index publication is separate from pure index construction. Kernel I/O counters span setup and checking and may show zero device bytes for cached reads.",
    };
    fs::write(
        out.join("result.json"),
        serde_json::to_vec_pretty(&rec).map_err(err)?,
    )?;
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("layout-cost: {e}");
        std::process::exit(1)
    }
}
