#[path = "common/mod.rs"]
mod common;
use common::{Measure, measured};
use serde::{Deserialize, Serialize};
use std::{fs, io, num::NonZeroUsize};
use storage_probe::coverage::{self, SealedSnapshot, Summary};
#[derive(Serialize, Deserialize)]
struct Hint {
    source_sha256: String,
    summaries: Vec<Summary>,
}
#[derive(Serialize)]
struct ResultRow {
    schema: u32,
    benchmark: &'static str,
    role: String,
    mode: String,
    shape: String,
    seed: u64,
    trial: i64,
    query_count: usize,
    source_sha256: String,
    anchor_root: Option<coverage::Digest>,
    hint_bytes: usize,
    source_bytes: usize,
    phase: Measure,
    calibration: Vec<Measure>,
    rss_kib: u64,
    process_cpu_ns: u128,
    kernel_io_delta: common::IoCounters,
    checks_ok: bool,
    fallback: Option<bool>,
    validation_boundary: &'static str,
}
fn err(e: impl std::fmt::Debug) -> io::Error {
    io::Error::other(format!("{e:?}"))
}
fn run() -> io::Result<()> {
    let a: Vec<_> = std::env::args().collect();
    if a.len() != 10 {
        return Err(err(
            "usage: sidecar-cost ROLE MODE SHAPE SEED TRIAL SOURCE HINT OUTPUT SCENARIO",
        ));
    }
    let role = &a[1];
    let mode = &a[2];
    let shape = &a[3];
    let seed = a[4].parse::<u64>().map_err(err)?;
    let trial = a[5].parse::<i64>().map_err(err)?;
    let source = &a[6];
    let hint = &a[7];
    let output = &a[8];
    let scenario = &a[9];
    let start = common::cpu_ns()?;
    let io_start = common::io_counters()?;
    let calibration = common::calibration()?;
    let (hash, root, hint_bytes, source_bytes, fallback, phase) = if role == "source" {
        let (built, phase) = measured(|| -> io::Result<_> {
            let bytes = fs::read(source)?;
            let rows = storage_probe::disk::decode_events(&bytes)?;
            let hash = common::sha_hex(&bytes);
            let summaries = rows
                .chunks(64)
                .map(coverage::summarize)
                .collect::<Result<Vec<_>, _>>()
                .map_err(err)?;
            let wire = serde_json::to_vec(&Hint {
                source_sha256: hash.clone(),
                summaries,
            })
            .map_err(err)?;
            fs::write(hint, &wire)?;
            Ok((hash, wire.len(), bytes.len()))
        })?;
        let (hash, hint_bytes, source_bytes) = built?;
        (hash, None, hint_bytes, source_bytes, None, phase)
    } else if role == "server" {
        let (built, phase) = measured(|| -> io::Result<_> {
            let bytes = fs::read(source)?;
            let rows = storage_probe::disk::decode_events(&bytes)?;
            let hash = common::sha_hex(&bytes);
            let hint_blob = if scenario == "absent" || scenario == "baseline" {
                None
            } else {
                fs::read(hint).ok()
            };
            let hint_bytes = hint_blob.as_ref().map_or(0, Vec::len);
            let parsed = hint_blob
                .as_deref()
                .and_then(|b| serde_json::from_slice::<Hint>(b).ok())
                .filter(|h| h.source_sha256 == hash);
            let offered = parsed.and_then(|h| {
                if h.summaries.len() != rows.len().div_ceil(64) {
                    return None;
                }
                if rows
                    .chunks(64)
                    .zip(&h.summaries)
                    .all(|(r, s)| coverage::validate_summary(r, s).is_ok())
                {
                    Some(h.summaries)
                } else {
                    None
                }
            });
            let fallback = offered.is_none();
            let snapshot = SealedSnapshot::new(rows, NonZeroUsize::new(64).unwrap(), seed, offered)
                .map_err(err)?;
            Ok((hash, snapshot, fallback, bytes.len(), hint_bytes))
        })?;
        let (hash, snapshot, fallback, source_bytes, hint_bytes) = built?;
        // All oracle generation, equality queries, and root comparison are outside server phase.
        let bytes = fs::read(source)?;
        let rows = storage_probe::disk::decode_events(&bytes)?;
        let (generated, base) = common::workload(shape, seed, 2048);
        if coverage::rows_digest(&rows) != coverage::rows_digest(&generated) {
            return Err(err("source mismatch"));
        }
        let central = SealedSnapshot::new(
            storage_probe::disk::decode_events(&bytes)?,
            NonZeroUsize::new(64).unwrap(),
            seed,
            None,
        )
        .map_err(err)?;
        if snapshot.anchor().root != central.anchor().root {
            return Err(err("root mismatch"));
        }
        let all = vec![true; snapshot.anchor().block_count];
        for q in common::queries(base, 2048) {
            let want = common::expected(&rows, &q);
            let actual = snapshot.query(&q, &all).map_err(err)?;
            let got = actual
                .positions
                .iter()
                .map(|&i| (i, snapshot.row_digest_at(i).unwrap()))
                .collect::<Vec<_>>();
            common::check(&got, &want)?;
            let c = central.query(&q, &all).map_err(err)?;
            if c.positions != actual.positions {
                return Err(err("query mismatch"));
            }
        }
        if scenario == "valid" && fallback || scenario != "valid" && !fallback {
            return Err(err("fallback mismatch"));
        }
        (
            hash,
            Some(snapshot.anchor().root),
            hint_bytes,
            source_bytes,
            Some(fallback),
            phase,
        )
    } else {
        return Err(err("unknown role"));
    };
    let result = ResultRow {
        schema: 1,
        benchmark: "sidecar",
        role: role.clone(),
        mode: mode.clone(),
        shape: shape.clone(),
        seed,
        trial,
        query_count: if role == "server" { 128 } else { 0 },
        source_sha256: hash,
        anchor_root: root,
        hint_bytes,
        source_bytes,
        phase,
        calibration,
        rss_kib: common::rss_kib()?,
        process_cpu_ns: common::cpu_ns()? - start,
        kernel_io_delta: common::io_counters()?.since(io_start),
        checks_ok: true,
        fallback,
        validation_boundary: "Source phase reads and decodes retained source, builds summaries, serializes and writes hints. Server phase reads and decodes source and hints, validates hints against every raw block, and builds the immutable root. Independent 128-query equality and root comparison run afterward.",
    };
    fs::write(output, serde_json::to_vec_pretty(&result).map_err(err)?)?;
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("sidecar-cost: {e}");
        std::process::exit(1)
    }
}
