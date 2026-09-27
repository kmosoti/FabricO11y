#[path = "common/mod.rs"]
mod common;
use common::{Measure, check, measured};
use serde::Serialize;
use std::{fs, io, num::NonZeroUsize, path::PathBuf};
use storage_probe::{
    Snapshot,
    coverage::{CoverageStatus, SealedSnapshot},
    resume::{self, Accumulator, Binding, ORDER_VERSION, TOKENIZER_VERSION},
};

#[derive(Serialize)]
struct QuerySample {
    index: usize,
    family: usize,
    mode: &'static str,
    timing: Measure,
    matches: usize,
    page_bytes: Option<usize>,
}
#[derive(Serialize)]
struct Record {
    schema: u32,
    benchmark: &'static str,
    mode: String,
    shape: String,
    seed: u64,
    trial: i64,
    events: usize,
    query_count: usize,
    source_sha256: String,
    answer_sha256: String,
    calibration: Vec<Measure>,
    scalar_build: Measure,
    sealed_build: Measure,
    samples: Vec<QuerySample>,
    verify: Measure,
    partial_resume_merge: Measure,
    replay: Measure,
    full_page_bytes: usize,
    two_page_bytes: usize,
    residual_bytes: usize,
    p50_scalar_ns: u128,
    p99_scalar_ns: u128,
    p50_receipt_ns: u128,
    p99_receipt_ns: u128,
    rss_kib: u64,
    process_cpu_ns: u128,
    kernel_io_delta: common::IoCounters,
    checks_ok: bool,
    validation_boundary: &'static str,
}
fn err(e: impl std::fmt::Debug) -> io::Error {
    io::Error::other(format!("{e:?}"))
}
fn run() -> io::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 6 && args[1] == "fixture" {
        let seed = args[3].parse::<u64>().map_err(err)?;
        let count = args[4].parse::<usize>().map_err(err)?;
        let (rows, _) = common::workload(&args[2], seed, count);
        let bytes = storage_probe::disk::encode_events(&rows)?;
        fs::write(&args[5], bytes)?;
        return Ok(());
    }
    if args.len() != 7 {
        return Err(io::Error::other(
            "usage: receipt-cost MODE SHAPE SEED TRIAL SOURCE OUTPUT",
        ));
    }
    let mode = &args[1];
    let shape = &args[2];
    let seed = args[3].parse::<u64>().map_err(err)?;
    let trial = args[4].parse::<i64>().map_err(err)?;
    let source = PathBuf::from(&args[5]);
    let output = PathBuf::from(&args[6]);
    let cpu_start = common::cpu_ns()?;
    let io_start = common::io_counters()?;
    let bytes = fs::read(&source)?;
    let rows = storage_probe::disk::decode_events(&bytes)?;
    let (generated, base) = common::workload(shape, seed, 2048);
    if storage_probe::coverage::rows_digest(&rows)
        != storage_probe::coverage::rows_digest(&generated)
    {
        return Err(err("source fixture mismatch"));
    }
    let queries = common::queries(base, 2048);
    let wants: Vec<_> = queries.iter().map(|q| common::expected(&rows, q)).collect();
    let block = NonZeroUsize::new(64).unwrap();
    let calibration = common::calibration()?;
    let scalar_input = storage_probe::disk::decode_events(&bytes)?;
    let sealed_input = storage_probe::disk::decode_events(&bytes)?;
    let (scalar, scalar_build) = measured(|| Snapshot::new(scalar_input, block))?;
    std::hint::black_box(&scalar);
    let (sealed, sealed_build) = measured(|| SealedSnapshot::new(sealed_input, block, seed, None))?;
    let sealed = sealed.map_err(err)?;
    let available = vec![true; sealed.anchor().block_count];
    let mut samples = Vec::new();
    let mut scalar_times = Vec::new();
    let mut receipt_times = Vec::new();
    let mut full_page_bytes = 0;
    for (i, (q, want)) in queries.iter().zip(&wants).enumerate() {
        let order = if (trial.rem_euclid(2) as usize + i) % 2 == 0 {
            ["scalar", "receipt"]
        } else {
            ["receipt", "scalar"]
        };
        for variant in order {
            if variant == "scalar" {
                // Direct source Vec scan and complete row-digest calculation inside timing.
                let (answer, timing) = measured(|| common::expected(&rows, q))?;
                check(&answer, want)?;
                scalar_times.push(timing.wall_ns);
                samples.push(QuerySample {
                    index: i,
                    family: i % 8,
                    mode: "scalar",
                    timing,
                    matches: answer.len(),
                    page_bytes: None,
                });
            } else {
                let binding = Binding {
                    anchor: sealed.anchor().clone(),
                    query: q.clone(),
                    tokenizer_version: TOKENIZER_VERSION,
                    order_version: ORDER_VERSION,
                };
                let (verified, timing) = measured(|| {
                    let page = resume::execute(&sealed, &binding, &available)?;
                    Accumulator::new(binding.clone(), page)
                })?;
                let accumulator = verified.map_err(err)?;
                let actual = accumulator
                    .rows()
                    .iter()
                    .map(|r| (r.block * 64 + r.offset, r.digest))
                    .collect::<Vec<_>>();
                check(&actual, want)?;
                // Serialization is explicitly outside the query timer. Recreate the
                // identical page because verification consumes the measured page.
                let page = resume::execute(&sealed, &binding, &available).map_err(err)?;
                let page_rows = page
                    .rows
                    .iter()
                    .map(|r| (r.block * 64 + r.offset, r.digest))
                    .collect::<Vec<_>>();
                check(&page_rows, want)?;
                let wire = serde_json::to_vec(&page).map_err(err)?;
                full_page_bytes += wire.len();
                if accumulator.status() != CoverageStatus::Complete {
                    return Err(err("full page incomplete"));
                }
                receipt_times.push(timing.wall_ns);
                samples.push(QuerySample {
                    index: i,
                    family: i % 8,
                    mode: "receipt",
                    timing,
                    matches: actual.len(),
                    page_bytes: Some(wire.len()),
                });
            }
        }
    }
    let q = &queries[0];
    let want = &wants[0];
    let binding = Binding {
        anchor: sealed.anchor().clone(),
        query: q.clone(),
        tokenizer_version: TOKENIZER_VERSION,
        order_version: ORDER_VERSION,
    };
    let full = resume::execute(&sealed, &binding, &available).map_err(err)?;
    let cloned = full.clone();
    let (verified, verify) = measured(|| Accumulator::new(binding.clone(), cloned))?;
    let verified = verified.map_err(err)?;
    let got = verified
        .rows()
        .into_iter()
        .map(|r| (r.block * 64 + r.offset, r.digest))
        .collect::<Vec<_>>();
    check(&got, want)?;
    let mut left = available.clone();
    let mut right = available.clone();
    for i in 0..left.len() {
        left[i] = i % 2 == 0;
        right[i] = !left[i];
    }
    let (partial, partial_resume_merge) = measured(|| -> Result<_, resume::ResumeError> {
        let a = resume::execute(&sealed, &binding, &left)?;
        let mut acc = Accumulator::new(binding.clone(), a.clone())?;
        let residual = acc.residual();
        let b = resume::resume(&sealed, &residual, &right)?;
        acc.merge(b.clone())?;
        Ok((a, b, acc))
    })?;
    let (a, b, mut acc) = partial.map_err(err)?;
    let initial = Accumulator::new(binding.clone(), a.clone()).map_err(err)?;
    let residual_bytes = serde_json::to_vec(&initial.residual()).map_err(err)?.len();
    if !acc.residual().blocks.is_empty() {
        return Err(err("residual remains"));
    }
    let got = acc
        .rows()
        .into_iter()
        .map(|r| (r.block * 64 + r.offset, r.digest))
        .collect::<Vec<_>>();
    check(&got, want)?;
    let duplicate = b.clone();
    let (replayed, replay) = measured(|| acc.merge(duplicate))?;
    replayed.map_err(err)?;
    let two_page_bytes =
        serde_json::to_vec(&a).map_err(err)?.len() + serde_json::to_vec(&b).map_err(err)?.len();
    let rec = Record {
        schema: 1,
        benchmark: "receipt",
        mode: mode.clone(),
        shape: shape.clone(),
        seed,
        trial,
        events: rows.len(),
        query_count: queries.len(),
        source_sha256: common::sha_hex(&bytes),
        answer_sha256: common::sha_hex(&serde_json::to_vec(&wants).map_err(err)?),
        calibration,
        scalar_build,
        sealed_build,
        samples,
        verify,
        partial_resume_merge,
        replay,
        full_page_bytes,
        two_page_bytes,
        residual_bytes,
        p50_scalar_ns: common::rank(&scalar_times, 50),
        p99_scalar_ns: common::rank(&scalar_times, 99),
        p50_receipt_ns: common::rank(&receipt_times, 50),
        p99_receipt_ns: common::rank(&receipt_times, 99),
        rss_kib: common::rss_kib()?,
        process_cpu_ns: common::cpu_ns()? - cpu_start,
        kernel_io_delta: common::io_counters()?.since(io_start),
        checks_ok: true,
        validation_boundary: "Independent scalar positions plus complete-event row digests gate every query; source and oracle are prepared outside timed operations. Query timing excludes JSON serialization; process CPU sample includes checking through result assembly.",
    };
    fs::write(output, serde_json::to_vec_pretty(&rec).map_err(err)?)?;
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("receipt-cost: {e}");
        std::process::exit(1)
    }
}
